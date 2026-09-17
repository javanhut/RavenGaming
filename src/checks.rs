//! The readiness list: everything that has to be true before a game runs
//! well, each one reduced to a sentence and, where there is one, a fix.
//!
//! This is the module the Overview page is made of, and the one the "Fix
//! what is missing" button works from. Keeping it apart from the UI means
//! the rule for whether something is wrong is written once, tested, and
//! not re-derived by whichever page happens to be drawing it.
//!
//! A check never guesses. Where the state cannot be established —
//! `vulkaninfo` not installed, so whether Vulkan works is genuinely
//! unknown — the answer is [`State::Unknown`] and the UI says it does not
//! know, rather than reporting a pass that has not been observed.

use std::fmt;

use crate::drivers::{self, Requirement};
use crate::gpu::{self, DriverKind, Gpu};
use crate::tune::{self, Tweak};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    /// Nothing to do.
    Good,
    /// Works, but something is being left on the table.
    Advisory,
    /// A game will not run, or will run badly enough to notice.
    Problem,
    /// Could not be established.
    Unknown,
}

impl State {
    pub fn icon(self) -> &'static str {
        match self {
            State::Good => "emblem-ok-symbolic",
            State::Advisory => "dialog-information-symbolic",
            State::Problem => "dialog-warning-symbolic",
            State::Unknown => "dialog-question-symbolic",
        }
    }

    /// The style class the row's icon takes.
    pub fn css_class(self) -> &'static str {
        match self {
            State::Good => "success",
            State::Advisory => "accent-text",
            State::Problem => "warning",
            State::Unknown => "dim",
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            State::Good => "Ready",
            State::Advisory => "Could be better",
            State::Problem => "Needs attention",
            State::Unknown => "Unknown",
        })
    }
}

/// Something the app can do about a failing check, without the person
/// having to know what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Install these packages, through rvn.
    Install(Vec<String>),
    /// Build DKMS modules for these kernels.
    BuildModules(Vec<String>),
    /// Apply the system settings.
    ApplyTweaks,
    /// Nothing this app can do; the text says what the person can.
    Manual(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub title: String,
    pub detail: String,
    pub state: State,
    pub fix: Option<Fix>,
}

impl Check {
    fn good(title: impl Into<String>, detail: impl Into<String>) -> Check {
        Check {
            title: title.into(),
            detail: detail.into(),
            state: State::Good,
            fix: None,
        }
    }

    fn problem(title: impl Into<String>, detail: impl Into<String>, fix: Option<Fix>) -> Check {
        Check {
            title: title.into(),
            detail: detail.into(),
            state: State::Problem,
            fix,
        }
    }

    fn advisory(title: impl Into<String>, detail: impl Into<String>, fix: Option<Fix>) -> Check {
        Check {
            title: title.into(),
            detail: detail.into(),
            state: State::Advisory,
            fix,
        }
    }
}

/// Everything read once, so a page redraw does not re-run `dkms status`
/// and re-read the package database for every row.
pub struct System {
    pub gpus: Vec<Gpu>,
    pub packages: std::collections::BTreeSet<String>,
    pub kernels: Vec<drivers::Kernel>,
    pub dkms: Vec<drivers::DkmsModule>,
    pub missing: Vec<Requirement>,
}

impl System {
    pub fn read() -> System {
        let gpus = gpu::discover();
        let packages = drivers::installed_packages();
        let installed = |name: &str| packages.contains(name);
        let missing = drivers::requirements(&gpus, &installed);
        System {
            kernels: drivers::installed_kernels(),
            dkms: drivers::dkms_status(),
            missing,
            packages,
            gpus,
        }
    }

    pub fn has(&self, package: &str) -> bool {
        self.packages.contains(package)
    }
}

/// The whole readiness list, in the order it is worth reading.
pub fn all(system: &System) -> Vec<Check> {
    let mut checks = vec![driver_check(system), libraries_check(system)];
    checks.push(modules_check(system));
    checks.push(vulkan_check(system));
    checks.push(thirty_two_bit_check(system));
    checks.push(tweaks_check());
    checks.push(steam_check(system));
    checks.push(extras_check(system));
    checks
}

/// A single number for the hero: how many checks are clear.
pub fn score(checks: &[Check]) -> (usize, usize) {
    let counted: Vec<&Check> = checks
        .iter()
        .filter(|c| c.state != State::Unknown)
        .collect();
    let good = counted.iter().filter(|c| c.state == State::Good).count();
    (good, counted.len())
}

