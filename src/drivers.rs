//! Graphics drivers: what is installed, what is missing, and what to build.
//!
//! Three separate questions get confused with each other constantly, and
//! this module keeps them apart:
//!
//! 1. **Is a kernel module bound to the card?** [`crate::gpu`] answers that
//!    from `/sys`, and it is the only one of the three that is about right
//!    now rather than about packages.
//! 2. **Is the userspace half installed?** A bound `nvidia` module with no
//!    `lib32-nvidia-utils` is the single most common broken-game report
//!    there is: 64-bit games run, every 32-bit game and every Proton title
//!    dies on a missing library. Nothing in `/sys` shows it.
//! 3. **Will it still work after the next reboot?** A DKMS module is built
//!    per kernel. Install a second kernel, boot it, and the card has no
//!    driver — which looks like the driver broke, not like a module was
//!    never built. This is what the rebuild button is for.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::gpu::{DriverKind, Gpu, Vendor};

/// One kernel installed on this machine.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Kernel {
    /// `6.17.11-raven`, as `uname -r` gives it.
    pub release: String,
    /// Whether this is the one currently booted.
    pub running: bool,
}

/// One DKMS module, for one kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DkmsModule {
    pub name: String,
    pub version: String,
    pub kernel: String,
    /// `installed`, `built`, `added`, or whatever else dkms reported.
    pub state: String,
}

impl DkmsModule {
    /// Only `installed` means the module is in place for that kernel.
    /// `built` means compiled but not put where the kernel will find it.
    pub fn is_installed(&self) -> bool {
        self.state == "installed"
    }
}

/// Every kernel with modules on disk, running one first.
///
/// `/usr/lib/modules` also collects stale directories from removed kernels,
/// which have no `pkgbase` and no compressed module index. Those are not
/// kernels anyone can boot and are left out, so the rebuild list does not
/// offer to build for something that is not there.
pub fn installed_kernels() -> Vec<Kernel> {
    let running = running_kernel();
    let mut kernels: Vec<Kernel> = fs::read_dir("/usr/lib/modules")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|release| is_bootable_kernel(Path::new("/usr/lib/modules").join(release)))
        .map(|release| Kernel {
            running: release == running,
            release,
        })
        .collect();
    kernels.sort_by(|a, b| b.running.cmp(&a.running).then(a.release.cmp(&b.release)));
    kernels
}

fn is_bootable_kernel(dir: impl AsRef<Path>) -> bool {
    let dir = dir.as_ref();
    dir.join("pkgbase").exists() || dir.join("modules.dep").exists() || dir.join("kernel").is_dir()
}

pub fn running_kernel() -> String {
    // `/proc/sys/kernel/osrelease` is `uname -r` without spawning uname.
    crate::gpu::read_text("/proc/sys/kernel/osrelease").unwrap_or_default()
}

/// Whether DKMS is installed at all. A distribution kernel module that is
/// not a DKMS one has nothing here, and that is a fine state to be in.
pub fn dkms_available() -> bool {
    which("dkms").is_some()
}

/// Everything `dkms status` reports.
pub fn dkms_status() -> Vec<DkmsModule> {
    let Some(dkms) = which("dkms") else {
        return Vec::new();
    };
    let Ok(out) = Command::new(dkms).arg("status").output() else {
        return Vec::new();
    };
    parse_dkms_status(&String::from_utf8_lossy(&out.stdout))
}

/// Parses `dkms status`.
///
/// The modern format is `name/version, kernel, arch: state`; older dkms
/// wrote `name, version, kernel, arch: state`. Both are read, because a
/// rolling distribution has both in the wild and a parser that only knows
/// one silently reports "no modules" on the other — which would have this
/// app tell someone their driver is missing when it is fine.
fn parse_dkms_status(text: &str) -> Vec<DkmsModule> {
    let mut modules = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((left, state)) = line.rsplit_once(':') else {
            continue;
        };
        let fields: Vec<&str> = left.split(',').map(str::trim).collect();
        let (name, version, kernel) = match fields.as_slice() {
            // name/version, kernel, arch
            [head, kernel, _arch] => match head.split_once('/') {
                Some((name, version)) => (name, version, *kernel),
                None => continue,
            },
            // name, version, kernel, arch
            [name, version, kernel, _arch] => (*name, *version, *kernel),
            _ => continue,
        };
        modules.push(DkmsModule {
            name: name.to_string(),
            version: version.to_string(),
            kernel: kernel.to_string(),
            state: state.trim().to_string(),
        });
    }
    modules
}

