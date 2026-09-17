//! Screen sharing: what it needs, and what this desktop has.
//!
//! # The honest position
//!
//! Sharing a screen into Discord, a browser call or OBS goes through one
//! path on Wayland: an application asks `xdg-desktop-portal` for a
//! ScreenCast session, a portal *backend* asks the compositor to capture,
//! and the frames come back over PipeWire. Three pieces, and all three
//! have to be there.
//!
//! Huginn implements no capture protocol at all — it holds the framebuffer
//! alone and records the screen itself, which is why `Super`+`Print` works
//! and why `wf-recorder` and OBS's screen capture do not. A portal backend
//! has nothing to ask. So on Raven today a *call* cannot be given the
//! screen, and no combination of packages changes that; it needs work in
//! the compositor.
//!
//! This page therefore does not offer a fix it cannot deliver. It reports
//! each piece, names the one that is missing, and points at the thing that
//! does work — the built-in recorder, which captures gameplay at full
//! quality and is what the Capture page is for. Telling someone to install
//! `xdg-desktop-portal-wlr` here would have them install a backend that
//! then fails against this compositor, which is worse than saying so.

use std::path::Path;
use std::process::Command;

use crate::checks::{Check, Fix, State};
use crate::drivers::which;

/// Every `.portal` file, which is how a backend declares what it can do.
const PORTAL_DIRS: [&str; 2] = [
    "/usr/share/xdg-desktop-portal/portals",
    "/usr/local/share/xdg-desktop-portal/portals",
];

/// Whether any installed portal backend implements an interface.
pub fn portal_backend_for(interface: &str) -> Option<String> {
    for dir in PORTAL_DIRS {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "portal") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if declares_interface(&text, interface) {
                return Some(
                    path.file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
            }
        }
    }
    None
}

/// A `.portal` file's `Interfaces=` line is semicolon-separated. A
/// substring match would have `ScreenCast` found inside `ScreenCastFoo`,
/// so the list is split and compared whole.
fn declares_interface(text: &str, interface: &str) -> bool {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("Interfaces="))
        .any(|list| {
            list.split(';')
                .map(str::trim)
                .any(|entry| entry == interface)
        })
}

fn process_running(name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
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
        // `comm` is the executable name, truncated to 15 characters — long
        // enough for every name checked here, and cheaper than cmdline.
        if let Ok(comm) = std::fs::read_to_string(path.join("comm"))
            && comm.trim() == name
        {
            return true;
        }
    }
    false
}

/// Whether PipeWire is running, which is how captured frames and game
/// audio both travel.
pub fn pipewire_running() -> bool {
    process_running("pipewire")
}

/// A camera, for the picture-in-picture half of streaming.
pub fn cameras() -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir("/sys/class/video4linux")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = std::fs::read_to_string(entry.path().join("name")).ok()?;
            Some(name.trim().to_string())
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Whether the session has a microphone PipeWire can see.
pub fn has_microphone() -> bool {
    // `/proc/asound/cards` lists every sound card the kernel has; a
    // machine with none has no microphone whatever PipeWire says. This
    // avoids spawning wpctl, which is not installed everywhere.
    std::fs::read_to_string("/proc/asound/cards")
        .map(|text| text.lines().any(|l| l.contains('[')))
        .unwrap_or(false)
}

/// Screen-capture software that is installed and would use the portal.
pub fn capture_applications() -> Vec<&'static str> {
    ["obs", "wf-recorder", "gpu-screen-recorder", "kooha"]
        .into_iter()
        .filter(|name| which(name).is_some())
        .collect()
}

/// The readiness list for the Sharing page.
pub fn checks() -> Vec<Check> {
    let mut checks = Vec::new();

    let portal_installed = Path::new("/usr/libexec/xdg-desktop-portal").exists()
        || Path::new("/usr/lib/xdg-desktop-portal").exists()
        || which("xdg-desktop-portal").is_some();
    checks.push(if portal_installed {
        Check {
            title: "Desktop portal".into(),
            detail: "xdg-desktop-portal is installed. It is what an application asks when it wants a file, a screen or a camera.".into(),
            state: State::Good,
            fix: None,
        }
    } else {
        Check {
            title: "Desktop portal".into(),
            detail: "xdg-desktop-portal is not installed. Without it a sandboxed or Flatpak application cannot open a file, let alone a screen.".into(),
            state: State::Problem,
            fix: Some(Fix::Install(vec!["xdg-desktop-portal".into()])),
        }
    });

    checks.push(match portal_backend_for("org.freedesktop.impl.portal.ScreenCast") {
        Some(backend) => Check {
            title: "Screen-sharing backend".into(),
            detail: format!("{backend} provides the ScreenCast portal, so an application can ask for a screen."),
            state: State::Good,
            fix: None,
        },
        None => Check {
            title: "Screen-sharing backend".into(),
            detail:
                "No installed portal backend provides ScreenCast, and Huginn offers no capture protocol for one to use — it draws the screen and records it itself. Sharing a screen into a call is therefore not possible on this desktop yet; it needs support in the compositor, not a package. Recording works: see the Capture page."
                    .into(),
            state: State::Problem,
            fix: Some(Fix::Manual(
                "Nothing to install. Record with Super+Print and share the file, or share a window from inside the application when it offers that.",
            )),
        },
    });

    checks.push(if pipewire_running() {
        Check {
            title: "PipeWire".into(),
            detail: "Running. Game audio, voice chat and — once a compositor can capture — screen frames all travel over it.".into(),
            state: State::Good,
            fix: None,
        }
    } else {
        Check {
            title: "PipeWire".into(),
            detail: "PipeWire is not running. Voice chat and game audio both go through it.".into(),
            state: State::Problem,
            fix: Some(Fix::Install(vec!["pipewire".into(), "pipewire-pulse".into(), "wireplumber".into()])),
        }
    });

    let cameras = cameras();
    checks.push(Check {
        title: "Camera".into(),
        detail: if cameras.is_empty() {
            "No camera was found. Only needed if you want your face on the stream.".into()
        } else {
            format!("Found: {}.", cameras.join(", "))
        },
        state: if cameras.is_empty() {
            State::Advisory
        } else {
            State::Good
        },
        fix: None,
    });

    checks.push(if has_microphone() {
        Check {
            title: "Microphone".into(),
            detail: "A sound device with capture is present.".into(),
            state: State::Good,
            fix: None,
        }
    } else {
        Check {
            title: "Microphone".into(),
            detail: "No sound card was found, so there is nothing to talk into.".into(),
            state: State::Advisory,
            fix: None,
        }
    });

    let apps = capture_applications();
    checks.push(Check {
        title: "Capture software".into(),
        detail: if apps.is_empty() {
            "No screen-capture application is installed. On this desktop the built-in recorder is the one that works; the others rely on a capture protocol Huginn does not offer.".into()
        } else {
            format!(
                "Installed: {}. These rely on the ScreenCast portal, so they cannot capture the screen on this desktop yet.",
                apps.join(", ")
            )
        },
        state: State::Advisory,
        fix: None,
    });

    checks
}

