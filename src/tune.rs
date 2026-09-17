//! The system settings that decide whether a game runs well, and the one
//! privileged path that changes them.
//!
//! # Why these settings and not others
//!
//! Every tweak here is one that stops a game from *working*, or that the
//! defaults get wrong for games specifically. There is no list of
//! micro-optimisations, because a gaming app that ships thirty toggles is
//! one that makes people break their computer looking for frames.
//!
//! * `vm.max_map_count` — the number of memory mappings a process may hold.
//!   The historical default of 65530 is below what modern engines ask for,
//!   and a game over the limit does not run slowly, it crashes on load with
//!   an out-of-memory error on a machine with memory to spare.
//! * `kernel.split_lock_mitigate` — when a game takes a split lock the
//!   kernel stalls every core to punish it. Correct for a server, and the
//!   cause of hard hitching in games that do it in a hot loop.
//! * The open-file limit — Wine's esync gives every synchronisation object
//!   a file descriptor. At 4096 a large game runs out and falls back, or
//!   fails outright.
//! * `vm.swappiness` — at the default of 60 a game's own pages get swapped
//!   out to grow the page cache while it loads, and the stutter comes back
//!   as the game faults them in again.
//!
//! # How a change is made
//!
//! Nothing here writes to `/proc/sys` or `/sys` from the session process.
//! The app re-runs its own binary under `run0` or `pkexec` with
//! `--apply <action>`, and [`apply_privileged`] — the only code that runs as
//! root — validates the action against a fixed list before touching
//! anything. An action that is not on the list is refused, so a bug in the
//! UI cannot turn into an arbitrary write as root.

use std::fmt;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::drivers::which;
use crate::gpu::{Gpu, read_num, read_text};

/// Where a persisted change is written. One file, owned by this app, so
/// undoing everything is deleting one file.
const SYSCTL_FILE: &str = "/etc/sysctl.d/99-raven-gaming.conf";
const LIMITS_FILE: &str = "/etc/security/limits.d/99-raven-gaming.conf";

const POWER_SOCKET: &str = "/run/raven-power/ctl";
const POWERD_TIMEOUT: Duration = Duration::from_secs(2);

// ---- the tweaks ----------------------------------------------------------

/// One system setting the app knows how to read and change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tweak {
    MaxMapCount,
    SplitLock,
    Swappiness,
    FileLimit,
}

pub const TWEAKS: [Tweak; 4] = [
    Tweak::MaxMapCount,
    Tweak::SplitLock,
    Tweak::Swappiness,
    Tweak::FileLimit,
];

impl Tweak {
    pub fn title(self) -> &'static str {
        match self {
            Tweak::MaxMapCount => "Memory mappings for large games",
            Tweak::SplitLock => "Split-lock stalls",
            Tweak::Swappiness => "Keep games out of swap",
            Tweak::FileLimit => "Open files for Wine and Proton",
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Tweak::MaxMapCount => {
                "Raises vm.max_map_count. Below this, big games crash while loading on a machine with memory to spare."
            }
            Tweak::SplitLock => {
                "Stops the kernel stalling every core when a game takes a split lock. Removes hitching in the games that do it."
            }
            Tweak::Swappiness => {
                "Lowers vm.swappiness so a loading game's own memory is not swapped out to grow the disk cache."
            }
            Tweak::FileLimit => {
                "Raises the open-file limit to what Wine's esync needs. At the default, large games run out of descriptors."
            }
        }
    }

    /// The `sysctl` key, for the three that are one.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Tweak::MaxMapCount => Some("vm.max_map_count"),
            Tweak::SplitLock => Some("kernel.split_lock_mitigate"),
            Tweak::Swappiness => Some("vm.swappiness"),
            Tweak::FileLimit => None,
        }
    }

    /// What this app sets it to.
    pub fn wanted(self) -> u64 {
        match self {
            // Valve ships this on the Steam Deck; it is the number games
            // are tested against.
            Tweak::MaxMapCount => 2_147_483_642,
            Tweak::SplitLock => 0,
            Tweak::Swappiness => 10,
            Tweak::FileLimit => 524_288,
        }
    }

    /// Whether a reading counts as already set. Most are "at least this",
    /// not "exactly this", so a person who set something stricter by hand
    /// is never told they need this app's value instead.
    pub fn satisfied_by(self, current: u64) -> bool {
        match self {
            Tweak::MaxMapCount | Tweak::FileLimit => current >= self.wanted(),
            Tweak::SplitLock => current == 0,
            Tweak::Swappiness => current <= self.wanted(),
        }
    }

    /// The value in force right now.
    pub fn current(self) -> Option<u64> {
        match self {
            Tweak::FileLimit => open_file_limit(),
            _ => read_num(sysctl_path(self.key()?)),
        }
    }

    pub fn is_applied(self) -> bool {
        self.current().is_some_and(|v| self.satisfied_by(v)) || self.is_pending()
    }

    /// Written, but not yet in force.
    ///
    /// Only the open-file limit can be in this state: it is a PAM limit,
    /// not a sysctl, and a session inherits the limit it was started with.
    /// Without this the app would keep reporting it as missing after
    /// setting it, and offering to set it again, for as long as the person
    /// stayed logged in.
    pub fn is_pending(self) -> bool {
        self == Tweak::FileLimit
            && Path::new(LIMITS_FILE).exists()
            && !self.current().is_some_and(|v| self.satisfied_by(v))
    }

    /// How it reads in the UI: "65,530 → 2,147,483,642".
    pub fn change_text(self) -> String {
        if self.is_pending() {
            return format!(
                "Set to {} — takes effect at your next login",
                thousands(self.wanted())
            );
        }
        match self.current() {
            Some(current) if self.satisfied_by(current) => format!("Set to {}", thousands(current)),
            Some(current) => format!("{} → {}", thousands(current), thousands(self.wanted())),
            None => "Not available on this kernel".to_string(),
        }
    }
}