/// The kernels that a DKMS module is missing from, given everything dkms
/// reported and every kernel on disk.
///
/// A module known to dkms but not installed for a kernel is the reboot that
/// comes up with no driver. This is the list the rebuild button works from.
pub fn kernels_missing_modules(modules: &[DkmsModule], kernels: &[Kernel]) -> Vec<String> {
    let names: BTreeSet<&str> = modules.iter().map(|m| m.name.as_str()).collect();
    if names.is_empty() {
        return Vec::new();
    }
    kernels
        .iter()
        .filter(|kernel| {
            names.iter().any(|name| {
                !modules
                    .iter()
                    .any(|m| m.name == *name && m.kernel == kernel.release && m.is_installed())
            })
        })
        .map(|k| k.release.clone())
        .collect()
}

// ---- what a card needs ---------------------------------------------------

/// A package this machine ought to have, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub package: String,
    /// What breaks without it, in the person's terms rather than the
    /// packager's.
    pub reason: String,
    /// Whether a game simply will not start without this, as against
    /// running but worse.
    pub essential: bool,
}

impl Requirement {
    fn new(package: &str, reason: &str, essential: bool) -> Requirement {
        Requirement {
            package: package.into(),
            reason: reason.into(),
            essential,
        }
    }
}

/// NVIDIA's open kernel modules cover Turing and everything after it.
/// Older cards need the proprietary module or they get no driver at all.
///
/// PCI device IDs are not a documented architecture map, but NVIDIA has
/// allocated them in order for twenty years and Turing starts at `0x1e00`
/// — Pascal's highest is `0x1d81`. The cost of being wrong either way is a
/// package recommendation the person can override, not a broken install.
const FIRST_TURING_DEVICE_ID: u16 = 0x1e00;

pub fn nvidia_open_supported(device_id: u16) -> bool {
    device_id >= FIRST_TURING_DEVICE_ID
}

/// The packages a card needs to run games, given what is on the system.
///
/// Only packages that are actually missing come back, so an empty list is
/// the good answer and the UI can say so without further checking.
pub fn requirements(gpus: &[Gpu], installed: &dyn Fn(&str) -> bool) -> Vec<Requirement> {
    let mut wanted: Vec<Requirement> = Vec::new();
    for gpu in gpus {
        match gpu.vendor {
            Vendor::Nvidia => {
                let module = if nvidia_open_supported(gpu.device_id) {
                    "nvidia-open-dkms"
                } else {
                    "nvidia-dkms"
                };
                // Either module package counts: somebody on the prebuilt
                // `nvidia-open` for the stock kernel is not missing a driver.
                if !installed(module) && !installed("nvidia-open") && !installed("nvidia") {
                    wanted.push(Requirement::new(
                        module,
                        "the kernel driver for your NVIDIA card, rebuilt automatically for every kernel you install",
                        true,
                    ));
                }
                wanted.push(Requirement::new(
                    "nvidia-utils",
                    "NVIDIA's OpenGL and Vulkan libraries — without these no 3D game starts",
                    true,
                ));
                wanted.push(Requirement::new(
                    "lib32-nvidia-utils",
                    "the 32-bit half of the same libraries, which every Proton and Steam Play game needs",
                    true,
                ));
            }
            Vendor::Amd => {
                wanted.push(Requirement::new(
                    "vulkan-radeon",
                    "the Vulkan driver for your AMD card",
                    true,
                ));
                wanted.push(Requirement::new(
                    "lib32-vulkan-radeon",
                    "the 32-bit Vulkan driver, which every Proton and Steam Play game needs",
                    true,
                ));
                wanted.push(Requirement::new(
                    "lib32-mesa",
                    "32-bit OpenGL for older native games",
                    false,
                ));
            }
            Vendor::Intel => {
                wanted.push(Requirement::new(
                    "vulkan-intel",
                    "the Vulkan driver for your Intel graphics",
                    true,
                ));
                wanted.push(Requirement::new(
                    "lib32-vulkan-intel",
                    "the 32-bit Vulkan driver, which every Proton and Steam Play game needs",
                    true,
                ));
            }
            Vendor::Other => {}
        }
    }
    // Everything a game needs regardless of whose card it is.
    wanted.push(Requirement::new(
        "vulkan-icd-loader",
        "the loader every Vulkan game asks for a driver through",
        true,
    ));
    wanted.push(Requirement::new(
        "lib32-vulkan-icd-loader",
        "the 32-bit Vulkan loader, for Proton",
        true,
    ));

    let mut seen = BTreeSet::new();
    wanted
        .into_iter()
        .filter(|r| seen.insert(r.package.clone()))
        .filter(|r| !installed(&r.package))
        .collect()
}