/// The headline over the readiness list. A problem outranks an advisory,
/// which outranks everything being fine.
pub fn verdict(checks: &[Check]) -> (State, &'static str, &'static str) {
    if checks.iter().any(|c| c.state == State::Problem) {
        (
            State::Problem,
            "Games will not run properly yet",
            "Something below stops a game starting, or makes it far slower than it should be.",
        )
    } else if checks.iter().any(|c| c.state == State::Advisory) {
        (
            State::Advisory,
            "Ready to play",
            "Everything a game needs is in place. What is left below is optional.",
        )
    } else {
        (
            State::Good,
            "Ready to play",
            "Drivers, libraries and system settings are all where they should be.",
        )
    }
}

/// Every fix the list offers, gathered into as few operations as possible
/// — one package transaction, one module build, one settings change — so
/// "Fix what is missing" asks for a password once rather than five times.
pub fn combined_fixes(checks: &[Check]) -> Vec<Fix> {
    let mut packages: Vec<String> = Vec::new();
    let mut kernels: Vec<String> = Vec::new();
    let mut tweaks = false;
    for check in checks {
        match &check.fix {
            Some(Fix::Install(names)) => {
                for name in names {
                    if !packages.contains(name) {
                        packages.push(name.clone());
                    }
                }
            }
            Some(Fix::BuildModules(list)) => {
                for kernel in list {
                    if !kernels.contains(kernel) {
                        kernels.push(kernel.clone());
                    }
                }
            }
            Some(Fix::ApplyTweaks) => tweaks = true,
            Some(Fix::Manual(_)) | None => {}
        }
    }
    let mut fixes = Vec::new();
    if !packages.is_empty() {
        fixes.push(Fix::Install(packages));
    }
    if !kernels.is_empty() {
        fixes.push(Fix::BuildModules(kernels));
    }
    if tweaks {
        fixes.push(Fix::ApplyTweaks);
    }
    fixes
}

// ---- the checks themselves -----------------------------------------------

fn driver_check(system: &System) -> Check {
    let Some(gpu) = gpu::gaming_gpu(&system.gpus) else {
        return Check {
            title: "Graphics card".into(),
            detail: "No graphics card was found on the PCI bus. In a virtual machine this is normal; on real hardware it is not.".into(),
            state: State::Unknown,
            fix: None,
        };
    };
    let title = format!("Driver for {}", gpu.title());
    match gpu.driver {
        DriverKind::None => Check::problem(
            title,
            format!(
                "No kernel module is bound to this {} card, so nothing can render on it.",
                gpu.vendor
            ),
            driver_install_fix(system),
        ),
        DriverKind::Nouveau => Check::problem(
            title,
            "Nouveau cannot raise this card's clocks, so games run at a fraction of the speed the hardware is capable of. NVIDIA's own driver is the fix.",
            driver_install_fix(system),
        ),
        driver => Check::good(
            title,
            format!("{} is loaded and driving the card.", driver.label()),
        ),
    }
}

/// The driver package the missing-requirements list is already asking for,
/// if any — so the driver check and the libraries check never offer to
/// install the same thing twice.
fn driver_install_fix(system: &System) -> Option<Fix> {
    let names: Vec<String> = system
        .missing
        .iter()
        .filter(|r| r.package.contains("nvidia") || r.package.contains("dkms"))
        .map(|r| r.package.clone())
        .collect();
    (!names.is_empty()).then_some(Fix::Install(names))
}

fn libraries_check(system: &System) -> Check {
    let essential: Vec<&Requirement> = system.missing.iter().filter(|r| r.essential).collect();
    if essential.is_empty() {
        return Check::good(
            "Graphics libraries",
            "Every OpenGL and Vulkan library your cards need is installed, 64-bit and 32-bit.",
        );
    }
    let detail = essential
        .iter()
        .map(|r| format!("{} — {}", r.package, r.reason))
        .collect::<Vec<_>>()
        .join("\n");
    Check::problem(
        "Graphics libraries",
        detail,
        Some(Fix::Install(
            essential.iter().map(|r| r.package.clone()).collect(),
        )),
    )
}