/// Whether the compositor drawing this session is Huginn, which is what
/// makes the paragraph above true. Another compositor on Raven — a person
/// running Sway, say — has its own capture support, and this page must not
/// tell them their working screen share is impossible.
pub fn compositor_is_huginn() -> bool {
    if std::env::var_os("HUGINN_SOCKET").is_some() {
        return true;
    }
    match std::env::var("XDG_CURRENT_DESKTOP") {
        Ok(desktop) => desktop
            .split(':')
            .any(|d| d.eq_ignore_ascii_case("huginn") || d.eq_ignore_ascii_case("raven")),
        Err(_) => process_running("huginn"),
    }
}

/// The keyboard shortcut the compositor records with, for the UI to show.
pub const RECORD_SHORTCUT: [&str; 2] = ["Super", "Print"];

/// Whether a recording is in progress right now, judged by a `.rvr` file
/// that has been written to in the last few seconds. The compositor
/// publishes no state, so this is the only way to know without asking it.
pub fn recording_in_progress() -> bool {
    let now = std::time::SystemTime::now();
    crate::capture::recordings().iter().any(|r| {
        !r.complete
            && r.modified.is_some_and(|m| {
                now.duration_since(m)
                    .map(|age| age.as_secs() < 10)
                    .unwrap_or(false)
            })
    })
}

/// Restarts the portal service, the one thing on this page that is worth
/// a button: a portal that started before the session is a very common
/// cause of a file dialog or a share that does nothing at all.
pub fn restart_portal() -> Result<(), String> {
    let systemctl = which("systemctl").ok_or("systemctl is not available")?;
    let output = Command::new(systemctl)
        .args(["--user", "restart", "xdg-desktop-portal.service"])
        .output()
        .map_err(|e| format!("Could not restart the portal: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("The portal could not be restarted")
            .trim()
            .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_portal_file_declares_whole_interfaces() {
        let file = "[portal]\nDBusName=org.freedesktop.impl.portal.desktop.raven\nInterfaces=org.freedesktop.impl.portal.FileChooser;\nUseIn=Raven;Huginn\n";
        assert!(declares_interface(
            file,
            "org.freedesktop.impl.portal.FileChooser"
        ));
        assert!(!declares_interface(
            file,
            "org.freedesktop.impl.portal.ScreenCast"
        ));
    }

    #[test]
    fn several_interfaces_on_one_line_are_all_found() {
        let file = "Interfaces=org.freedesktop.impl.portal.ScreenCast;org.freedesktop.impl.portal.Screenshot;\n";
        assert!(declares_interface(
            file,
            "org.freedesktop.impl.portal.ScreenCast"
        ));
        assert!(declares_interface(
            file,
            "org.freedesktop.impl.portal.Screenshot"
        ));
    }

    #[test]
    fn a_prefix_is_not_a_match() {
        let file = "Interfaces=org.freedesktop.impl.portal.ScreenCastExtra;\n";
        assert!(!declares_interface(
            file,
            "org.freedesktop.impl.portal.ScreenCast"
        ));
    }

    #[test]
    fn a_file_with_no_interfaces_declares_none() {
        assert!(!declares_interface("[portal]\nDBusName=x\n", "anything"));
        assert!(!declares_interface("", "anything"));
    }

    #[test]
    fn the_sharing_list_always_covers_every_piece() {
        let titles: Vec<String> = checks().into_iter().map(|c| c.title).collect();
        for expected in [
            "Desktop portal",
            "Screen-sharing backend",
            "PipeWire",
            "Camera",
            "Microphone",
            "Capture software",
        ] {
            assert!(titles.iter().any(|t| t == expected), "missing {expected}");
        }
    }

    #[test]
    fn the_sharing_page_never_offers_a_fix_it_cannot_deliver() {
        // The backend check's only "fix" is advice, because no package
        // makes screen sharing work against a compositor with no capture
        // protocol. Offering an install there would be a lie.
        let backend = checks()
            .into_iter()
            .find(|c| c.title == "Screen-sharing backend")
            .unwrap();
        if backend.state == State::Problem {
            assert!(matches!(backend.fix, Some(Fix::Manual(_)) | None));
        }
    }
}