// ---- what is installed ---------------------------------------------------

/// Every installed package name, read once.
///
/// `rvn --json --no-sync list` is the supported way to ask, and it is what
/// Raven Store uses. The local database is read directly as a fallback: it
/// is pacman's own on-disk format, which rvn maintains, and reading it
/// means the check still works when rvn is mid-upgrade or absent.
pub fn installed_packages() -> BTreeSet<String> {
    if let Some(rvn) = which("rvn")
        && let Ok(out) = Command::new(rvn)
            .args(["--json", "--no-sync", "list"])
            .output()
        && out.status.success()
    {
        let names = parse_rvn_list(&String::from_utf8_lossy(&out.stdout));
        if !names.is_empty() {
            return names;
        }
    }
    local_database_packages()
}

/// Package names out of rvn's `installed` event.
fn parse_rvn_list(stdout: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value["event"] != "installed" {
            continue;
        }
        for package in value["packages"].as_array().into_iter().flatten() {
            if let Some(name) = package["name"].as_str() {
                names.insert(name.to_string());
            }
        }
    }
    names
}

/// `/var/lib/pacman/local` holds one directory per installed package,
/// named `package-version-release`. The version is split off at the last
/// two hyphens, which is the format's own rule.
fn local_database_packages() -> BTreeSet<String> {
    fs::read_dir("/var/lib/pacman/local")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| strip_package_version(&e.file_name().to_string_lossy()))
        .collect()
}

fn strip_package_version(dir: &str) -> Option<String> {
    let without_release = dir.rsplit_once('-')?.0;
    let name = without_release.rsplit_once('-')?.0;
    (!name.is_empty()).then(|| name.to_string())
}

/// Whether a process with this executable name is running.
///
/// `comm` is the name the kernel keeps, truncated to fifteen characters —
/// long enough for every name asked about here, and far cheaper than
/// reading and splitting each `cmdline`.
pub fn process_running(name: &str) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().chars().all(|c| c.is_ascii_digit()))
        {
            continue;
        }
        if let Ok(comm) = fs::read_to_string(path.join("comm"))
            && comm.trim() == name
        {
            return true;
        }
    }
    false
}

pub fn which(command: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(command))
            .find(|path| path.is_file())
    })
}

// ---- a plain-language verdict --------------------------------------------

/// What to tell someone about one card, in one line each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub headline: String,
    pub detail: String,
    pub good: bool,
}