fn modules_check(system: &System) -> Check {
    if !drivers::dkms_available() || system.dkms.is_empty() {
        return Check::good(
            "Driver modules for every kernel",
            "No driver on this system is built with DKMS, so there is nothing that a kernel update could leave behind.",
        );
    }
    let missing = drivers::kernels_missing_modules(&system.dkms, &system.kernels);
    if missing.is_empty() {
        let kernels = system.kernels.len();
        return Check::good(
            "Driver modules for every kernel",
            format!(
                "Built for {} installed kernel{}. A kernel update will not leave you without a driver.",
                kernels,
                if kernels == 1 { "" } else { "s" }
            ),
        );
    }
    let running_is_affected = system
        .kernels
        .iter()
        .any(|k| k.running && missing.contains(&k.release));
    let detail = format!(
        "The graphics module is not built for {}. {}",
        list(&missing),
        match (running_is_affected, missing.len()) {
            (true, _) => "That includes the kernel you are running now.",
            (false, 1) =>
                "Boot into it and the card will have no driver, which looks exactly like the driver having broken.",
            (false, _) =>
                "Boot into any of those and the card will have no driver, which looks exactly like the driver having broken.",
        }
    );
    Check::problem(
        "Driver modules for every kernel",
        detail,
        Some(Fix::BuildModules(missing)),
    )
}

fn vulkan_check(system: &System) -> Check {
    let icds = gpu::vulkan_icds();
    if icds.is_empty() {
        return Check::problem(
            "Vulkan",
            "No Vulkan driver is installed. Almost every game from the last five years needs one, and Proton needs it for all of them.",
            Some(Fix::Install(
                system
                    .missing
                    .iter()
                    .filter(|r| r.package.contains("vulkan") || r.package.contains("nvidia-utils"))
                    .map(|r| r.package.clone())
                    .collect(),
            )),
        );
    }
    let broken: Vec<&str> = icds
        .iter()
        .filter(|i| !i.library_present)
        .map(|i| i.file.as_str())
        .collect();
    if !broken.is_empty() {
        return Check::problem(
            "Vulkan",
            format!(
                "{} points at a driver library that is not on disk. This is a half-removed package, and Vulkan fails with an error that names neither.",
                broken.join(", ")
            ),
            Some(Fix::Manual(
                "Reinstall the driver package that owns the manifest, or remove the stale file.",
            )),
        );
    }
    let vendors: Vec<&str> = icds
        .iter()
        .filter(|icd| system.gpus.iter().any(|g| g.vendor == icd.vendor))
        .map(|icd| icd.vendor.name())
        .collect();
    if vendors.is_empty() {
        return Check::advisory(
            "Vulkan",
            format!(
                "{} Vulkan driver{} installed, but none of them is for the graphics in this machine.",
                icds.len(),
                if icds.len() == 1 { " is" } else { "s are" }
            ),
            None,
        );
    }
    Check::good(
        "Vulkan",
        format!(
            "A Vulkan driver is installed for your {} graphics.",
            list_str(&vendors)
        ),
    )
}

/// Whether the 32-bit world exists at all.
///
/// Deliberately narrow. Which *drivers* are missing their 32-bit half is
/// [`libraries_check`]'s job, and reporting the same two packages twice
/// with the same button under two headings would make one problem look
/// like two.
fn thirty_two_bit_check(system: &System) -> Check {
    if gpu::has_32bit_stack() {
        return Check::good(
            "32-bit support",
            "The 32-bit libraries are in place, which is what Proton, Steam Play and older native games are built against.",
        );
    }
    Check::problem(
        "32-bit support",
        "There are no 32-bit graphics libraries on this system. Proton, Steam Play, and every older native game need them; without them a game exits immediately with a linker error naming a library nobody has heard of.",
        Some(Fix::Install(
            system
                .missing
                .iter()
                .filter(|r| r.package.starts_with("lib32-"))
                .map(|r| r.package.clone())
                .collect(),
        )),
    )
}

fn tweaks_check() -> Check {
    let needed = tune::tweaks_needed();
    if needed.is_empty() {
        return Check::good(
            "System settings",
            "The kernel settings that large games depend on are all set.",
        );
    }
    // A too-low map count is the one that stops a game dead. The rest cost
    // smoothness, which is an advisory rather than a failure.
    let severe = needed.contains(&Tweak::MaxMapCount);
    let detail = needed
        .iter()
        .map(|t| format!("{} — {}", t.title(), t.change_text()))
        .collect::<Vec<_>>()
        .join("\n");
    if severe {
        Check::problem("System settings", detail, Some(Fix::ApplyTweaks))
    } else {
        Check::advisory("System settings", detail, Some(Fix::ApplyTweaks))
    }
}

