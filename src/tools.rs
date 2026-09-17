//! Game tools: the things that go around a game rather than in it, and
//! the Proton prefixes they end up operating on.
//!
//! # Two halves
//!
//! The first is a catalogue, like the emulators page: the overlays,
//! wrappers and diagnostics, what each one is actually for, and whether it
//! is installed. Most of these are one package and one sentence.
//!
//! The second is the part that is about this machine rather than about
//! software in general: the Proton builds Steam has, and the prefix each
//! game keeps its Windows-side files in. A prefix is where a game's
//! settings, saves and installed runtimes live, and "delete the prefix and
//! let it rebuild" is the oldest fix in Windows-on-Linux — so it is worth
//! being able to see them, find them, and get rid of one on purpose rather
//! than by pasting a path into a terminal.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::drivers::which;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Performance,
    Overlay,
    Compatibility,
    Diagnostics,
}

impl Kind {
    pub fn title(self) -> &'static str {
        match self {
            Kind::Performance => "Performance",
            Kind::Overlay => "Overlays and tuning",
            Kind::Compatibility => "Windows compatibility",
            Kind::Diagnostics => "Diagnostics",
        }
    }

    pub fn lede(self) -> &'static str {
        match self {
            Kind::Performance => {
                "Wrappers a game is started through, which change how the system treats it while it runs."
            }
            Kind::Overlay => "Things drawn over a game, or that change how it is drawn.",
            Kind::Compatibility => {
                "What runs Windows games, and what fixes them when they will not run."
            }
            Kind::Diagnostics => "Small programs that answer \"is the driver actually working\".",
        }
    }

    pub fn tint(self) -> &'static str {
        match self {
            Kind::Performance => "orange",
            Kind::Overlay => "purple",
            Kind::Compatibility => "blue",
            Kind::Diagnostics => "teal",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub name: &'static str,
    pub package: &'static str,
    /// What to run to check for it and, where it makes sense, to start it.
    pub binary: &'static str,
    pub kind: Kind,
    pub what: &'static str,
    /// True when running it from here makes sense. A library does not.
    pub launchable: bool,
}

impl Tool {
    /// Installed, judged by the binary when there is one and by the
    /// package database when there is not — `lib32-mangohud` ships a
    /// library and no program, so `which` would always say no.
    pub fn is_installed(&self, packages: &dyn Fn(&str) -> bool) -> bool {
        if self.binary.is_empty() {
            packages(self.package)
        } else {
            which(self.binary).is_some() || packages(self.package)
        }
    }
}