pub fn verdict(gpu: &Gpu, missing: &[Requirement]) -> Verdict {
    let essential: Vec<&Requirement> = missing.iter().filter(|r| r.essential).collect();
    match gpu.driver {
        DriverKind::None => Verdict {
            headline: "No driver is loaded".into(),
            detail: format!(
                "Nothing is driving this {} card, so it cannot render anything. Installing its driver is the first thing to do.",
                gpu.vendor
            ),
            good: false,
        },
        DriverKind::Nouveau => Verdict {
            headline: "Running on Nouveau".into(),
            detail:
                "Nouveau is the community driver. It draws the desktop, but it cannot clock this card up, so games run at a fraction of the speed they should. NVIDIA's own driver is the fix."
                    .into(),
            good: false,
        },
        _ if !essential.is_empty() => Verdict {
            headline: "The driver is half installed".into(),
            detail: format!(
                "The kernel side is fine, but {} missing. Games that need {} will fail to start.",
                list_packages(&essential),
                if essential.len() == 1 { "it" } else { "them" }
            ),
            good: false,
        },
        _ => Verdict {
            headline: format!("Ready — {}", gpu.driver.label()),
            detail: "The kernel driver and the graphics libraries are both in place.".into(),
            good: true,
        },
    }
}

fn list_packages(missing: &[&Requirement]) -> String {
    let names: Vec<&str> = missing.iter().map(|r| r.package.as_str()).collect();
    match names.as_slice() {
        [one] => format!("{one} is"),
        [a, b] => format!("{a} and {b} are"),
        _ => format!("{} and {} more are", names[0], names.len() - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::Gpu;

    fn nvidia(device_id: u16, driver: DriverKind) -> Gpu {
        Gpu {
            address: "0000:01:00.0".into(),
            vendor: Vendor::Nvidia,
            vendor_id: 0x10de,
            device_id,
            model: "GeForce GTX 1660 Ti Mobile".into(),
            driver,
            module: None,
            card: None,
            render_node: None,
            boot_vga: false,
            drives_internal_panel: false,
            integrated: false,
        }
    }

    #[test]
    fn both_dkms_status_formats_are_read() {
        let modern = "nvidia/615.71.09, 6.17.11-raven, x86_64: installed";
        let older = "nvidia, 615.71.09, 6.17.11-raven, x86_64: installed";
        for text in [modern, older] {
            let parsed = parse_dkms_status(text);
            assert_eq!(parsed.len(), 1, "failed on {text}");
            assert_eq!(parsed[0].name, "nvidia");
            assert_eq!(parsed[0].version, "615.71.09");
            assert_eq!(parsed[0].kernel, "6.17.11-raven");
            assert!(parsed[0].is_installed());
        }
    }

    #[test]
    fn a_built_but_uninstalled_module_is_not_installed() {
        let parsed = parse_dkms_status("v4l2loopback/0.15.1, 6.17.11-raven, x86_64: built");
        assert!(!parsed[0].is_installed());
    }

    #[test]
    fn dkms_noise_is_skipped() {
        let parsed =
            parse_dkms_status("\n\nnot a status line\nnvidia/1.0, 6.1.0, x86_64: installed\n");
        assert_eq!(parsed.len(), 1);
    }

    #[test]
    fn a_kernel_with_no_module_built_is_reported() {
        let modules = parse_dkms_status("nvidia/615.71.09, 6.17.11-raven, x86_64: installed");
        let kernels = vec![
            Kernel {
                release: "6.17.11-raven".into(),
                running: true,
            },
            Kernel {
                release: "7.2.6-arch2-1".into(),
                running: false,
            },
        ];
        assert_eq!(
            kernels_missing_modules(&modules, &kernels),
            vec!["7.2.6-arch2-1"]
        );
    }

    #[test]
    fn nothing_is_missing_when_every_kernel_has_it() {
        let modules = parse_dkms_status(
            "nvidia/1.0, 6.17.11-raven, x86_64: installed\nnvidia/1.0, 7.2.6-arch2-1, x86_64: installed",
        );
        let kernels = vec![
            Kernel {
                release: "6.17.11-raven".into(),
                running: true,
            },
            Kernel {
                release: "7.2.6-arch2-1".into(),
                running: false,
            },
        ];
        assert!(kernels_missing_modules(&modules, &kernels).is_empty());
    }

    #[test]
    fn no_dkms_modules_means_nothing_to_rebuild() {
        let kernels = vec![Kernel {
            release: "6.17.11-raven".into(),
            running: true,
        }];
        assert!(kernels_missing_modules(&[], &kernels).is_empty());
    }

    #[test]
    fn turing_and_later_can_use_the_open_modules() {
        // TU116M, the card this was written on.
        assert!(nvidia_open_supported(0x2191));
        // GP106, a Pascal card.
        assert!(!nvidia_open_supported(0x1c03));
    }

    #[test]
    fn a_bound_driver_with_no_32_bit_libraries_is_still_broken() {
        let gpus = vec![nvidia(0x2191, DriverKind::NvidiaProprietary)];
        let installed = |p: &str| p != "lib32-nvidia-utils";
        let missing = requirements(&gpus, &installed);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].package, "lib32-nvidia-utils");
        assert!(missing[0].essential);
        let v = verdict(&gpus[0], &missing);
        assert!(!v.good);
        assert!(v.detail.contains("lib32-nvidia-utils"));
    }

    #[test]
    fn a_fully_installed_card_is_ready() {
        let gpus = vec![nvidia(0x2191, DriverKind::NvidiaProprietary)];
        let missing = requirements(&gpus, &|_| true);
        assert!(missing.is_empty());
        assert!(verdict(&gpus[0], &missing).good);
    }

    #[test]
    fn the_prebuilt_nvidia_package_counts_as_a_driver() {
        let gpus = vec![nvidia(0x2191, DriverKind::NvidiaProprietary)];
        let installed = |p: &str| p == "nvidia-open";
        let missing = requirements(&gpus, &installed);
        assert!(
            !missing.iter().any(|r| r.package.contains("dkms")),
            "should not ask for a DKMS package when a prebuilt one is installed"
        );
    }

    #[test]
    fn nouveau_is_called_out_whatever_is_installed() {
        let gpus = vec![nvidia(0x2191, DriverKind::Nouveau)];
        let v = verdict(&gpus[0], &requirements(&gpus, &|_| true));
        assert!(!v.good);
        assert!(v.headline.contains("Nouveau"));
    }

    #[test]
    fn a_card_with_nothing_bound_is_the_loudest_case() {
        let gpus = [nvidia(0x2191, DriverKind::None)];
        let v = verdict(&gpus[0], &[]);
        assert!(!v.good);
        assert!(v.headline.contains("No driver"));
    }

    #[test]
    fn requirements_do_not_repeat_across_two_cards() {
        let gpus = vec![
            nvidia(0x2191, DriverKind::NvidiaProprietary),
            Gpu {
                vendor: Vendor::Amd,
                vendor_id: 0x1002,
                device_id: 0x15d8,
                ..nvidia(0x15d8, DriverKind::Amdgpu)
            },
        ];
        let missing = requirements(&gpus, &|_| false);
        let mut names: Vec<&str> = missing.iter().map(|r| r.package.as_str()).collect();
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "a package was recommended twice");
        assert!(names.contains(&"lib32-vulkan-radeon"));
        assert!(names.contains(&"lib32-nvidia-utils"));
    }

    #[test]
    fn package_directories_give_up_their_names() {
        assert_eq!(
            strip_package_version("nvidia-open-dkms-615.71.09-1").as_deref(),
            Some("nvidia-open-dkms")
        );
        assert_eq!(
            strip_package_version("mesa-1:26.2.2-1").as_deref(),
            Some("mesa")
        );
        assert_eq!(strip_package_version("broken"), None);
    }

    #[test]
    fn rvn_list_events_give_up_their_packages() {
        let stdout = r#"{"event":"other"}
{"event":"installed","packages":[{"name":"mesa","version":"1"},{"name":"steam"}]}
not json
"#;
        let names = parse_rvn_list(stdout);
        assert!(names.contains("mesa"));
        assert!(names.contains("steam"));
        assert_eq!(names.len(), 2);
    }
}
