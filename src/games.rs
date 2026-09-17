//! The games on this computer, and the line that makes each one run right.
//!
//! # What gets found
//!
//! Steam's library, from the VDF files Steam itself keeps, plus Lutris and
//! Heroic when they are installed. Nothing is launched from here and no
//! account is touched: the app reads the same files the launcher reads, so
//! it cannot get a library into a state the launcher disagrees with.
//!
//! # Why launch options are the output
//!
//! On a laptop with two graphics cards, a game started normally runs on the
//! integrated one — the desktop is already there, and nothing tells the
//! game otherwise. It works, so nobody suspects it; it is just inexplicably
//! slow. The fix is four environment variables that nobody can be expected
//! to remember, so this page writes them out for the machine they are
//! actually on, ready to paste into Steam's launch options.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::drivers::which;
use crate::gpu::{Gpu, Vendor, is_hybrid};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    Steam,
    Lutris,
    Heroic,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Steam => "Steam",
            Source::Lutris => "Lutris",
            Source::Heroic => "Heroic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub name: String,
    pub source: Source,
    /// Steam's app id, when there is one. It is what the person needs to
    /// look anything up.
    pub app_id: Option<String>,
    /// Bytes on disk, when the launcher recorded it.
    pub size_on_disk: u64,
    /// Seconds since the epoch, when the launcher recorded it.
    pub last_played: Option<u64>,
    pub install_dir: Option<PathBuf>,
}

impl Game {
    /// "16.1 GB · last played 3 days ago", or as much of it as is known.
    pub fn detail(&self) -> String {
        let mut parts = Vec::new();
        if self.size_on_disk > 0 {
            parts.push(crate::tune::human_bytes(self.size_on_disk));
        }
        if let Some(when) = self.last_played.filter(|t| *t > 0) {
            parts.push(format!("last played {}", ago(when)));
        }
        if parts.is_empty() {
            parts.push("installed".into());
        }
        parts.join(" · ")
    }
}

/// "3 days ago", from a unix timestamp. Coarse on purpose: nobody needs
/// the minute they last played something.
fn ago(timestamp: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let seconds = now.saturating_sub(timestamp);
    match seconds {
        0..=3599 => "in the last hour".into(),
        s if s < 172_800 => "yesterday".into(),
        s if s < 2_592_000 => format!("{} days ago", s / 86_400),
        s if s < 63_072_000 => format!("{} months ago", s / 2_592_000),
        s => format!("{} years ago", s / 31_536_000),
    }
}

/// Every game found, best-known first.
pub fn discover() -> Vec<Game> {
    let mut games = steam_games();
    games.extend(lutris_games());
    games.extend(heroic_games());
    games.sort_by(|a, b| {
        b.last_played
            .cmp(&a.last_played)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    games
}

// ---- Steam ---------------------------------------------------------------

/// Where Steam might be, in the order Steam itself looks.
fn steam_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    [
        home.join(".local/share/Steam"),
        home.join(".steam/steam"),
        home.join(".steam/root"),
        home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"),
    ]
    .into_iter()
    .filter(|p| p.join("steamapps").is_dir())
    .collect()
}

pub fn steam_installed() -> bool {
    which("steam").is_some() || !steam_roots().is_empty()
}

fn steam_games() -> Vec<Game> {
    let mut games = Vec::new();
    let mut seen_libraries: Vec<PathBuf> = Vec::new();
    for root in steam_roots() {
        for library in steam_libraries(&root) {
            // A root found twice under different names (`.steam/steam` is
            // usually a symlink to the real one) must not list its games
            // twice. Canonical paths make the two the same path.
            let key = library.canonicalize().unwrap_or_else(|_| library.clone());
            if seen_libraries.contains(&key) {
                continue;
            }
            seen_libraries.push(key);
            games.extend(games_in_library(&library));
        }
    }
    games
}

/// Every `steamapps` directory Steam knows about, including libraries on
/// other drives, from `libraryfolders.vdf`.
fn steam_libraries(root: &Path) -> Vec<PathBuf> {
    let steamapps = root.join("steamapps");
    let mut libraries = vec![steamapps.clone()];
    let Ok(text) = fs::read_to_string(steamapps.join("libraryfolders.vdf")) else {
        return libraries;
    };
    for path in vdf_values(&text, "path") {
        let candidate = Path::new(&path).join("steamapps");
        if candidate.is_dir() && !libraries.contains(&candidate) {
            libraries.push(candidate);
        }
    }
    libraries
}

fn games_in_library(steamapps: &Path) -> Vec<Game> {
    fs::read_dir(steamapps)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("appmanifest_"))
        })
        .filter_map(|path| fs::read_to_string(&path).ok())
        .filter_map(|text| parse_app_manifest(&text, steamapps))
        .collect()
}