pub const CATALOGUE: [Tool; 16] = [
    Tool {
        name: "GameMode",
        package: "gamemode",
        binary: "gamemoderun",
        kind: Kind::Performance,
        what: "Puts the processor in performance mode while a game runs and back afterwards, so a game does not have to wait for the governor to notice it.",
        launchable: false,
    },
    Tool {
        name: "GameMode, 32-bit",
        package: "lib32-gamemode",
        binary: "",
        kind: Kind::Performance,
        what: "The same, for 32-bit and Proton games, which cannot load the 64-bit library.",
        launchable: false,
    },
    Tool {
        name: "Gamescope",
        package: "gamescope",
        binary: "gamescope",
        kind: Kind::Performance,
        what: "Runs a game inside its own tiny compositor, so its resolution, refresh rate and scaling are its own and not the desktop's.",
        launchable: false,
    },
    Tool {
        name: "MangoHud",
        package: "mangohud",
        binary: "mangohud",
        kind: Kind::Overlay,
        what: "Frame rate, frame times, temperatures and clocks drawn in the corner of the game itself.",
        launchable: false,
    },
    Tool {
        name: "MangoHud, 32-bit",
        package: "lib32-mangohud",
        binary: "",
        kind: Kind::Overlay,
        what: "The same overlay inside 32-bit and Proton games.",
        launchable: false,
    },
    Tool {
        name: "GOverlay",
        package: "goverlay",
        binary: "goverlay",
        kind: Kind::Overlay,
        what: "A window for configuring MangoHud and vkBasalt, instead of editing their config files.",
        launchable: true,
    },
    Tool {
        name: "vkBasalt",
        package: "vkbasalt",
        binary: "",
        kind: Kind::Overlay,
        what: "Post-processing — sharpening, colour — applied to any Vulkan game as it is drawn.",
        launchable: false,
    },
    Tool {
        name: "CoreCtrl",
        package: "corectrl",
        binary: "corectrl",
        kind: Kind::Overlay,
        what: "Fan curves, clocks and voltages for AMD cards, with a profile per game.",
        launchable: true,
    },
    Tool {
        name: "Steam",
        package: "steam",
        binary: "steam",
        kind: Kind::Compatibility,
        what: "Valve's client, and Proton with it — the translation layer that runs Windows games.",
        launchable: true,
    },
    Tool {
        name: "Lutris",
        package: "lutris",
        binary: "lutris",
        kind: Kind::Compatibility,
        what: "Installs and manages games from GOG, Epic, Battle.net and elsewhere, each in its own prefix.",
        launchable: true,
    },
    Tool {
        name: "Heroic",
        package: "heroic-games-launcher",
        binary: "heroic",
        kind: Kind::Compatibility,
        what: "A launcher for Epic, GOG and Amazon libraries.",
        launchable: true,
    },
    Tool {
        name: "Protontricks",
        package: "protontricks",
        binary: "protontricks",
        kind: Kind::Compatibility,
        what: "Runs winetricks against a Steam game's own prefix, which is how a missing Windows runtime gets installed into it.",
        launchable: true,
    },
    Tool {
        name: "Winetricks",
        package: "winetricks",
        binary: "winetricks",
        kind: Kind::Compatibility,
        what: "The same for prefixes that are not Steam's.",
        launchable: true,
    },
    Tool {
        name: "Vulkan tools",
        package: "vulkan-tools",
        binary: "vulkaninfo",
        kind: Kind::Diagnostics,
        what: "`vkcube` draws a spinning cube and `vulkaninfo` prints what the driver can do. Between them they settle whether Vulkan works.",
        launchable: false,
    },
    Tool {
        name: "Mesa utilities",
        package: "mesa-utils",
        binary: "glxinfo",
        kind: Kind::Diagnostics,
        what: "`glxinfo` and `glxgears`, the same question for OpenGL.",
        launchable: false,
    },
    Tool {
        name: "inxi",
        package: "inxi",
        binary: "inxi",
        kind: Kind::Diagnostics,
        what: "Prints a summary of the whole machine, which is what a support forum asks for first.",
        launchable: false,
    },
];

pub const KINDS: [Kind; 4] = [
    Kind::Performance,
    Kind::Overlay,
    Kind::Compatibility,
    Kind::Diagnostics,
];

pub fn by_kind(kind: Kind) -> Vec<&'static Tool> {
    CATALOGUE.iter().filter(|t| t.kind == kind).collect()
}

// ---- diagnostics ---------------------------------------------------------

/// A diagnostic that can be run and read here rather than in a terminal.
#[derive(Debug, Clone, Copy)]
pub struct Probe {
    pub name: &'static str,
    pub binary: &'static str,
    pub args: &'static [&'static str],
    pub what: &'static str,
}

pub const PROBES: [Probe; 3] = [
    Probe {
        name: "Vulkan summary",
        binary: "vulkaninfo",
        args: &["--summary"],
        what: "Which Vulkan drivers the loader found, and what each one is.",
    },
    Probe {
        name: "OpenGL renderer",
        binary: "glxinfo",
        args: &["-B"],
        what: "Which card OpenGL is rendering on, and with what driver.",
    },
    Probe {
        name: "Graphics summary",
        binary: "inxi",
        args: &["-Gxx"],
        what: "The cards, their drivers and the display, in one paragraph.",
    },
];

impl Probe {
    pub fn is_available(&self) -> bool {
        which(self.binary).is_some()
    }