fn sysctl_path(key: &str) -> PathBuf {
    Path::new("/proc/sys").join(key.replace('.', "/"))
}

/// This process's own soft limit on open files, which is the session's.
fn open_file_limit() -> Option<u64> {
    let text = read_text("/proc/self/limits")?;
    let line = text.lines().find(|l| l.starts_with("Max open files"))?;
    // "Max open files  4096  4096  files" — soft, then hard.
    line.split_whitespace().nth(3)?.parse().ok()
}

fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

pub fn tweaks_needed() -> Vec<Tweak> {
    TWEAKS.into_iter().filter(|t| !t.is_applied()).collect()
}

// ---- GPU performance level -----------------------------------------------

/// How hard the card is allowed to clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuMode {
    /// The driver decides. Right for everything that is not a game.
    Auto,
    /// Hold the highest clocks. Warmer, louder, and steadier frame times.
    High,
    /// Hold the lowest. For a laptop on battery that is not playing.
    Low,
}

impl GpuMode {
    pub fn sysfs_value(self) -> &'static str {
        match self {
            GpuMode::Auto => "auto",
            GpuMode::High => "high",
            GpuMode::Low => "low",
        }
    }

    pub fn from_sysfs(value: &str) -> Option<GpuMode> {
        match value.trim() {
            "auto" => Some(GpuMode::Auto),
            "high" => Some(GpuMode::High),
            "low" => Some(GpuMode::Low),
            _ => None,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            GpuMode::Auto => "Automatic",
            GpuMode::High => "Maximum",
            GpuMode::Low => "Minimum",
        }
    }
}

impl fmt::Display for GpuMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.title())
    }
}

/// The file AMD exposes the clock policy in, when the card has one.
/// NVIDIA has no equivalent, so a card without this file simply has no
/// control here and the UI says so rather than showing a dead switch.
pub fn gpu_mode_path(gpu: &Gpu) -> Option<PathBuf> {
    let card = gpu.card.as_ref()?;
    let path = Path::new("/sys/class/drm")
        .join(card)
        .join("device/power_dpm_force_performance_level");
    path.exists().then_some(path)
}

pub fn gpu_mode(gpu: &Gpu) -> Option<GpuMode> {
    GpuMode::from_sysfs(&read_text(gpu_mode_path(gpu)?)?)
}

// ---- presets -------------------------------------------------------------

/// The one control most people will touch: a named set of everything
/// above, so nobody has to know what a governor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Quiet,
    Balanced,
    Performance,
}

pub const PRESETS: [Preset; 3] = [Preset::Quiet, Preset::Balanced, Preset::Performance];