/// One `appmanifest_NNN.acf`.
///
/// Steam's runtimes and Proton builds live in the same directory as the
/// games and have manifests of the same shape. They are left out: nobody
/// thinks of "Proton - Experimental" as a game, and a library page that
/// lists it looks broken.
fn parse_app_manifest(text: &str, steamapps: &Path) -> Option<Game> {
    let app_id = vdf_value(text, "appid")?;
    let name = vdf_value(text, "name")?;
    if is_steam_plumbing(&app_id, &name) {
        return None;
    }
    let install_dir = vdf_value(text, "installdir").map(|d| steamapps.join("common").join(d));
    Some(Game {
        name,
        source: Source::Steam,
        app_id: Some(app_id),
        size_on_disk: vdf_value(text, "SizeOnDisk")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        last_played: vdf_value(text, "LastPlayed").and_then(|v| v.parse().ok()),
        install_dir,
    })
}

/// Steam's own components, which share the library with the games.
fn is_steam_plumbing(app_id: &str, name: &str) -> bool {
    const TOOLS: [&str; 4] = ["228980", "1070560", "1391110", "1493710"];
    TOOLS.contains(&app_id)
        || name.starts_with("Proton")
        || name.starts_with("Steam Linux Runtime")
        || name.starts_with("SteamLinuxRuntime")
        || name == "Steamworks Common Redistributables"
}