    /// Runs it and returns what it said. Read-only, every one of them.
    pub fn run(&self) -> Result<String, String> {
        let binary = which(self.binary).ok_or(format!("{} is not installed", self.binary))?;
        let output = std::process::Command::new(binary)
            .args(self.args)
            .output()
            .map_err(|e| format!("Could not run {}: {e}", self.binary))?;
        let text = String::from_utf8_lossy(&output.stdout);
        if text.trim().is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("It printed nothing")
                .trim()
                .to_string());
        }
        Ok(text.into_owned())
    }
}

// ---- Proton --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtonBuild {
    pub name: String,
    /// Valve's own, as against one installed by hand into
    /// `compatibilitytools.d`.
    pub official: bool,
    pub path: PathBuf,
}

/// Every Proton build Steam can use.
pub fn proton_builds() -> Vec<ProtonBuild> {
    let mut builds = Vec::new();
    for root in crate::games::steam_roots() {
        for (dir, official) in [
            (root.join("steamapps/common"), true),
            (root.join("compatibilitytools.d"), false),
        ] {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                // In `common` only the Proton directories are builds; the
                // rest of it is games.
                if official && !name.starts_with("Proton") {
                    continue;
                }
                if builds.iter().any(|b: &ProtonBuild| b.name == name) {
                    continue;
                }
                builds.push(ProtonBuild {
                    name,
                    official,
                    path,
                });
            }
        }
    }
    builds.sort_by(|a, b| a.official.cmp(&b.official).then(a.name.cmp(&b.name)));
    builds
}

/// One game's Windows-side world: its C: drive, registry and saves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefix {
    pub app_id: String,
    /// The game's name, when the library knew it.
    pub game: Option<String>,
    pub path: PathBuf,
    pub bytes: u64,
}

impl Prefix {
    pub fn title(&self) -> String {
        match &self.game {
            Some(name) => name.clone(),
            None => format!("App {}", self.app_id),
        }
    }
}

/// Every Proton prefix, largest first.
///
/// `names` maps a Steam app id to a game name so a prefix can be shown as
/// the game it belongs to rather than as a number. An id with no name is a
/// game that has since been uninstalled, and its prefix is exactly the
/// kind of thing worth finding here.
pub fn prefixes(names: &BTreeMap<String, String>) -> Vec<Prefix> {
    let mut found = Vec::new();
    for root in crate::games::steam_roots() {
        let dir = root.join("steamapps/compatdata");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let app_id = entry.file_name().to_string_lossy().into_owned();
            if !path.is_dir() || !app_id.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if found.iter().any(|p: &Prefix| p.app_id == app_id) {
                continue;
            }
            found.push(Prefix {
                bytes: directory_size(&path, 0),
                game: names.get(&app_id).cloned(),
                app_id,
                path,
            });
        }
    }
    found.sort_by_key(|prefix| std::cmp::Reverse(prefix.bytes));
    found
}

/// Bytes under a directory, depth-limited so a link loop inside a prefix
/// cannot hang the window.
fn directory_size(dir: &Path, depth: u32) -> u64 {
    if depth > 6 {
        return 0;
    }
    std::fs::read_dir(dir)
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

/// Whether a path is a Proton prefix this app may delete.
///
/// Belt and braces before an irreversible delete: it has to be inside a
/// Steam library's `compatdata`, be named as an app id, and actually
/// contain a prefix. A bug that passed the wrong path in gets refused
/// rather than removing somebody's home directory.
pub fn is_deletable_prefix(path: &Path) -> bool {
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    let named_by_id = path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().chars().all(|c| c.is_ascii_digit()));
    let under_compatdata = path.parent().is_some_and(|p| p.ends_with("compatdata"));
    let in_a_library = crate::games::steam_roots().iter().any(|root| {
        root.join("steamapps/compatdata")
            .canonicalize()
            .is_ok_and(|dir| path.starts_with(dir))
    });
    let looks_like_a_prefix = path.join("pfx").is_dir() || path.join("pfx.lock").exists();
    named_by_id && under_compatdata && in_a_library && looks_like_a_prefix
}