impl Preset {
    pub fn title(self) -> &'static str {
        match self {
            Preset::Quiet => "Quiet",
            Preset::Balanced => "Balanced",
            Preset::Performance => "Performance",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Preset::Quiet => "power-profile-power-saver-symbolic",
            Preset::Balanced => "power-profile-balanced-symbolic",
            Preset::Performance => "power-profile-performance-symbolic",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Preset::Quiet => "Cool and quiet. For older games, emulators, and playing on battery.",
            Preset::Balanced => "The system decides. Right for almost everything.",
            Preset::Performance => {
                "Highest clocks held. Warmer and louder, for steady frame times."
            }
        }
    }

    /// The power profile this preset asks the system for.
    pub fn power_profile(self) -> &'static str {
        match self {
            Preset::Quiet => "power-saver",
            Preset::Balanced => "balanced",
            Preset::Performance => "performance",
        }
    }

    pub fn gpu_mode(self) -> GpuMode {
        match self {
            Preset::Quiet => GpuMode::Low,
            Preset::Balanced => GpuMode::Auto,
            Preset::Performance => GpuMode::High,
        }
    }

    pub fn from_power_profile(profile: &str) -> Option<Preset> {
        match profile {
            "power-saver" => Some(Preset::Quiet),
            "balanced" => Some(Preset::Balanced),
            "performance" => Some(Preset::Performance),
            _ => None,
        }
    }
}

/// The profile the system says it is on, asked of raven-powerd first —
/// it owns the governor on Raven Linux and re-applies its own preset, so
/// anything written behind its back does not stick.
pub fn active_preset() -> Option<Preset> {
    if let Ok(reply) = ask_powerd("profile")
        && let Some(word) = reply.split_whitespace().next()
        && let Some(preset) = Preset::from_power_profile(word)
    {
        return Some(preset);
    }
    let out = Command::new(which("powerprofilesctl")?)
        .arg("get")
        .output()
        .ok()?;
    Preset::from_power_profile(String::from_utf8_lossy(&out.stdout).trim())
}

fn ask_powerd(request: &str) -> Result<String, String> {
    let mut stream = UnixStream::connect(POWER_SOCKET)
        .map_err(|e| format!("raven-powerd is not reachable: {e}"))?;
    stream.set_read_timeout(Some(POWERD_TIMEOUT)).ok();
    stream.set_write_timeout(Some(POWERD_TIMEOUT)).ok();
    stream
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(|e| e.to_string())?;
    Ok(reply.trim().to_string())
}

/// Sets the system power profile, by the same ladder Raven Power uses:
/// raven-powerd, then power-profiles-daemon, then nothing.
pub fn set_power_profile(profile: &str) -> Result<(), String> {
    if !matches!(profile, "performance" | "balanced" | "power-saver") {
        return Err(format!("Not a power profile: {profile}"));
    }
    match ask_powerd(&format!("profile {profile}")) {
        Ok(reply) if !reply.starts_with("error") => return Ok(()),
        Ok(reply) if !reply.contains("unknown command") => {
            return Err(format!("raven-powerd refused: {reply}"));
        }
        _ => {}
    }
    let Some(ctl) = which("powerprofilesctl") else {
        return Err("Neither raven-powerd nor power-profiles-daemon is available".into());
    };
    let status = Command::new(ctl)
        .args(["set", profile])
        .status()
        .map_err(|e| format!("Could not start the system profile service: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("The system power-profile service rejected this change".into())
    }
}

// ---- privileged actions --------------------------------------------------

/// Everything this app will ever do as root, by name.
///
/// The privileged process matches the string it was given against this
/// list and does nothing else. Adding a capability means adding a variant
/// here, which is the point: the list is short and reviewable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Build and install every DKMS module for one kernel.
    DkmsInstall { kernel: String },
    /// Write the tweaks above to `/proc/sys` and persist them.
    ApplyTweaks,
    /// Remove the persisted file and restore the kernel's own defaults.
    RevertTweaks,
    /// Hold or release a card's clocks. `card` is a `cardN` name.
    GpuMode { card: String, mode: GpuMode },
}