/// The cover Steam has already downloaded for a game.
///
/// Steam keeps its library artwork under `appcache/librarycache/<appid>`.
/// Reading it means the shelf in this app shows the same covers the
/// person sees in Steam, with nothing fetched from the network and no
/// artwork shipped here — every file was put there by Steam, for this
/// user, for a game they own.
///
/// `library_600x900` is the portrait cover the shelf wants. `header` is
/// the wide banner, used only as a fallback: a 460×215 image in a portrait
/// frame is cropped hard, which still beats an empty tile.
pub fn cover_art(game: &Game) -> Option<PathBuf> {
    let app_id = game.app_id.as_ref()?;
    if !app_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    for root in steam_roots() {
        let dir = root.join("appcache/librarycache").join(app_id);
        for name in ["library_600x900.jpg", "library_600x900.png", "header.jpg"] {
            let path = dir.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Whether this game can be started from here, and with what.
///
/// Only Steam: `steam://rungameid/<appid>` is a documented URL that Steam
/// itself registers, so starting a game is handing Steam the same string
/// its own shortcuts use. Lutris and Heroic identify games by an internal
/// id this app does not read, and guessing one would start the wrong game.
pub fn launch_command(game: &Game) -> Option<(PathBuf, String)> {
    if game.source != Source::Steam {
        return None;
    }
    let app_id = game.app_id.as_ref()?;
    if app_id.is_empty() || !app_id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let steam = which("steam").or_else(|| which("xdg-open"))?;
    Some((steam, format!("steam://rungameid/{app_id}")))
}

pub fn launch(game: &Game) -> Result<(), String> {
    let (binary, url) = launch_command(game).ok_or("This game cannot be started from here")?;
    std::process::Command::new(&binary)
        .arg(&url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start {}: {e}", game.name))
}

/// The drive the games live on: the first Steam library when there is
/// one, and the home directory otherwise. What a "how full is it" gauge
/// should be measuring is the filesystem a game download would land in,
/// which on a machine with a separate games drive is not the root one.
pub fn library_root() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    steam_roots()
        .first()
        .and_then(|root| steam_libraries(root).into_iter().next())
        .filter(|path| path.is_dir())
        .unwrap_or(home)
}

/// The launcher applications installed, for the quick-actions list.
pub fn launchers() -> Vec<(&'static str, &'static str, &'static str)> {
    // command, label, icon
    [
        ("steam", "Launch Steam", "steam"),
        ("lutris", "Open Lutris", "net.lutris.Lutris"),
        ("heroic", "Open Heroic", "com.heroicgameslauncher.hgl"),
        ("protontricks", "Proton tricks", "application-x-executable"),
    ]
    .into_iter()
    .filter(|(command, ..)| which(command).is_some())
    .collect()
}

// ---- Lutris and Heroic ---------------------------------------------------

fn lutris_games() -> Vec<Game> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let dirs = [
        home.join(".config/lutris/games"),
        home.join(".var/app/net.lutris.Lutris/config/lutris/games"),
    ];
    let mut games = Vec::new();
    for dir in dirs {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = file.strip_suffix(".yml") else {
                continue;
            };
            if let Some(name) = lutris_name(stem) {
                games.push(Game {
                    name,
                    source: Source::Lutris,
                    app_id: None,
                    size_on_disk: 0,
                    last_played: None,
                    install_dir: None,
                });
            }
        }
    }
    games
}

/// Lutris names its config files `some-game-1234567890`. The trailing
/// number is an id, not part of the name, and the rest is a slug.
fn lutris_name(stem: &str) -> Option<String> {
    let slug = match stem.rsplit_once('-') {
        Some((slug, id)) if id.chars().all(|c| c.is_ascii_digit()) => slug,
        _ => stem,
    };
    if slug.is_empty() {
        return None;
    }
    Some(title_case(slug))
}

fn title_case(slug: &str) -> String {
    slug.split('-')
        .filter(|w| !w.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn heroic_games() -> Vec<Game> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let files = [
        home.join(".config/heroic/legendaryConfig/legendary/installed.json"),
        home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/legendaryConfig/legendary/installed.json"),
        home.join(".config/heroic/gog_store/installed.json"),
    ];
    let mut games = Vec::new();
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        games.extend(parse_heroic(&value));
    }
    games
}

/// Heroic writes two shapes: a map of id to entry for Epic, and an
/// `{"installed": [...]}` array for GOG.
fn parse_heroic(value: &serde_json::Value) -> Vec<Game> {
    let entries: Vec<&serde_json::Value> = if let Some(list) = value["installed"].as_array() {
        list.iter().collect()
    } else if let Some(map) = value.as_object() {
        map.values().collect()
    } else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|entry| {
            let name = entry["title"]
                .as_str()
                .or_else(|| entry["app_name"].as_str())?;
            Some(Game {
                name: name.to_string(),
                source: Source::Heroic,
                app_id: None,
                size_on_disk: entry["install_size"].as_u64().unwrap_or(0),
                last_played: None,
                install_dir: entry["install_path"].as_str().map(PathBuf::from),
            })
        })
        .collect()
}

// ---- the VDF files -------------------------------------------------------

/// The value of the first `"key" "value"` pair with this key.
///
/// Valve's KeyValues format is nested braces with quoted strings. A full
/// parser is not needed for reading a handful of flat keys out of a
/// manifest, and writing one would mean owning a parser for a format
/// nobody documents.
fn vdf_value(text: &str, key: &str) -> Option<String> {
    vdf_values(text, key).into_iter().next()
}

fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let mut values = Vec::new();
    for line in text.lines() {
        let mut parts = line.split('"').skip(1);
        let (Some(found), Some(_between), Some(value)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if found == key {
            values.push(value.to_string());
        }
    }
    values
}

// ---- launch options ------------------------------------------------------

/// The environment a game should be started in on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LaunchRecipe {
    /// Environment assignments, in the order they should be written.
    pub environment: Vec<(String, String)>,
    /// Wrapper commands, outermost first: `gamemoderun mangohud %command%`.
    pub wrappers: Vec<String>,
    /// One line per thing the recipe does, for the UI to explain itself.
    pub notes: Vec<String>,
}