fn steam_check(system: &System) -> Check {
    if crate::games::steam_installed() {
        let count = crate::games::discover().len();
        return Check::good(
            "Game launchers",
            match count {
                0 => "Steam is installed. No games yet.".to_string(),
                1 => "Steam is installed, with 1 game in the library.".to_string(),
                n => format!("Steam is installed, with {n} games in the library."),
            },
        );
    }
    if system.has("lutris") || system.has("heroic-games-launcher") {
        return Check::good("Game launchers", "A game launcher is installed.");
    }
    Check::advisory(
        "Game launchers",
        "No game launcher is installed. Steam brings Proton with it, which is what runs Windows games on Linux.",
        Some(Fix::Install(vec!["steam".into()])),
    )
}

fn extras_check(system: &System) -> Check {
    let missing: Vec<String> = ["gamemode", "mangohud"]
        .into_iter()
        .filter(|p| !system.has(p))
        .map(String::from)
        .collect();
    if missing.is_empty() {
        return Check::good(
            "Performance helpers",
            "GameMode and MangoHud are installed, so games can ask for performance mode and show their own frame rate.",
        );
    }
    Check::advisory(
        "Performance helpers",
        "GameMode switches the CPU to performance while a game runs; MangoHud shows frame rate and temperatures inside the game. Neither is required.",
        Some(Fix::Install(missing)),
    )
}

// ---- wording -------------------------------------------------------------

fn list(items: &[String]) -> String {
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    list_str(&refs)
}

fn list_str(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(state: State, fix: Option<Fix>) -> Check {
        Check {
            title: "x".into(),
            detail: "y".into(),
            state,
            fix,
        }
    }

    #[test]
    fn one_problem_decides_the_headline() {
        let checks = vec![
            check(State::Good, None),
            check(State::Advisory, None),
            check(State::Problem, None),
        ];
        assert_eq!(verdict(&checks).0, State::Problem);
    }

    #[test]
    fn advisories_alone_still_read_as_ready() {
        let checks = vec![check(State::Good, None), check(State::Advisory, None)];
        let (state, headline, _) = verdict(&checks);
        assert_eq!(state, State::Advisory);
        assert_eq!(headline, "Ready to play");
    }

    #[test]
    fn everything_clear_is_the_quiet_case() {
        assert_eq!(verdict(&[check(State::Good, None)]).0, State::Good);
    }

    #[test]
    fn unknown_checks_are_left_out_of_the_score() {
        let checks = vec![
            check(State::Good, None),
            check(State::Good, None),
            check(State::Problem, None),
            check(State::Unknown, None),
        ];
        assert_eq!(score(&checks), (2, 3));
    }

    #[test]
    fn fixes_are_gathered_into_one_of_each_kind() {
        let checks = vec![
            check(
                State::Problem,
                Some(Fix::Install(vec!["a".into(), "b".into()])),
            ),
            check(
                State::Problem,
                Some(Fix::Install(vec!["b".into(), "c".into()])),
            ),
            check(State::Problem, Some(Fix::BuildModules(vec!["6.1".into()]))),
            check(
                State::Problem,
                Some(Fix::BuildModules(vec!["6.1".into(), "7.2".into()])),
            ),
            check(State::Advisory, Some(Fix::ApplyTweaks)),
            check(State::Advisory, Some(Fix::ApplyTweaks)),
            check(State::Problem, Some(Fix::Manual("do it yourself"))),
            check(State::Good, None),
        ];
        let fixes = combined_fixes(&checks);
        assert_eq!(
            fixes,
            vec![
                // Deduplicated, and in the order the checks asked.
                Fix::Install(vec!["a".into(), "b".into(), "c".into()]),
                Fix::BuildModules(vec!["6.1".into(), "7.2".into()]),
                Fix::ApplyTweaks,
            ]
        );
    }

    #[test]
    fn nothing_to_fix_is_no_fixes() {
        assert!(combined_fixes(&[check(State::Good, None)]).is_empty());
        assert!(combined_fixes(&[]).is_empty());
    }

    #[test]
    fn lists_read_as_sentences() {
        assert_eq!(list_str(&[]), "");
        assert_eq!(list_str(&["one"]), "one");
        assert_eq!(list_str(&["one", "two"]), "one and two");
        assert_eq!(list_str(&["one", "two", "three"]), "one, two, and three");
    }
}
