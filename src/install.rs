//! Installing packages, by driving `rvn --json`.
//!
//! The same approach Raven Store takes, and for the same reasons: rvn is
//! run as a process and its JSON event stream is read, rather than linked
//! in. One code path serves the terminal and every front end, and this
//! window stays an ordinary user process — rvn hands the root half to
//! `rvnd` on `/run/rvn/ctl` itself, so there is no sudo and no password
//! dialog for a package install.
//!
//! Only the events a progress dialog can show anything with are kept. The
//! rest of rvn's stream is real and useful in a terminal, and noise in a
//! two-line dialog.

use std::io::{BufRead, BufReader};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

use serde_json::Value;

use crate::drivers::which;

/// rvn's socket to its root daemon. `RVN_SOCKET` redirects rvn in
/// development, so this looks where rvn will look.
fn daemon_socket() -> PathBuf {
    std::env::var_os("RVN_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/rvn/ctl"))
}

/// Why an install cannot start right now, if it cannot.
///
/// In `--json` mode rvn does not explain a missing daemon; it falls back to
/// running in-process as the user and fails partway through with a
/// permission error that names sudo. Checking first gives the same advice
/// the terminal would give, before anything has been downloaded.
pub fn unavailable() -> Option<String> {
    if which("rvn").is_none() {
        return Some("rvn is not installed, so packages cannot be installed from here.".into());
    }
    use std::io::ErrorKind::{ConnectionRefused, NotFound};
    match UnixStream::connect(daemon_socket()) {
        Err(e) if matches!(e.kind(), NotFound | ConnectionRefused) => Some(
            "rvnd is not running, so packages cannot be installed from here. Start it with `sudo raven-rc start rvnd`, or run `sudo rvn install …` in a terminal."
                .into(),
        ),
        _ => None,
    }
}

/// What an install reports while it runs.
#[derive(Debug, Clone)]
pub enum Event {
    /// A stage began: "Resolving dependencies".
    Stage(String),
    /// Bytes or items done out of a total, for the bar.
    Progress {
        label: String,
        done: u64,
        total: u64,
    },
    /// A line worth putting in the log.
    Message(String),
    /// rvn reported a fatal error.
    Failed(String),
    /// The process exited.
    Exited { success: bool },
}

/// Starts `rvn install`, and returns the channel its events arrive on.
pub fn install(packages: &[String]) -> Result<Receiver<Event>, String> {
    if packages.is_empty() {
        return Err("Nothing to install".into());
    }
    for name in packages {
        validate_package_name(name)?;
    }
    let rvn = which("rvn").ok_or("rvn is not installed")?;
    let mut child = Command::new(rvn)
        .args(["--json", "-y", "install"])
        .args(packages)
        // `-y` never reads stdin.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start rvn: {e}"))?;

    let (sender, receiver) = mpsc::channel();
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let out = {
        let sender = sender.clone();
        std::thread::spawn(move || read_events(stdout, sender))
    };
    let err = {
        let sender = sender.clone();
        std::thread::spawn(move || read_log(stderr, sender))
    };
    std::thread::spawn(move || {
        let _ = out.join();
        let _ = err.join();
        let success = child.wait().map(|s| s.success()).unwrap_or(false);
        let _ = sender.send(Event::Exited { success });
    });
    Ok(receiver)
}

/// A package name, as Arch and rvn allow: letters, digits, and
/// `@._+-`. Anything else is not a package and is never handed to rvn —
/// the name comes from this app's own tables, so a failure here is a bug
/// in the app rather than something a person typed, and it should be loud.
fn validate_package_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '-'));
    if ok {
        Ok(())
    } else {
        Err(format!("Not a package name: {name}"))
    }
}

fn read_events(stdout: std::process::ChildStdout, sender: Sender<Event>) {
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(event) = translate(&value)
            && sender.send(event).is_err()
        {
            return;
        }
    }
}

/// One rvn JSON event into one of ours, or nothing when it is not
/// something a progress dialog shows.
fn translate(value: &Value) -> Option<Event> {
    match value["event"].as_str()? {
        "stage" => Some(Event::Stage(text(value, "name")?)),
        "stage_done" => Some(Event::Message(text(value, "message")?)),
        "progress" => Some(Event::Progress {
            label: text(value, "label").unwrap_or_default(),
            done: value["done"].as_u64().unwrap_or(0),
            total: value["total"].as_u64().unwrap_or(0),
        }),
        "message" => {
            let kind = value["kind"].as_str().unwrap_or("info");
            // `detail` is the terminal's indented commentary; in a dialog
            // it is noise.
            if kind == "detail" {
                return None;
            }
            Some(Event::Message(text(value, "text")?))
        }
        "failed" => Some(Event::Failed(
            text(value, "message").unwrap_or_else(|| "rvn failed".into()),
        )),
        _ => None,
    }
}