/// Opens protontricks against one game's prefix.
pub fn protontricks(app_id: &str) -> Result<(), String> {
    if !app_id.chars().all(|c| c.is_ascii_digit()) || app_id.is_empty() {
        return Err(format!("Not a Steam app id: {app_id}"));
    }
    let binary = which("protontricks").ok_or("protontricks is not installed")?;
    std::process::Command::new(binary)
        .arg("--gui")
        .arg(app_id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start protontricks: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_is_shelved_exactly_once() {
        let shelved: usize = KINDS.iter().map(|k| by_kind(*k).len()).sum();
        assert_eq!(shelved, CATALOGUE.len());
        for kind in KINDS {
            assert!(!by_kind(kind).is_empty(), "{} is empty", kind.title());
        }
    }

    #[test]
    fn no_package_is_listed_twice() {
        let mut packages: Vec<&str> = CATALOGUE.iter().map(|t| t.package).collect();
        let before = packages.len();
        packages.sort_unstable();
        packages.dedup();
        assert_eq!(packages.len(), before);
    }

    #[test]
    fn every_package_name_would_survive_rvn() {
        for tool in CATALOGUE {
            assert!(
                tool.package
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+')),
                "{} would be refused",
                tool.package
            );
            assert!(!tool.what.is_empty(), "{} says nothing", tool.name);
        }
    }

    #[test]
    fn a_library_with_no_program_is_found_by_its_package() {
        // lib32-mangohud installs no binary, so `which` can never see it
        // and the package database is the only witness.
        let library = CATALOGUE
            .iter()
            .find(|t| t.package == "lib32-mangohud")
            .unwrap();
        assert!(library.binary.is_empty());
        assert!(library.is_installed(&|p| p == "lib32-mangohud"));
        assert!(!library.is_installed(&|_| false));
    }

    #[test]
    fn a_program_counts_as_installed_either_way() {
        let steam = CATALOGUE.iter().find(|t| t.name == "Steam").unwrap();
        // Present in the database but not on PATH still counts: a package
        // that installs somewhere unusual is installed.
        assert!(steam.is_installed(&|p| p == "steam"));
    }

    #[test]
    fn only_read_only_probes_are_offered() {
        // Every probe must be something that prints and exits. A tool that
        // changes anything has no business on a diagnostics button.
        for probe in PROBES {
            assert!(!probe.binary.is_empty());
            for arg in probe.args {
                assert!(
                    arg.starts_with('-'),
                    "{} takes a non-flag argument: {arg}",
                    probe.name
                );
            }
        }
    }

    #[test]
    fn a_prefix_shows_the_game_it_belongs_to() {
        let mut names = BTreeMap::new();
        names.insert("489830".to_string(), "Skyrim Special Edition".to_string());
        let known = Prefix {
            app_id: "489830".into(),
            game: names.get("489830").cloned(),
            path: PathBuf::from("/x"),
            bytes: 0,
        };
        assert_eq!(known.title(), "Skyrim Special Edition");
        // An id with no name is a prefix left behind by a game that has
        // been uninstalled, which is worth finding rather than hiding.
        let orphan = Prefix {
            app_id: "12345".into(),
            game: None,
            path: PathBuf::from("/x"),
            bytes: 0,
        };
        assert_eq!(orphan.title(), "App 12345");
    }

    #[test]
    fn nothing_outside_a_steam_library_can_be_deleted() {
        for path in ["/", "/home", "/tmp", "/etc", "/definitely/not/here/4a9f"] {
            assert!(
                !is_deletable_prefix(Path::new(path)),
                "would have deleted {path}"
            );
        }
        if let Some(home) = std::env::var_os("HOME") {
            assert!(!is_deletable_prefix(Path::new(&home)));
        }
    }

    #[test]
    fn a_real_prefix_on_this_machine_passes_every_guard() {
        // Only asserts when there is one; on a machine with no Steam this
        // is vacuous rather than wrong.
        for prefix in prefixes(&BTreeMap::new()) {
            assert!(
                is_deletable_prefix(&prefix.path),
                "{} is a prefix but the guard refuses it",
                prefix.path.display()
            );
        }
    }

    #[test]
    fn protontricks_refuses_anything_that_is_not_an_app_id() {
        for bad in ["", "abc", "../../etc", "489830; rm -rf /"] {
            assert!(protontricks(bad).is_err(), "accepted {bad:?}");
        }
    }
}
