//! What graphics hardware this computer has, and what is driving it.
//!
//! Everything here is read from `/sys`, which is always present and never
//! needs a tool installed. `lspci` is not run: it would only be parsing the
//! same PCI IDs back out of formatted text, and it is not on every install.
//!
//! A device's `vendor`/`device` files give the IDs, `class` says whether it
//! draws, and the `driver` symlink says which kernel module claimed it —
//! the one fact that separates "the card is in the machine" from "the card
//! works". A card with no driver bound is the shape of a fresh install that
//! has not had its driver put on yet, and the Drivers page is built around
//! recognising exactly that.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const PCI_DEVICES: &str = "/sys/bus/pci/devices";

/// PCI class 0x03 is "Display controller"; the low byte is the interface.
fn is_display_class(class: u32) -> bool {
    (class >> 16) == 0x03
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    Other,
}

impl Vendor {
    pub fn from_id(id: u16) -> Vendor {
        match id {
            0x10de => Vendor::Nvidia,
            0x1002 | 0x1022 => Vendor::Amd,
            0x8086 => Vendor::Intel,
            _ => Vendor::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Vendor::Nvidia => "NVIDIA",
            Vendor::Amd => "AMD",
            Vendor::Intel => "Intel",
            Vendor::Other => "Unknown vendor",
        }
    }

    /// The tint the sidebar and cards give this vendor. Not the accent:
    /// the accent is the person's, and a card's vendor is not a choice
    /// they made.
    pub fn tint(self) -> &'static str {
        match self {
            Vendor::Nvidia => "green",
            Vendor::Amd => "red",
            Vendor::Intel => "blue",
            Vendor::Other => "gray",
        }
    }
}

impl fmt::Display for Vendor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How a card is being driven, judged only by the module bound to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverKind {
    /// NVIDIA's own module, proprietary or open-kernel — both are `nvidia`.
    NvidiaProprietary,
    /// The reverse-engineered NVIDIA driver. Runs the desktop; will not run
    /// a modern game at a playable rate.
    Nouveau,
    /// The in-tree AMD driver. Nothing to install.
    Amdgpu,
    /// AMD's older in-tree driver, for pre-GCN cards.
    Radeon,
    /// The in-tree Intel driver, old or new.
    Intel,
    /// Something claimed it that we do not recognise.
    Other(&'static str),
    /// Nothing is bound. The card is inert.
    None,
}

impl DriverKind {
    fn from_module(module: Option<&str>) -> DriverKind {
        match module {
            Some("nvidia") => DriverKind::NvidiaProprietary,
            Some("nouveau") => DriverKind::Nouveau,
            Some("amdgpu") => DriverKind::Amdgpu,
            Some("radeon") => DriverKind::Radeon,
            Some("i915") | Some("xe") => DriverKind::Intel,
            Some(_) => DriverKind::Other("other"),
            None => DriverKind::None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DriverKind::NvidiaProprietary => "NVIDIA driver",
            DriverKind::Nouveau => "Nouveau",
            DriverKind::Amdgpu => "amdgpu",
            DriverKind::Radeon => "radeon",
            DriverKind::Intel => "Intel",
            DriverKind::Other(name) => name,
            DriverKind::None => "No driver",
        }
    }