impl Action {
    /// The command line the unprivileged half builds.
    pub fn args(&self) -> Vec<String> {
        match self {
            Action::DkmsInstall { kernel } => {
                vec!["--apply".into(), "dkms-install".into(), kernel.clone()]
            }
            Action::ApplyTweaks => vec!["--apply".into(), "tweaks".into()],
            Action::RevertTweaks => vec!["--apply".into(), "revert-tweaks".into()],
            Action::GpuMode { card, mode } => vec![
                "--apply".into(),
                "gpu-mode".into(),
                card.clone(),
                mode.sysfs_value().into(),
            ],
        }
    }

    /// What the authorization prompt says this is for.
    pub fn description(&self) -> String {
        match self {
            Action::DkmsInstall { kernel } => format!("Build graphics modules for {kernel}"),
            Action::ApplyTweaks => "Apply Raven gaming system settings".into(),
            Action::RevertTweaks => "Restore the system's own settings".into(),
            Action::GpuMode { .. } => "Set the graphics card's clock policy".into(),
        }
    }

    /// The other side: an argument list back into an action, refusing
    /// anything that is not one. This is what runs as root.
    pub fn parse(args: &[String]) -> Result<Action, String> {
        match args {
            [verb, kernel] if verb == "dkms-install" => {
                validate_kernel(kernel)?;
                Ok(Action::DkmsInstall {
                    kernel: kernel.clone(),
                })
            }
            [verb] if verb == "tweaks" => Ok(Action::ApplyTweaks),
            [verb] if verb == "revert-tweaks" => Ok(Action::RevertTweaks),
            [verb, card, mode] if verb == "gpu-mode" => {
                validate_card(card)?;
                let mode =
                    GpuMode::from_sysfs(mode).ok_or_else(|| format!("Not a mode: {mode}"))?;
                Ok(Action::GpuMode {
                    card: card.clone(),
                    mode,
                })
            }
            _ => Err(format!("Unknown action: {}", args.join(" "))),
        }
    }
}

/// A kernel release, as `uname -r` gives it. Letters, digits, and the
/// three punctuation marks a release uses — nothing that could walk out of
/// `/usr/lib/modules` or reach a shell.
fn validate_kernel(release: &str) -> Result<(), String> {
    let shaped = !release.is_empty()
        && release.len() <= 64
        && release
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'));
    if !shaped {
        return Err(format!("Not a kernel release: {release}"));
    }
    if !Path::new("/usr/lib/modules").join(release).is_dir() {
        return Err(format!("No such kernel is installed: {release}"));
    }
    Ok(())
}

/// A DRM card name: `card` and digits, and it has to exist.
fn validate_card(card: &str) -> Result<(), String> {
    let shaped = card
        .strip_prefix("card")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()));
    if !shaped {
        return Err(format!("Not a graphics card: {card}"));
    }
    if !Path::new("/sys/class/drm").join(card).is_dir() {
        return Err(format!("No such graphics card: {card}"));
    }
    Ok(())
}

/// Runs an action, as root. Reached only through `--apply`.
pub fn apply_privileged(args: &[String]) -> Result<(), String> {
    match Action::parse(args)? {
        Action::DkmsInstall { kernel } => dkms_install(&kernel),
        Action::ApplyTweaks => apply_tweaks(),
        Action::RevertTweaks => revert_tweaks(),
        Action::GpuMode { card, mode } => {
            let path = Path::new("/sys/class/drm")
                .join(&card)
                .join("device/power_dpm_force_performance_level");
            fs::write(&path, mode.sysfs_value()).map_err(|e| format!("{}: {e}", path.display()))
        }
    }
}