fn text(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .map(|s| strip_ansi(s).trim().to_string())
}

/// rvn colours its terminal output; a dialog shows the escape codes as
/// literal characters unless they come out.
///
/// A CSI sequence is `ESC [`, then parameter and intermediate bytes, then
/// one final byte in `@`..=`~`. The `[` is itself inside that range, so it
/// has to be consumed before the scan for the final byte starts — looking
/// for the terminator straight after the ESC finds the `[` and leaves the
/// colour numbers in the text.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            // Any other escape is two characters; drop both.
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// stderr is makepkg's output, mostly. Every line goes to the log.
fn read_log(stderr: std::process::ChildStderr, sender: Sender<Event>) {
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        let line = strip_ansi(&line).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if sender.send(Event::Message(line)).is_err() {
            return;
        }
    }
}

/// Opens rvn in a terminal, for a person who would rather watch it there.
pub fn open_in_terminal(packages: &[String]) -> Result<(), String> {
    let terminal = ["raven-terminal", "foot", "alacritty", "kitty", "xterm"]
        .into_iter()
        .find_map(which)
        .ok_or("No terminal emulator is installed")?;
    let command = format!("sudo rvn install {}", packages.join(" "));
    Command::new(terminal)
        .arg("-e")
        .args([
            "sh",
            "-c",
            &format!("{command}; echo; read -r -p 'Press Enter to close…' _"),
        ])
        .stdin(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open a terminal: {e}"))
}

/// Whether a path is inside the user's own cache, which is the only place
/// this app will delete anything from.
pub fn is_in_user_cache(path: &Path) -> bool {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return false;
    };
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"));
    let Ok(path) = path.canonicalize() else {
        return false;
    };
    let Ok(cache) = cache.canonicalize() else {
        return false;
    };
    path.starts_with(&cache) && path != cache
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rvn_events_become_dialog_events() {
        let stage = serde_json::json!({"event": "stage", "name": "Resolving dependencies"});
        assert!(
            matches!(translate(&stage), Some(Event::Stage(s)) if s == "Resolving dependencies")
        );

        let progress = serde_json::json!({
            "event": "progress", "label": "lib32-nvidia-utils", "done": 50u64, "total": 100u64
        });
        match translate(&progress) {
            Some(Event::Progress { label, done, total }) => {
                assert_eq!(label, "lib32-nvidia-utils");
                assert_eq!((done, total), (50, 100));
            }
            other => panic!("expected progress, got {other:?}"),
        }

        let failed = serde_json::json!({"event": "failed", "message": "target not found"});
        assert!(matches!(translate(&failed), Some(Event::Failed(m)) if m == "target not found"));
    }

    #[test]
    fn terminal_only_noise_is_dropped() {
        let detail = serde_json::json!({"event": "message", "kind": "detail", "text": "  -> x"});
        assert!(translate(&detail).is_none());
        let unknown = serde_json::json!({"event": "something-new"});
        assert!(translate(&unknown).is_none());
        assert!(translate(&serde_json::json!({})).is_none());
    }

    #[test]
    fn colour_codes_do_not_reach_the_dialog() {
        assert_eq!(strip_ansi("\u{1b}[1;32minstalled\u{1b}[0m"), "installed");
        assert_eq!(strip_ansi("plain"), "plain");
        // An unterminated escape swallows the rest rather than printing it.
        assert_eq!(strip_ansi("a\u{1b}["), "a");
        assert_eq!(strip_ansi("a\u{1b}[0;1;38;5;208mb\u{1b}[0m"), "ab");
        // A lone ESC at the end of a line leaves the text before it alone.
        assert_eq!(strip_ansi("done\u{1b}"), "done");
    }

    #[test]
    fn only_real_package_names_are_passed_on() {
        for name in ["lib32-nvidia-utils", "mesa", "gcc-libs+", "a.b_c@1"] {
            assert!(validate_package_name(name).is_ok(), "rejected {name}");
        }
        for name in ["", "-rf", "../etc", "a b", "x;rm -rf /", &"x".repeat(200)] {
            assert!(validate_package_name(name).is_err(), "accepted {name:?}");
        }
    }

    #[test]
    fn installing_nothing_is_refused_before_rvn_is_started() {
        assert!(install(&[]).is_err());
    }

    #[test]
    fn only_paths_inside_the_cache_may_be_cleared() {
        // The cache itself is not deletable, only things under it.
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap();
        assert!(!is_in_user_cache(&home));
        assert!(!is_in_user_cache(Path::new("/")));
        assert!(!is_in_user_cache(Path::new("/etc")));
        let cache = home.join(".cache");
        if cache.is_dir() {
            assert!(!is_in_user_cache(&cache));
        }
        // A path that does not exist cannot be canonicalised, so it is not
        // in the cache either — nothing is deleted on a guess.
        assert!(!is_in_user_cache(&cache.join("definitely-not-here-4a9f")));
    }
}