    /// Whether this driver can be expected to run a 3D game well.
    pub fn plays_games(self) -> bool {
        matches!(
            self,
            DriverKind::NvidiaProprietary
                | DriverKind::Amdgpu
                | DriverKind::Radeon
                | DriverKind::Intel
        )
    }
}

#[derive(Debug, Clone)]
pub struct Gpu {
    /// `0000:01:00.0`.
    pub address: String,
    pub vendor: Vendor,
    pub vendor_id: u16,
    pub device_id: u16,
    /// The marketing name, when the PCI ID database knew it.
    pub model: String,
    pub driver: DriverKind,
    /// The kernel module's name, verbatim.
    pub module: Option<String>,
    /// `/dev/dri/cardN`, when the driver made one.
    pub card: Option<String>,
    /// `/dev/dri/renderDN`, the node a game actually renders on.
    pub render_node: Option<PathBuf>,
    /// Whether the firmware booted with this card as the display.
    pub boot_vga: bool,
    /// A card behind a laptop's internal panel, told by having an eDP or
    /// LVDS connector of its own.
    pub drives_internal_panel: bool,
    /// The machine's built-in graphics, as against a card added to it.
    /// Decided by [`discover`] once every card is known, because the
    /// question only has an answer relative to the others.
    pub integrated: bool,
}

impl Gpu {
    /// The line the UI names a card by.
    pub fn title(&self) -> String {
        if self.model.is_empty() {
            format!(
                "{} device {:04x}:{:04x}",
                self.vendor, self.vendor_id, self.device_id
            )
        } else {
            self.model.clone()
        }
    }

    /// The machine's built-in graphics. See [`classify`] for how it is
    /// decided; PCI itself does not say.
    pub fn is_integrated(&self) -> bool {
        self.integrated
    }
}

/// Every display controller on the PCI bus, in bus order.
pub fn discover() -> Vec<Gpu> {
    let mut gpus = Vec::new();
    let Ok(entries) = fs::read_dir(PCI_DEVICES) else {
        return gpus;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let Some(class) = read_hex(path.join("class")) else {
            continue;
        };
        if !is_display_class(class as u32) {
            continue;
        }
        let vendor_id = read_hex(path.join("vendor")).unwrap_or(0) as u16;
        let device_id = read_hex(path.join("device")).unwrap_or(0) as u16;
        let address = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let module = fs::read_link(path.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
        let (card, render_node) = drm_nodes(&path);
        let drives_internal_panel = card.as_deref().is_some_and(drives_internal_panel);
        gpus.push(Gpu {
            address,
            vendor: Vendor::from_id(vendor_id),
            vendor_id,
            device_id,
            model: model_name(vendor_id, device_id),
            driver: DriverKind::from_module(module.as_deref()),
            module,
            card,
            render_node,
            boot_vga: read_num(path.join("boot_vga")) == Some(1),
            drives_internal_panel,
            integrated: false,
        });
    }
    classify(&mut gpus);
    gpus
}

/// Decides which cards are the machine's built-in graphics.
///
/// Two signals, and both are needed because either alone is wrong
/// somewhere:
///
/// * **An internal panel.** A card with an eDP or LVDS connector is wired
///   to a laptop's own screen, which only built-in graphics normally is.
/// * **`boot_vga`.** The firmware brings up the integrated GPU on a hybrid
///   laptop, because the discrete one is powered down until something asks
///   for it.
///
/// `boot_vga` is only consulted on a machine that has a panel at all. On a
/// desktop it marks whichever card the firmware chose, which says nothing
/// about integrated versus discrete, and treating it as such would have
/// this app tell someone with two graphics cards to render on the wrong
/// one.
///
/// If every card ends up marked — a laptop whose panel hangs off the
/// discrete card, so one card has the panel and the other has `boot_vga` —
/// the signals are not telling them apart, and none is marked rather than
/// all. There is then no offload to recommend, which on such a machine is
/// the right answer anyway.
fn classify(gpus: &mut [Gpu]) {
    let is_laptop = gpus.iter().any(|g| g.drives_internal_panel);
    if gpus.len() < 2 || !is_laptop {
        return;
    }
    for gpu in gpus.iter_mut() {
        gpu.integrated = gpu.drives_internal_panel || gpu.boot_vga;
    }
    if gpus.iter().all(|g| g.integrated) {
        for gpu in gpus.iter_mut() {
            gpu.integrated = false;
        }
    }
}

/// The `cardN` and `renderDN` a PCI device owns.
fn drm_nodes(device: &Path) -> (Option<String>, Option<PathBuf>) {
    let mut card = None;
    let mut render = None;
    let Ok(entries) = fs::read_dir(device.join("drm")) else {
        return (card, render);
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        match name.strip_prefix("card") {
            Some(rest) if rest.chars().all(|c| c.is_ascii_digit()) => card = Some(name),
            _ if name.starts_with("renderD") => {
                render = Some(PathBuf::from("/dev/dri").join(&name));
            }
            _ => {}
        }
    }
    (card, render)
}

/// Whether a card has a connector for a built-in screen.
///
/// The connectors are children of the card in `/sys/class/drm/cardN/`, not
/// of the PCI device — the PCI device's own `drm/` directory holds only
/// the card, control and render nodes. Looking in the wrong one of those
/// two finds no connectors at all, which reads as "no card drives the
/// panel" and gets every hybrid laptop backwards.
fn drives_internal_panel(card: &str) -> bool {
    let prefix = format!("{card}-");
    fs::read_dir(Path::new("/sys/class/drm").join(card))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.strip_prefix(&prefix).map(String::from)
        })
        .any(|connector| connector.starts_with("eDP") || connector.starts_with("LVDS"))
}