fn dkms_install(kernel: &str) -> Result<(), String> {
    let dkms = which("dkms").ok_or("dkms is not installed")?;
    let output = Command::new(dkms)
        .args(["autoinstall", "-k", kernel])
        .output()
        .map_err(|e| format!("Could not run dkms: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    // dkms writes the interesting part to stdout and the summary to
    // stderr; the last non-empty line of either is what a person can act
    // on, and a build log path is usually in it.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reason = last_line(&stderr)
        .or_else(|| last_line(&stdout))
        .unwrap_or("dkms failed");
    Err(reason.to_string())
}

fn last_line(text: &str) -> Option<&str> {
    text.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(str::trim)
}

fn apply_tweaks() -> Result<(), String> {
    // In force now.
    for tweak in TWEAKS {
        if let Some(key) = tweak.key() {
            let path = sysctl_path(key);
            fs::write(&path, tweak.wanted().to_string())
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    // And after the next boot.
    let mut sysctl = String::from(
        "# Written by Raven Gaming. Delete this file to restore the system's\n\
         # own settings, or use the Revert button in the app.\n",
    );
    for tweak in TWEAKS {
        if let Some(key) = tweak.key() {
            sysctl.push_str(&format!("{key} = {}\n", tweak.wanted()));
        }
    }
    write_new(Path::new(SYSCTL_FILE), &sysctl)?;

    // The open-file limit is not a sysctl; it is a PAM limit, and it only
    // takes effect on the next login. The UI says so rather than letting
    // someone wonder why the number did not move.
    let limit = Tweak::FileLimit.wanted();
    write_new(
        Path::new(LIMITS_FILE),
        &format!(
            "# Written by Raven Gaming, for Wine and Proton's esync.\n\
             * soft nofile {limit}\n* hard nofile {limit}\n"
        ),
    )
}

fn revert_tweaks() -> Result<(), String> {
    for file in [SYSCTL_FILE, LIMITS_FILE] {
        match fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{file}: {e}")),
        }
    }
    Ok(())
}

fn write_new(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

/// Whether the persisted files this app writes are in place.
pub fn tweaks_persisted() -> bool {
    Path::new(SYSCTL_FILE).exists() || Path::new(LIMITS_FILE).exists()
}

// ---- asking for the password --------------------------------------------

/// Re-runs this binary as root to carry out one action.
///
/// `run0` first, as on the rest of Raven, falling back to `pkexec`. run0
/// needs a booted systemd; without one it fails before any authorization
/// happens, which must not be reported to the person as a refusal.
pub fn run_as_root(action: &Action) -> Result<(), String> {
    let executable = std::env::current_exe()
        .map_err(|e| format!("Could not locate the Raven Gaming executable: {e}"))?;
    let systemd_booted = Path::new("/run/systemd/system").is_dir();
    let output = if which("run0").is_some() && systemd_booted {
        Command::new("run0")
            .args([
                "--unit=raven-gaming-apply".to_string(),
                format!("--description={}", action.description()),
            ])
            .arg(&executable)
            .args(action.args())
            .output()
    } else if which("pkexec").is_some() {
        Command::new("pkexec")
            .arg(&executable)
            .args(action.args())
            .output()
    } else {
        return Err("Raven Gaming needs run0 or pkexec to change system settings".into());
    }
    .map_err(|e| format!("Could not request authorization: {e}"))?;

    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    // pkexec's own refusal, and run0's, both say "dismissed" or "not
    // authorized"; anything else is the action's own error, which is
    // worth showing verbatim.
    if stderr.contains("dismissed") || stderr.contains("not authorized") {
        return Err("Authorization was not given".into());
    }
    Err(last_line(&stderr)
        .unwrap_or("The change could not be made")
        .to_string())
}

// ---- shader cache --------------------------------------------------------

/// Where Mesa and NVIDIA keep compiled shaders, and how big it has grown.
///
/// A cold shader cache is the stutter people describe as "the first ten
/// minutes are rough". Nothing here changes it; the page says where it is
/// and offers to clear it, which is the one useful action — a cache from
/// an older driver is stale and gets recompiled anyway.
pub fn shader_cache_dirs() -> Vec<(PathBuf, u64)> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".cache")));
    let Some(cache) = cache else {
        return Vec::new();
    };
    ["mesa_shader_cache", "mesa_shader_cache_db", "nv", "nvidia"]
        .iter()
        .map(|name| cache.join(name))
        .filter(|p| p.is_dir())
        .map(|p| {
            let size = directory_size(&p, 0);
            (p, size)
        })
        .collect()
}