impl LaunchRecipe {
    /// The string to paste into Steam's launch options.
    pub fn steam_launch_options(&self) -> String {
        let mut parts: Vec<String> = self
            .environment
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        parts.extend(self.wrappers.iter().cloned());
        parts.push("%command%".into());
        parts.join(" ")
    }

    /// The same thing for a terminal, where the game is a real command.
    pub fn shell_prefix(&self) -> String {
        let mut parts: Vec<String> = self
            .environment
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        parts.extend(self.wrappers.iter().cloned());
        parts.join(" ")
    }

    pub fn is_empty(&self) -> bool {
        self.environment.is_empty() && self.wrappers.is_empty()
    }
}

/// What a game on this machine should be started with.
///
/// Only things that apply are included. On a desktop with one card there
/// is no offload to set up and the recipe says so by being short, rather
/// than by listing variables that do nothing.
pub fn launch_recipe(gpus: &[Gpu], installed: &dyn Fn(&str) -> bool) -> LaunchRecipe {
    let mut recipe = LaunchRecipe::default();
    if is_hybrid(gpus) {
        let discrete = gpus.iter().find(|g| !g.is_integrated());
        match discrete.map(|g| g.vendor) {
            Some(Vendor::Nvidia) => {
                recipe
                    .environment
                    .push(("__NV_PRIME_RENDER_OFFLOAD".into(), "1".into()));
                recipe
                    .environment
                    .push(("__GLX_VENDOR_LIBRARY_NAME".into(), "nvidia".into()));
                recipe
                    .environment
                    .push(("__VK_LAYER_NV_optimus".into(), "NVIDIA_only".into()));
                recipe.notes.push(
                    "Runs the game on the NVIDIA card. Without this it lands on the integrated graphics, which works but is several times slower."
                        .into(),
                );
            }
            Some(_) => {
                recipe.environment.push(("DRI_PRIME".into(), "1".into()));
                recipe.notes.push(
                    "Runs the game on the discrete card rather than the integrated one.".into(),
                );
            }
            None => {}
        }
    }
    if installed("gamemode") {
        recipe.wrappers.push("gamemoderun".into());
        recipe.notes.push(
            "Puts the CPU in performance mode while the game runs, and back afterwards.".into(),
        );
    }
    if installed("mangohud") {
        recipe.wrappers.push("mangohud".into());
        recipe
            .notes
            .push("Shows frame rate and temperatures in the corner of the game.".into());
    }
    recipe
}

/// Which card a game would land on with no launch options at all.
pub fn default_render_gpu(gpus: &[Gpu]) -> Option<&Gpu> {
    crate::gpu::primary(gpus)
}

