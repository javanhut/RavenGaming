//! Emulators: what runs what, which are installed, and what they need
//! from the system.
//!
//! # A catalogue, not a store
//!
//! This is a fixed table. It does not search a repository or fetch a list
//! from anywhere — it names the emulators that are the usual answer for
//! each console, says what each one runs, and says plainly where a BIOS
//! image is required, because that is the single most common reason a
//! freshly installed emulator does nothing.
//!
//! Nothing here downloads a BIOS or a game. Where one is needed the entry
//! says so and stops; dumping it from hardware somebody owns is their
//! business and not something an installer can do for them.

use crate::drivers::which;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Family {
    Nintendo,
    Sony,
    Sega,
    Microsoft,
    Computer,
    Multi,
}

impl Family {
    pub fn title(self) -> &'static str {
        match self {
            Family::Nintendo => "Nintendo",
            Family::Sony => "PlayStation",
            Family::Sega => "Sega",
            Family::Microsoft => "Xbox",
            Family::Computer => "Computers and DOS",
            Family::Multi => "Everything at once",
        }
    }

    /// The tint of the icon tile, so the shelves read apart at a glance.
    pub fn tint(self) -> &'static str {
        match self {
            Family::Nintendo => "red",
            Family::Sony => "blue",
            Family::Sega => "indigo",
            Family::Microsoft => "green",
            Family::Computer => "orange",
            Family::Multi => "purple",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Emulator {
    pub name: &'static str,
    /// What to install it with.
    pub package: &'static str,
    /// What to run, when it is not the package name.
    pub binary: &'static str,
    pub family: Family,
    /// The machines it emulates.
    pub systems: &'static str,
    /// Anything that stops it working out of the box.
    pub caveat: Option<&'static str>,
}

impl Emulator {
    pub fn is_installed(&self) -> bool {
        which(self.binary).is_some()
    }
}

/// The catalogue, grouped by family in the order the page lists them.
pub const CATALOGUE: [Emulator; 16] = [
    Emulator {
        name: "RetroArch",
        package: "retroarch",
        binary: "retroarch",
        family: Family::Multi,
        systems: "Almost everything up to the sixth generation, through downloadable cores",
        caveat: Some(
            "Install cores from inside RetroArch before it can run anything — it ships with none.",
        ),
    },
    Emulator {
        name: "Ares",
        package: "ares",
        binary: "ares",
        family: Family::Multi,
        systems: "Nintendo, Sega and NEC consoles, with accuracy as the priority",
        caveat: None,
    },
    Emulator {
        name: "Dolphin",
        package: "dolphin-emu",
        binary: "dolphin-emu",
        family: Family::Nintendo,
        systems: "GameCube and Wii",
        caveat: None,
    },
    Emulator {
        name: "melonDS",
        package: "melonds",
        binary: "melonDS",
        family: Family::Nintendo,
        systems: "Nintendo DS",
        caveat: Some("Needs the DS BIOS and firmware dumped from a console you own."),
    },
    Emulator {
        name: "mGBA",
        package: "mgba-qt",
        binary: "mgba-qt",
        family: Family::Nintendo,
        systems: "Game Boy, Game Boy Color and Game Boy Advance",
        caveat: None,
    },
    Emulator {
        name: "Snes9x",
        package: "snes9x-gtk",
        binary: "snes9x-gtk",
        family: Family::Nintendo,
        systems: "Super Nintendo",
        caveat: None,
    },
    Emulator {
        name: "Mupen64Plus",
        package: "mupen64plus",
        binary: "mupen64plus",
        family: Family::Nintendo,
        systems: "Nintendo 64",
        caveat: None,
    },
    Emulator {
        name: "Ryujinx",
        package: "ryujinx",
        binary: "Ryujinx",
        family: Family::Nintendo,
        systems: "Nintendo Switch",
        caveat: Some("Needs keys and firmware dumped from a console you own."),
    },
    Emulator {
        name: "DuckStation",
        package: "duckstation",
        binary: "duckstation-qt",
        family: Family::Sony,
        systems: "PlayStation 1",
        caveat: Some("Needs a PS1 BIOS image dumped from a console you own."),
    },
    Emulator {
        name: "PCSX2",
        package: "pcsx2",
        binary: "pcsx2-qt",
        family: Family::Sony,
        systems: "PlayStation 2",
        caveat: Some("Needs a PS2 BIOS image dumped from a console you own."),
    },
    Emulator {
        name: "RPCS3",
        package: "rpcs3",
        binary: "rpcs3",
        family: Family::Sony,
        systems: "PlayStation 3",
        caveat: Some("Needs the PS3 firmware, which Sony publishes, installed from inside RPCS3."),
    },
    Emulator {
        name: "PPSSPP",
        package: "ppsspp",
        binary: "PPSSPPSDL",
        family: Family::Sony,
        systems: "PlayStation Portable",
        caveat: None,
    },
    Emulator {
        name: "Flycast",
        package: "flycast",
        binary: "flycast",
        family: Family::Sega,
        systems: "Dreamcast, Naomi and Atomiswave",
        caveat: Some("Needs the Dreamcast BIOS for some games."),
    },
    Emulator {
        name: "xemu",
        package: "xemu",
        binary: "xemu",
        family: Family::Microsoft,
        systems: "Original Xbox",
        caveat: Some("Needs a BIOS and a hard disk image from a console you own."),
    },
    Emulator {
        name: "ScummVM",
        package: "scummvm",
        binary: "scummvm",
        family: Family::Computer,
        systems: "Point-and-click adventures, from LucasArts to Sierra",
        caveat: None,
    },
    Emulator {
        name: "DOSBox Staging",
        package: "dosbox-staging",
        binary: "dosbox",
        family: Family::Computer,
        systems: "MS-DOS games",
        caveat: None,
    },
];

pub fn installed() -> Vec<&'static Emulator> {
    CATALOGUE.iter().filter(|e| e.is_installed()).collect()
}