/// The card a game renders on unless it is told otherwise: the one the
/// firmware booted with, else the first with a render node, else the first
/// at all.
pub fn primary(gpus: &[Gpu]) -> Option<&Gpu> {
    gpus.iter()
        .find(|g| g.boot_vga && g.render_node.is_some())
        .or_else(|| gpus.iter().find(|g| g.render_node.is_some()))
        .or_else(|| gpus.first())
}

/// The card worth playing on: the fastest one present. Discrete beats
/// integrated, and a machine with one card answers with that card.
///
/// Among cards of equal standing the first on the bus wins, so the answer
/// does not depend on directory order — `max_by_key` would quietly return
/// the *last* of several equals, and a machine with two cards of the same
/// vendor would get a different recommendation for no visible reason.
pub fn gaming_gpu(gpus: &[Gpu]) -> Option<&Gpu> {
    gpus.iter()
        .filter(|g| !g.is_integrated())
        .fold(None, |best: Option<&Gpu>, gpu| match best {
            Some(best) if vendor_rank(best.vendor) >= vendor_rank(gpu.vendor) => Some(best),
            _ => Some(gpu),
        })
        .or_else(|| gpus.first())
}

fn vendor_rank(vendor: Vendor) -> u8 {
    match vendor {
        Vendor::Nvidia | Vendor::Amd => 2,
        Vendor::Intel => 1,
        Vendor::Other => 0,
    }
}

/// True when the machine has a discrete card *and* an integrated one, the
/// arrangement where which card a game lands on is a question with an
/// answer worth showing.
pub fn is_hybrid(gpus: &[Gpu]) -> bool {
    gpus.len() > 1
        && gpus.iter().any(|g| g.is_integrated())
        && gpus.iter().any(|g| !g.is_integrated())
}

// ---- Vulkan --------------------------------------------------------------

/// A Vulkan driver installed on the system, as named by its ICD manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VulkanIcd {
    /// `nvidia_icd.json`.
    pub file: String,
    /// The vendor the file name gives away.
    pub vendor: Vendor,
    /// Whether the library the manifest points at is actually on disk.
    /// A manifest whose library is missing is the classic half-installed
    /// driver, and Vulkan fails with a message that names neither.
    pub library_present: bool,
}

const ICD_DIRS: [&str; 3] = [
    "/usr/share/vulkan/icd.d",
    "/usr/local/share/vulkan/icd.d",
    "/etc/vulkan/icd.d",
];

pub fn vulkan_icds() -> Vec<VulkanIcd> {
    let mut icds: Vec<VulkanIcd> = Vec::new();
    for dir in ICD_DIRS {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if !file.ends_with(".json") || icds.iter().any(|i| i.file == file) {
                continue;
            }
            let text = fs::read_to_string(entry.path()).unwrap_or_default();
            icds.push(VulkanIcd {
                vendor: icd_vendor(&file),
                library_present: icd_library_present(&text),
                file,
            });
        }
    }
    icds.sort_by(|a, b| a.file.cmp(&b.file));
    icds
}