/// Steam's per-game launch options, read from the account's
/// `localconfig.vdf`, so the page can say which games are already set up.
///
/// One file per Steam account; all of them are read, because somebody with
/// two accounts has two sets of launch options and neither is the wrong one.
pub fn steam_launch_options() -> BTreeMap<String, String> {
    let mut options = BTreeMap::new();
    for root in steam_roots() {
        for entry in fs::read_dir(root.join("userdata"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let config = entry.path().join("config/localconfig.vdf");
            let Ok(text) = fs::read_to_string(&config) else {
                continue;
            };
            options.extend(parse_launch_options(&text));
        }
    }
    options
}

/// `localconfig.vdf` nests launch options under each app id:
///
/// ```text
/// "440"
/// {
///     "LaunchOptions"  "mangohud %command%"
/// }
/// ```
///
/// The app id is whichever numeric key was seen last before the option,
/// which is enough because the option always sits directly inside its app's
/// block.
fn parse_launch_options(text: &str) -> BTreeMap<String, String> {
    let mut options = BTreeMap::new();
    let mut current_app: Option<String> = None;
    for line in text.lines() {
        let quoted: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        match quoted.as_slice() {
            [key] if key.chars().all(|c| c.is_ascii_digit()) && !key.is_empty() => {
                current_app = Some(key.to_string());
            }
            [key, value] if key.eq_ignore_ascii_case("LaunchOptions") => {
                if let Some(app) = &current_app
                    && !value.trim().is_empty()
                {
                    options.insert(app.clone(), value.to_string());
                }
            }
            _ => {}
        }
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::DriverKind;

    /// `integrated` is what `gpu::discover` would have decided; the
    /// launch recipe takes it as given rather than re-deriving it.
    fn gpu(vendor: Vendor, internal: bool) -> Gpu {
        Gpu {
            address: "0000:01:00.0".into(),
            vendor,
            vendor_id: 0,
            device_id: 0,
            model: String::new(),
            driver: DriverKind::None,
            module: None,
            card: None,
            render_node: None,
            boot_vga: internal,
            drives_internal_panel: internal,
            integrated: internal,
        }
    }

    #[test]
    fn an_app_manifest_becomes_a_game() {
        let manifest = r#"
"AppState"
{
	"appid"		"489830"
	"name"		"The Elder Scrolls V: Skyrim Special Edition"
	"installdir"		"Skyrim Special Edition"
	"LastPlayed"		"1789615365"
	"SizeOnDisk"		"16092533099"
}
"#;
        let game = parse_app_manifest(manifest, Path::new("/games/steamapps")).unwrap();
        assert_eq!(game.name, "The Elder Scrolls V: Skyrim Special Edition");
        assert_eq!(game.app_id.as_deref(), Some("489830"));
        assert_eq!(game.size_on_disk, 16_092_533_099);
        assert_eq!(
            game.install_dir.unwrap(),
            Path::new("/games/steamapps/common/Skyrim Special Edition")
        );
    }

    #[test]
    fn steams_own_components_are_not_games() {
        let proton = "\"appid\" \"1493710\"\n\"name\" \"Proton - Experimental\"\n";
        assert!(parse_app_manifest(proton, Path::new("/x")).is_none());
        let runtime = "\"appid\" \"1391110\"\n\"name\" \"Steam Linux Runtime 2.0 (soldier)\"\n";
        assert!(parse_app_manifest(runtime, Path::new("/x")).is_none());
        let redist = "\"appid\" \"228980\"\n\"name\" \"Steamworks Common Redistributables\"\n";
        assert!(parse_app_manifest(redist, Path::new("/x")).is_none());
    }

    #[test]
    fn a_manifest_with_no_name_is_skipped() {
        assert!(parse_app_manifest("\"appid\" \"1\"\n", Path::new("/x")).is_none());
        assert!(parse_app_manifest("", Path::new("/x")).is_none());
    }

    #[test]
    fn every_library_path_is_found() {
        let vdf = r#"
"libraryfolders"
{
	"0"
	{
		"path"		"/home/someone/.local/share/Steam"
	}
	"1"
	{
		"path"		"/mnt/games/SteamLibrary"
	}
}
"#;
        assert_eq!(
            vdf_values(vdf, "path"),
            vec![
                "/home/someone/.local/share/Steam",
                "/mnt/games/SteamLibrary"
            ]
        );
    }

    #[test]
    fn launch_options_belong_to_the_app_above_them() {
        let config = r#"
			"apps"
			{
				"440"
				{
					"LaunchOptions"		"mangohud %command%"
				}
				"570"
				{
					"playtime"		"12"
				}
				"620"
				{
					"LaunchOptions"		"gamemoderun %command%"
				}
			}
"#;
        let options = parse_launch_options(config);
        assert_eq!(
            options.get("440").map(String::as_str),
            Some("mangohud %command%")
        );
        assert_eq!(
            options.get("620").map(String::as_str),
            Some("gamemoderun %command%")
        );
        // An app with no launch options does not get an empty entry.
        assert!(!options.contains_key("570"));
    }

    #[test]
    fn a_hybrid_nvidia_laptop_gets_the_offload_variables() {
        let gpus = vec![gpu(Vendor::Nvidia, false), gpu(Vendor::Amd, true)];
        let recipe = launch_recipe(&gpus, &|_| false);
        let options = recipe.steam_launch_options();
        assert!(options.contains("__NV_PRIME_RENDER_OFFLOAD=1"));
        assert!(options.contains("__GLX_VENDOR_LIBRARY_NAME=nvidia"));
        assert!(options.ends_with("%command%"));
        // Nothing is installed, so no wrappers.
        assert!(!options.contains("gamemoderun"));
    }

    #[test]
    fn a_hybrid_amd_laptop_gets_dri_prime() {
        let gpus = vec![gpu(Vendor::Amd, false), gpu(Vendor::Intel, true)];
        let recipe = launch_recipe(&gpus, &|_| false);
        assert_eq!(recipe.steam_launch_options(), "DRI_PRIME=1 %command%");
    }

    #[test]
    fn one_card_needs_no_offload() {
        let gpus = vec![gpu(Vendor::Amd, false)];
        let recipe = launch_recipe(&gpus, &|_| false);
        assert!(recipe.is_empty());
        assert_eq!(recipe.steam_launch_options(), "%command%");
    }

    #[test]
    fn installed_wrappers_are_added_in_the_right_order() {
        let gpus = vec![gpu(Vendor::Amd, false)];
        let recipe = launch_recipe(&gpus, &|p| matches!(p, "gamemode" | "mangohud"));
        // gamemoderun must be outside mangohud, or the overlay's process
        // is the one that gets the performance governor.
        assert_eq!(
            recipe.steam_launch_options(),
            "gamemoderun mangohud %command%"
        );
        assert_eq!(recipe.shell_prefix(), "gamemoderun mangohud");
    }

    #[test]
    fn lutris_file_names_become_titles() {
        assert_eq!(
            lutris_name("hollow-knight-1699999999").as_deref(),
            Some("Hollow Knight")
        );
        assert_eq!(lutris_name("doom").as_deref(), Some("Doom"));
        assert_eq!(lutris_name(""), None);
    }

    #[test]
    fn both_heroic_shapes_are_read() {
        let epic = serde_json::json!({
            "Fortnite": {"title": "Fortnite", "install_size": 1000u64},
        });
        let games = parse_heroic(&epic);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].name, "Fortnite");
        assert_eq!(games[0].size_on_disk, 1000);

        let gog = serde_json::json!({
            "installed": [{"title": "Cyberpunk 2077", "install_path": "/games/cp"}],
        });
        let games = parse_heroic(&gog);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].name, "Cyberpunk 2077");
    }

    #[test]
    fn only_steam_games_can_be_started_from_here() {
        let mut game = Game {
            name: "Skyrim".into(),
            source: Source::Steam,
            app_id: Some("489830".into()),
            size_on_disk: 0,
            last_played: None,
            install_dir: None,
        };
        // Whether a command comes back depends on steam or xdg-open being
        // installed, so this asserts the url rather than the presence.
        if let Some((_, url)) = launch_command(&game) {
            assert_eq!(url, "steam://rungameid/489830");
        }
        game.source = Source::Lutris;
        assert!(launch_command(&game).is_none());
        game.source = Source::Steam;
        game.app_id = None;
        assert!(launch_command(&game).is_none());
    }

    #[test]
    fn an_app_id_that_is_not_a_number_never_reaches_a_command_line() {
        // The id comes from a file on disk, so it is not trusted to be a
        // number just because Steam usually writes one.
        for bad in ["", "../../etc", "489830; rm -rf /", "abc"] {
            let game = Game {
                name: "x".into(),
                source: Source::Steam,
                app_id: Some(bad.into()),
                size_on_disk: 0,
                last_played: None,
                install_dir: None,
            };
            assert!(launch_command(&game).is_none(), "accepted {bad:?}");
            assert!(cover_art(&game).is_none(), "accepted {bad:?}");
        }
    }

    #[test]
    fn last_played_reads_as_english() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(ago(now), "in the last hour");
        assert_eq!(ago(now - 90_000), "yesterday");
        assert_eq!(ago(now - 3 * 86_400), "3 days ago");
        assert!(ago(now - 100 * 86_400).ends_with("months ago"));
    }
}