pub fn by_family(family: Family) -> Vec<&'static Emulator> {
    CATALOGUE.iter().filter(|e| e.family == family).collect()
}

pub const FAMILIES: [Family; 6] = [
    Family::Multi,
    Family::Nintendo,
    Family::Sony,
    Family::Sega,
    Family::Microsoft,
    Family::Computer,
];

/// What an emulator needs from the machine, over and above being
/// installed.
///
/// These are the three that account for most "it installed and then did
/// nothing" reports, and all three are answered elsewhere in this app —
/// so the page says which one is missing and points at the page that
/// fixes it, rather than duplicating the fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prerequisite {
    pub title: &'static str,
    pub detail: String,
    pub met: bool,
    /// The page that deals with it.
    pub page: &'static str,
}

pub fn prerequisites(gpus: &[crate::gpu::Gpu], controllers: usize) -> Vec<Prerequisite> {
    let vulkan: Vec<_> = crate::gpu::vulkan_icds()
        .into_iter()
        .filter(|icd| icd.library_present && gpus.iter().any(|g| g.vendor == icd.vendor))
        .collect();
    vec![
        Prerequisite {
            title: "A Vulkan driver",
            detail: if vulkan.is_empty() {
                "The newer emulators — PCSX2, RPCS3, Dolphin, DuckStation — render through Vulkan and will not start without a driver for it.".into()
            } else {
                "The newer emulators render through Vulkan, and a driver for this machine's graphics is installed.".into()
            },
            met: !vulkan.is_empty(),
            page: "graphics",
        },
        Prerequisite {
            title: "A controller",
            detail: match controllers {
                0 => "Nothing is plugged in. Console games were made for a pad, and most emulators need one mapped before they will accept any input at all.".into(),
                1 => "One controller is connected.".into(),
                n => format!("{n} controllers are connected."),
            },
            met: controllers > 0,
            page: "controllers",
        },
        Prerequisite {
            title: "32-bit libraries",
            detail: if crate::gpu::has_32bit_stack() {
                "In place, which the older emulators and their cores are built against.".into()
            } else {
                "Missing. Several older emulators, and a number of RetroArch cores, are 32-bit builds.".into()
            },
            met: crate::gpu::has_32bit_stack(),
            page: "graphics",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_has_something_in_it() {
        for family in FAMILIES {
            assert!(
                !by_family(family).is_empty(),
                "{} has no emulators",
                family.title()
            );
        }
    }

    #[test]
    fn the_catalogue_is_all_accounted_for() {
        // Every entry appears under exactly one family shelf, so none can
        // be listed twice or vanish from the page.
        let shelved: usize = FAMILIES.iter().map(|f| by_family(*f).len()).sum();
        assert_eq!(shelved, CATALOGUE.len());
    }

    #[test]
    fn no_two_entries_share_a_package_or_a_name() {
        let mut packages: Vec<&str> = CATALOGUE.iter().map(|e| e.package).collect();
        let before = packages.len();
        packages.sort_unstable();
        packages.dedup();
        assert_eq!(packages.len(), before, "a package is listed twice");
        let mut names: Vec<&str> = CATALOGUE.iter().map(|e| e.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before);
    }

    #[test]
    fn every_entry_is_installable_and_runnable() {
        for emulator in CATALOGUE {
            assert!(
                !emulator.package.is_empty(),
                "{} has no package",
                emulator.name
            );
            assert!(
                !emulator.binary.is_empty(),
                "{} has no binary",
                emulator.name
            );
            assert!(
                !emulator.systems.is_empty(),
                "{} says nothing about what it runs",
                emulator.name
            );
            // A package name that would be refused before it reached rvn
            // is a bug in this table, not in the person's typing.
            assert!(
                emulator
                    .package
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+')),
                "{} has a package name rvn would refuse",
                emulator.package
            );
        }
    }

    #[test]
    fn the_consoles_that_need_a_bios_say_so() {
        // The single most common reason a freshly installed emulator does
        // nothing. If these lose their caveat the page stops being honest.
        for name in ["PCSX2", "DuckStation", "xemu", "melonDS", "Ryujinx"] {
            let entry = CATALOGUE.iter().find(|e| e.name == name).unwrap();
            let caveat = entry.caveat.expect("should carry a caveat");
            assert!(
                caveat.contains("BIOS") || caveat.contains("firmware") || caveat.contains("keys"),
                "{name}'s caveat does not mention what is needed"
            );
        }
    }

    #[test]
    fn nothing_in_the_catalogue_offers_to_supply_a_bios() {
        for emulator in CATALOGUE {
            let caveat = emulator.caveat.unwrap_or_default().to_lowercase();
            assert!(
                !caveat.contains("download") || caveat.contains("sony publishes"),
                "{} implies this app can fetch a BIOS",
                emulator.name
            );
        }
    }

    #[test]
    fn prerequisites_point_at_a_page_that_exists() {
        let checks = prerequisites(&[], 0);
        assert_eq!(checks.len(), 3);
        for check in &checks {
            assert!(matches!(check.page, "graphics" | "controllers"));
            assert!(!check.detail.is_empty());
        }
        // With no cards and no pads, nothing is satisfied except possibly
        // the 32-bit stack, which is about this machine rather than input.
        assert!(!checks[0].met);
        assert!(!checks[1].met);
    }

    #[test]
    fn a_connected_controller_reads_as_english() {
        assert!(prerequisites(&[], 1)[1].detail.contains("One controller"));
        assert!(prerequisites(&[], 3)[1].detail.contains("3 controllers"));
    }
}