fn icd_vendor(file: &str) -> Vendor {
    let f = file.to_ascii_lowercase();
    if f.contains("nvidia") {
        Vendor::Nvidia
    } else if f.contains("radeon") || f.contains("amd") {
        Vendor::Amd
    } else if f.contains("intel") {
        Vendor::Intel
    } else {
        Vendor::Other
    }
}

/// Whether the `library_path` in an ICD manifest resolves to a file. A
/// bare soname is left alone: resolving it means walking the loader's
/// search path, and a bare soname is the normal, working case.
fn icd_library_present(manifest: &str) -> bool {
    let Some(path) = json_string(manifest, "library_path") else {
        return false;
    };
    if !path.contains('/') {
        return true;
    }
    Path::new(&path).exists()
}

/// The one string value out of a small, flat JSON manifest. These files
/// are written by driver packages and are two keys deep; a parser is not
/// needed and `serde_json` is not pulled in for them.
fn json_string(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after = &text[text.find(&needle)? + needle.len()..];
    let after = after.trim_start().strip_prefix(':')?.trim_start();
    let rest = after.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

// ---- 32-bit support ------------------------------------------------------

/// Whether the 32-bit graphics stack is installed. Proton and every older
/// native game are 32-bit, and without these a game exits at once with a
/// linker error that names a library the person has never heard of.
pub fn has_32bit_stack() -> bool {
    ["/usr/lib32", "/usr/lib/i386-linux-gnu"]
        .iter()
        .any(|dir| Path::new(dir).join("libGL.so.1").exists())
}

// ---- small sysfs helpers -------------------------------------------------

pub fn read_text(path: impl AsRef<Path>) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

pub fn read_num(path: impl AsRef<Path>) -> Option<u64> {
    read_text(path)?.parse().ok()
}

fn read_hex(path: impl AsRef<Path>) -> Option<u64> {
    let text = read_text(path)?;
    u64::from_str_radix(text.trim_start_matches("0x"), 16).ok()
}

// ---- model names ---------------------------------------------------------

/// The card's marketing name.
///
/// `hwdata`'s `pci.ids` is on most installs and is the authority, so it is
/// read when present. It is not a dependency: without it a card still gets
/// a name from its vendor, and the UI never shows a blank where a model
/// should be.
fn model_name(vendor: u16, device: u16) -> String {
    for path in ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"] {
        if let Ok(text) = fs::read_to_string(path)
            && let Some(name) = lookup_pci_id(&text, vendor, device)
        {
            return name;
        }
    }
    String::new()
}

/// Finds `device` under `vendor` in a `pci.ids` file.
///
/// The format: a vendor at column 0, its devices indented one tab, their
/// subsystems two. Anything more indented than one tab belongs to the
/// device above and is skipped.
fn lookup_pci_id(text: &str, vendor: u16, device: u16) -> Option<String> {
    let vendor_prefix = format!("{vendor:04x}");
    let device_prefix = format!("\t{device:04x}");
    let mut in_vendor = false;
    for line in text.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                // Left our vendor's block without finding the device.
                return None;
            }
            in_vendor = line.starts_with(&vendor_prefix);
            continue;
        }
        if in_vendor
            && !line.starts_with("\t\t")
            && let Some(rest) = line.strip_prefix(&device_prefix)
        {
            return Some(clean_model(rest.trim()));
        }
    }
    None
}