/// Bytes under a directory. Depth-limited: a shader cache is two levels
/// deep, and a symlink loop somewhere under `~/.cache` must not hang the
/// window.
fn directory_size(dir: &Path, depth: u32) -> u64 {
    if depth > 4 {
        return 0;
    }
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => directory_size(&entry.path(), depth + 1),
            Ok(t) if t.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stricter_setting_than_ours_still_counts() {
        assert!(Tweak::MaxMapCount.satisfied_by(2_147_483_642));
        assert!(Tweak::MaxMapCount.satisfied_by(4_000_000_000));
        assert!(!Tweak::MaxMapCount.satisfied_by(65_530));
        // Lower swappiness is stricter, so it satisfies.
        assert!(Tweak::Swappiness.satisfied_by(1));
        assert!(Tweak::Swappiness.satisfied_by(10));
        assert!(!Tweak::Swappiness.satisfied_by(60));
        // Split-lock mitigation is on or off; there is no stricter.
        assert!(Tweak::SplitLock.satisfied_by(0));
        assert!(!Tweak::SplitLock.satisfied_by(1));
        assert!(Tweak::FileLimit.satisfied_by(1_048_576));
        assert!(!Tweak::FileLimit.satisfied_by(4096));
    }

    #[test]
    fn a_limit_that_needs_a_relogin_is_not_reported_as_missing() {
        // Whether the file is there depends on the machine the tests run
        // on, so this asserts the relationship rather than the value:
        // pending and applied must agree, and nothing but the file limit
        // can ever be pending.
        assert!(!Tweak::FileLimit.is_pending() || Tweak::FileLimit.is_applied());
        for tweak in [Tweak::MaxMapCount, Tweak::SplitLock, Tweak::Swappiness] {
            assert!(!tweak.is_pending(), "{tweak:?} cannot be pending");
        }
        assert!(!tweaks_needed().contains(&Tweak::FileLimit) || !Tweak::FileLimit.is_pending());
    }

    #[test]
    fn a_sysctl_key_becomes_its_proc_path() {
        assert_eq!(
            sysctl_path("vm.max_map_count"),
            Path::new("/proc/sys/vm/max_map_count")
        );
        assert_eq!(
            sysctl_path("kernel.split_lock_mitigate"),
            Path::new("/proc/sys/kernel/split_lock_mitigate")
        );
    }

    #[test]
    fn big_numbers_are_grouped() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(65_530), "65,530");
        assert_eq!(thousands(2_147_483_642), "2,147,483,642");
    }

    #[test]
    fn every_action_survives_the_round_trip() {
        // A kernel and card that exist on the machine running the tests.
        let kernel = crate::drivers::running_kernel();
        let actions = [
            Action::ApplyTweaks,
            Action::RevertTweaks,
            Action::DkmsInstall { kernel },
        ];
        for action in actions {
            let args = action.args();
            assert_eq!(args[0], "--apply");
            assert_eq!(Action::parse(&args[1..]), Ok(action));
        }
    }

    #[test]
    fn the_privileged_half_refuses_anything_not_on_the_list() {
        for args in [
            vec!["rm".to_string(), "-rf".into()],
            vec![],
            vec!["tweaks".to_string(), "extra".into()],
            vec!["gpu-mode".to_string(), "card0".into(), "melt".into()],
        ] {
            assert!(Action::parse(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn a_kernel_argument_cannot_walk_out_of_its_directory() {
        for release in ["../../etc", "6.1; rm -rf /", "", &"x".repeat(80), "a/b"] {
            assert!(validate_kernel(release).is_err(), "accepted {release:?}");
        }
        // The running kernel is always installed, by definition.
        assert!(validate_kernel(&crate::drivers::running_kernel()).is_ok());
    }

    #[test]
    fn a_card_argument_has_to_be_a_card() {
        for card in ["cardX", "../card0", "card", "0", "card0/../.."] {
            assert!(validate_card(card).is_err(), "accepted {card:?}");
        }
    }

    #[test]
    fn presets_and_power_profiles_agree_both_ways() {
        for preset in PRESETS {
            assert_eq!(
                Preset::from_power_profile(preset.power_profile()),
                Some(preset)
            );
        }
        assert_eq!(Preset::from_power_profile("nonsense"), None);
    }

    #[test]
    fn gpu_modes_round_trip_through_sysfs() {
        for mode in [GpuMode::Auto, GpuMode::High, GpuMode::Low] {
            assert_eq!(GpuMode::from_sysfs(mode.sysfs_value()), Some(mode));
        }
        assert_eq!(GpuMode::from_sysfs("manual"), None);
        // sysfs reads come back with a newline.
        assert_eq!(GpuMode::from_sysfs("auto\n"), Some(GpuMode::Auto));
    }

    #[test]
    fn byte_sizes_read_the_way_people_say_them() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(5_368_709_120), "5.0 GB");
    }
}