/// `pci.ids` names carry a bracketed retail name after the chip's own —
/// "TU116M [GeForce GTX 1660 Ti Mobile]". The bracketed half is the one
/// people recognise, so it is what the UI shows.
fn clean_model(name: &str) -> String {
    match (name.find('['), name.rfind(']')) {
        (Some(open), Some(close)) if close > open + 1 => name[open + 1..close].to_string(),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_controllers_are_class_03() {
        assert!(is_display_class(0x030000));
        assert!(is_display_class(0x030200));
        assert!(!is_display_class(0x000000));
        assert!(!is_display_class(0x040300));
    }

    #[test]
    fn vendors_come_from_their_pci_ids() {
        assert_eq!(Vendor::from_id(0x10de), Vendor::Nvidia);
        assert_eq!(Vendor::from_id(0x1002), Vendor::Amd);
        assert_eq!(Vendor::from_id(0x8086), Vendor::Intel);
        assert_eq!(Vendor::from_id(0x1234), Vendor::Other);
    }

    #[test]
    fn nouveau_is_not_a_gaming_driver() {
        assert!(!DriverKind::from_module(Some("nouveau")).plays_games());
        assert!(!DriverKind::from_module(None).plays_games());
        assert!(DriverKind::from_module(Some("nvidia")).plays_games());
        assert!(DriverKind::from_module(Some("amdgpu")).plays_games());
        assert!(DriverKind::from_module(Some("i915")).plays_games());
        assert_eq!(DriverKind::from_module(Some("xe")), DriverKind::Intel);
        assert!(!DriverKind::from_module(Some("vfio-pci")).plays_games());
    }

    #[test]
    fn a_pci_id_lookup_finds_the_retail_name() {
        let ids = "\
# comment
1002  Advanced Micro Devices, Inc. [AMD/ATI]
\t15d8  Picasso/Raven 2 [Radeon Vega Series]
10de  NVIDIA Corporation
\t2191  TU116M [GeForce GTX 1660 Ti Mobile]
\t\t1043 1d81  Some laptop
\t2192  TU116M
";
        assert_eq!(
            lookup_pci_id(ids, 0x10de, 0x2191).as_deref(),
            Some("GeForce GTX 1660 Ti Mobile")
        );
        assert_eq!(
            lookup_pci_id(ids, 0x1002, 0x15d8).as_deref(),
            Some("Radeon Vega Series")
        );
        // A name with no bracket is kept whole.
        assert_eq!(
            lookup_pci_id(ids, 0x10de, 0x2192).as_deref(),
            Some("TU116M")
        );
        // A subsystem line is never mistaken for a device.
        assert_eq!(lookup_pci_id(ids, 0x10de, 0x1d81), None);
        // An absent device under a present vendor is absent.
        assert_eq!(lookup_pci_id(ids, 0x10de, 0x9999), None);
    }

    #[test]
    fn an_icd_manifest_is_read_for_its_library() {
        let manifest = r#"{"file_format_version":"1.0.0","ICD":{"library_path":"libGLX_nvidia.so.0","api_version":"1.3"}}"#;
        assert!(icd_library_present(manifest));
        let absolute = r#"{"ICD":{"library_path":"/nonexistent/libvulkan_thing.so"}}"#;
        assert!(!icd_library_present(absolute));
        assert!(!icd_library_present("{}"));
    }

    #[test]
    fn icd_files_name_their_vendor() {
        assert_eq!(icd_vendor("nvidia_icd.json"), Vendor::Nvidia);
        assert_eq!(icd_vendor("radeon_icd.x86_64.json"), Vendor::Amd);
        assert_eq!(icd_vendor("intel_hasvk_icd.json"), Vendor::Intel);
        assert_eq!(icd_vendor("lvp_icd.json"), Vendor::Other);
    }

    fn gpu(address: &str, vendor: Vendor, panel: bool, boot: bool) -> Gpu {
        Gpu {
            address: address.into(),
            vendor,
            vendor_id: 0,
            device_id: 0,
            model: String::new(),
            driver: DriverKind::None,
            module: None,
            card: Some("card0".into()),
            render_node: Some(PathBuf::from("/dev/dri/renderD128")),
            boot_vga: boot,
            drives_internal_panel: panel,
            integrated: false,
        }
    }

    /// As `discover` builds them: raw signals in, classified out.
    fn machine(gpus: Vec<Gpu>) -> Vec<Gpu> {
        let mut gpus = gpus;
        classify(&mut gpus);
        gpus
    }

    #[test]
    fn the_discrete_card_is_the_one_to_play_on() {
        // The machine this was written on: a GTX 1660 Ti with no
        // connectors in use, beside a Vega iGPU that has the panel and
        // that the firmware booted on.
        let gpus = machine(vec![
            gpu("0000:01:00.0", Vendor::Nvidia, false, false),
            gpu("0000:05:00.0", Vendor::Amd, true, true),
        ]);
        assert!(is_hybrid(&gpus));
        assert!(!gpus[0].is_integrated());
        assert!(gpus[1].is_integrated());
        assert_eq!(gaming_gpu(&gpus).unwrap().address, "0000:01:00.0");
        // The panel's card is what the desktop is already on.
        assert_eq!(primary(&gpus).unwrap().address, "0000:05:00.0");
    }

    #[test]
    fn boot_vga_alone_marks_the_integrated_card_on_a_laptop() {
        // Some laptops expose no connector at all on the iGPU; boot_vga
        // is then the only thing telling the two apart.
        let gpus = machine(vec![
            gpu("0000:00:02.0", Vendor::Intel, false, true),
            gpu("0000:01:00.0", Vendor::Nvidia, false, false),
        ]);
        // No panel anywhere means this does not look like a laptop, so
        // boot_vga is not read as "integrated".
        assert!(!gpus[0].is_integrated());

        let gpus = machine(vec![
            gpu("0000:00:02.0", Vendor::Intel, true, true),
            gpu("0000:01:00.0", Vendor::Nvidia, false, false),
        ]);
        assert!(gpus[0].is_integrated());
        assert_eq!(gaming_gpu(&gpus).unwrap().address, "0000:01:00.0");
    }

    #[test]
    fn two_cards_in_a_desktop_are_both_discrete() {
        // No panel: boot_vga says which one the firmware picked, not
        // which one is built in. Neither is integrated, and the app must
        // not start recommending an offload.
        let gpus = machine(vec![
            gpu("0000:01:00.0", Vendor::Nvidia, false, true),
            gpu("0000:02:00.0", Vendor::Amd, false, false),
        ]);
        assert!(!is_hybrid(&gpus));
        assert!(gpus.iter().all(|g| !g.is_integrated()));
        // The first of two equals, so the answer does not depend on the
        // order sysfs happened to list them in.
        assert_eq!(gaming_gpu(&gpus).unwrap().address, "0000:01:00.0");
    }

    #[test]
    fn a_panel_on_the_discrete_card_marks_neither() {
        // A muxed laptop: the panel is on the NVIDIA card, the firmware
        // booted on the Intel one. Both signals fire, on different cards.
        let gpus = machine(vec![
            gpu("0000:00:02.0", Vendor::Intel, false, true),
            gpu("0000:01:00.0", Vendor::Nvidia, true, false),
        ]);
        assert!(gpus.iter().all(|g| !g.is_integrated()));
        assert!(!is_hybrid(&gpus));
        assert_eq!(gaming_gpu(&gpus).unwrap().address, "0000:01:00.0");
    }

    #[test]
    fn one_card_is_not_a_hybrid_machine() {
        let gpus = machine(vec![gpu("0000:01:00.0", Vendor::Amd, true, true)]);
        assert!(!is_hybrid(&gpus));
        assert!(!gpus[0].is_integrated());
        assert_eq!(gaming_gpu(&gpus).unwrap().address, "0000:01:00.0");
        assert_eq!(primary(&gpus).unwrap().address, "0000:01:00.0");
    }

    #[test]
    fn a_machine_with_no_card_answers_nothing() {
        assert!(gaming_gpu(&[]).is_none());
        assert!(primary(&[]).is_none());
        assert!(!is_hybrid(&[]));
    }
}
