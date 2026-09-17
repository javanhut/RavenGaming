//! Recording gameplay, and turning a recording into a video.
//!
//! # Who does the recording
//!
//! Huginn does. It offers no capture protocol — it holds the framebuffer
//! alone, so the one process that can record the screen is the one drawing
//! it — and `Super`+`Print` starts and stops a recording of the focused
//! screen. That is deliberate, and it means this app does not record
//! anything itself and could not if it wanted to.
//!
//! What it does instead is the part the compositor leaves out: recordings
//! land in `~/Videos/Recordings` as `.rvr`, Raven's own lossless format,
//! which is cheap to write while a game is running and which no video
//! player opens. This page lists them, says how long each one is, and
//! turns the ones worth keeping into MP4 with `raven-export`.
//!
//! # Reading a recording without decoding it
//!
//! [`probe`] wants the size and length of a file, not its pixels. The
//! format puts a 20-byte header at the front and a 17-byte header on every
//! record, with the payload length in it — so the length of a recording is
//! found by walking record headers and seeking past the payloads, never
//! reading a frame. A three-gigabyte recording is probed in milliseconds
//! and the frames stay on disk.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime};

use crate::drivers::which;

/// The magic, version, and header sizes of the recording format, from
/// `raven-rec`. Duplicated rather than depended on: this app reads four
/// numbers out of a file header, and linking the compositor's crate for
/// that would tie a GTK app's build to the compositor's tree.
const MAGIC: &[u8; 6] = b"RVNREC";
const HEADER_LEN: u64 = 20;
const RECORD_HEADER_LEN: u64 = 17;
const KIND_END: u8 = 2;
const EXTENSION: &str = "rvr";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recording {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
    pub width: u32,
    pub height: u32,
    /// How long it runs. `None` when the file has no records at all.
    pub duration: Option<Duration>,
    /// Whether the end marker is there. A recording without one was cut
    /// short — the compositor was killed, or the machine lost power — and
    /// everything before the break is still good, which is worth saying
    /// rather than hiding.
    pub complete: bool,
    /// Whether an exported `.mp4` already sits beside it.
    pub exported: Option<PathBuf>,
}

impl Recording {
    pub fn name(&self) -> String {
        self.path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// "1920 × 1080 · 2m 14s · 340 MB".
    pub fn detail(&self) -> String {
        let mut parts = vec![format!("{} × {}", self.width, self.height)];
        if let Some(duration) = self.duration {
            parts.push(duration_text(duration));
        }
        parts.push(crate::tune::human_bytes(self.bytes));
        if !self.complete {
            parts.push("cut short".into());
        }
        parts.join(" · ")
    }

    /// Where an export of this recording goes.
    pub fn export_path(&self) -> PathBuf {
        self.path.with_extension("mp4")
    }
}

pub fn duration_text(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, (seconds % 3600) / 60),
    }
}

/// Where Huginn writes recordings: `<videos>/Recordings`, following
/// `XDG_VIDEOS_DIR` the same way the compositor does.
pub fn recordings_dir() -> PathBuf {
    videos_dir().join("Recordings")
}

fn videos_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if let Some(dir) = std::env::var_os("XDG_VIDEOS_DIR").map(PathBuf::from) {
        return dir;
    }
    // `user-dirs.dirs` is where the XDG directories actually live; the
    // environment variable is only set by a shell that sourced it.
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    if let Ok(text) = fs::read_to_string(config.join("user-dirs.dirs"))
        && let Some(dir) = parse_user_dir(&text, "XDG_VIDEOS_DIR", &home)
    {
        return dir;
    }
    home.join("Videos")
}

/// One line of `user-dirs.dirs`: `XDG_VIDEOS_DIR="$HOME/Videos"`.
fn parse_user_dir(text: &str, key: &str, home: &Path) -> Option<PathBuf> {
    let line = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .find(|l| l.starts_with(key))?;
    let value = line.split_once('=')?.1.trim().trim_matches('"');
    let path = value
        .strip_prefix("$HOME/")
        .map(|rest| home.join(rest))
        .unwrap_or_else(|| PathBuf::from(value));
    (!value.is_empty()).then_some(path)
}

/// Every recording in the folder, newest first.
pub fn recordings() -> Vec<Recording> {
    let dir = recordings_dir();
    let mut found: Vec<Recording> = fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == EXTENSION))
        .filter_map(|path| probe(&path))
        .collect();
    found.sort_by(|a, b| b.modified.cmp(&a.modified).then(a.path.cmp(&b.path)));
    found
}

/// Reads a recording's header and walks its record headers.
///
/// `None` when the file is not a recording at all, so a stray file in the
/// folder is left out rather than shown as a zero-by-zero recording.
pub fn probe(path: &Path) -> Option<Recording> {
    let metadata = fs::metadata(path).ok()?;
    let mut file = fs::File::open(path).ok()?;
    let mut header = [0u8; HEADER_LEN as usize];
    file.read_exact(&mut header).ok()?;
    if &header[..6] != MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(header[8..12].try_into().ok()?);
    let height = u32::from_le_bytes(header[12..16].try_into().ok()?);
    let (duration, complete) = walk_records(&mut file, metadata.len());
    let exported = {
        let mp4 = path.with_extension("mp4");
        mp4.exists().then_some(mp4)
    };
    Some(Recording {
        path: path.to_path_buf(),
        bytes: metadata.len(),
        modified: metadata.modified().ok(),
        width,
        height,
        duration,
        complete,
        exported,
    })
}

/// Walks every record header, seeking past the payloads, and returns the
/// last presentation timestamp and whether the end marker was reached.
fn walk_records(file: &mut fs::File, file_len: u64) -> (Option<Duration>, bool) {
    let mut offset = HEADER_LEN;
    let mut last_pts = None;
    let mut complete = false;
    let mut header = [0u8; RECORD_HEADER_LEN as usize];
    while offset + RECORD_HEADER_LEN <= file_len {
        if file.seek(SeekFrom::Start(offset)).is_err() || file.read_exact(&mut header).is_err() {
            break;
        }
        let kind = header[0];
        let Ok(pts_bytes) = header[1..9].try_into() else {
            break;
        };
        let pts = u64::from_le_bytes(pts_bytes);
        let Ok(len_bytes) = header[9..13].try_into() else {
            break;
        };
        let length = u32::from_le_bytes(len_bytes) as u64;
        last_pts = Some(pts);
        if kind == KIND_END {
            complete = true;
            break;
        }
        let Some(next) = offset
            .checked_add(RECORD_HEADER_LEN)
            .and_then(|o| o.checked_add(length))
        else {
            break;
        };
        if next <= offset {
            // A length that does not advance would spin here forever.
            break;
        }
        offset = next;
    }
    (last_pts.map(Duration::from_micros), complete)
}

// ---- exporting -----------------------------------------------------------

/// Whether `raven-export` is installed. It ships with the compositor, and
/// a system that has Huginn but not the exporter can still record — it
/// just cannot turn a recording into a video, which the page says plainly
/// rather than offering a button that fails.
pub fn exporter_available() -> bool {
    which("raven-export").is_some()
}

/// What an export reports while it runs.
#[derive(Debug, Clone)]
pub enum ExportEvent {
    /// A line of the exporter's own output, for the log.
    Log(String),
    /// Frames written so far, and the seconds of video they make up.
    Progress {
        frames: u64,
        seconds: f64,
    },
    Finished {
        ok: bool,
        message: String,
    },
}

/// Starts an export and returns the channel its progress arrives on.
///
/// `raven-export` writes its progress to stderr with a carriage return, as
/// a terminal program does, so the reader splits on both `\r` and `\n` —
/// reading lines alone would deliver the whole progress display as one
/// line when the export finished, which is no progress at all.
pub fn export(recording: &Path, output: &Path) -> Result<Receiver<ExportEvent>, String> {
    let exporter = which("raven-export").ok_or(
        "raven-export is not installed. It ships with the Raven desktop; install it to turn recordings into video.",
    )?;
    let mut child = Command::new(exporter)
        .arg(recording)
        .arg("-o")
        .arg(output)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start raven-export: {e}"))?;

    let (sender, receiver) = mpsc::channel();
    let stderr = child.stderr.take().expect("stderr piped");
    let reader = {
        let sender = sender.clone();
        std::thread::spawn(move || read_export_output(stderr, sender))
    };
    std::thread::spawn(move || {
        let _ = reader.join();
        let (ok, message) = match child.wait() {
            Ok(status) if status.success() => (true, "Exported".to_string()),
            Ok(status) => (false, format!("raven-export exited with {status}")),
            Err(e) => (false, format!("raven-export could not be waited for: {e}")),
        };
        let _ = sender.send(ExportEvent::Finished { ok, message });
    });
    Ok(receiver)
}

fn read_export_output(stderr: std::process::ChildStderr, sender: mpsc::Sender<ExportEvent>) {
    let mut stderr = stderr;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 512];
    while let Ok(read) = stderr.read(&mut chunk) {
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        while let Some(end) = buffer.iter().position(|b| *b == b'\n' || *b == b'\r') {
            let line: Vec<u8> = buffer.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line[..line.len() - 1])
                .trim()
                .to_string();
            if text.is_empty() {
                continue;
            }
            let event = parse_progress(&text)
                .map(|(frames, seconds)| ExportEvent::Progress { frames, seconds })
                .unwrap_or_else(|| ExportEvent::Log(text));
            if sender.send(event).is_err() {
                return;
            }
        }
    }
    if !buffer.is_empty() {
        let text = String::from_utf8_lossy(&buffer).trim().to_string();
        if !text.is_empty() {
            let _ = sender.send(ExportEvent::Log(text));
        }
    }
}

/// `raven-export`'s progress line: "1234 frames, 41.1 s of video".
fn parse_progress(line: &str) -> Option<(u64, f64)> {
    let (frames, rest) = line.split_once(" frames, ")?;
    let seconds = rest.strip_suffix(" s of video")?;
    Some((frames.trim().parse().ok()?, seconds.trim().parse().ok()?))
}

/// Opens a folder or file in whatever the desktop uses.
pub fn open_in_file_manager(path: &Path) -> Result<(), String> {
    let opener = which("raven-files")
        .or_else(|| which("xdg-open"))
        .ok_or("No file manager is available to open this")?;
    Command::new(opener)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Builds a recording in memory: header, then the records given as
    /// `(kind, pts, payload length)`.
    fn recording_bytes(width: u32, height: u32, records: &[(u8, u64, u32)]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&width.to_le_bytes());
        bytes.extend_from_slice(&height.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(bytes.len() as u64, HEADER_LEN);
        for (kind, pts, length) in records {
            bytes.push(*kind);
            bytes.extend_from_slice(&pts.to_le_bytes());
            bytes.extend_from_slice(&length.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes()); // crc, unread here
            bytes.extend(std::iter::repeat_n(0u8, *length as usize));
        }
        bytes
    }

    fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("raven-gaming-test-{name}"));
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn a_finished_recording_reports_its_length() {
        let bytes = recording_bytes(
            1920,
            1080,
            &[(0, 0, 8), (1, 1_000_000, 4), (KIND_END, 2_500_000, 0)],
        );
        let path = write_temp("finished.rvr", &bytes);
        let r = probe(&path).unwrap();
        assert_eq!((r.width, r.height), (1920, 1080));
        assert_eq!(r.duration, Some(Duration::from_micros(2_500_000)));
        assert!(r.complete);
        assert_eq!(r.export_path().extension().unwrap(), "mp4");
        fs::remove_file(path).ok();
    }

    #[test]
    fn a_recording_cut_short_still_reports_what_it_has() {
        let bytes = recording_bytes(800, 600, &[(0, 0, 4), (1, 3_000_000, 4)]);
        let path = write_temp("cutshort.rvr", &bytes);
        let r = probe(&path).unwrap();
        assert_eq!(r.duration, Some(Duration::from_micros(3_000_000)));
        assert!(!r.complete);
        assert!(r.detail().contains("cut short"));
        fs::remove_file(path).ok();
    }

    #[test]
    fn a_file_that_is_not_a_recording_is_not_one() {
        let path = write_temp("bogus.rvr", b"not a recording at all, just bytes");
        assert!(probe(&path).is_none());
        fs::remove_file(path).ok();
    }

    #[test]
    fn a_truncated_header_is_not_a_recording() {
        let path = write_temp("short.rvr", b"RVNREC");
        assert!(probe(&path).is_none());
        fs::remove_file(path).ok();
    }

    #[test]
    fn a_recording_with_no_records_has_no_length() {
        let path = write_temp("empty.rvr", &recording_bytes(640, 480, &[]));
        let r = probe(&path).unwrap();
        assert_eq!(r.duration, None);
        assert!(!r.complete);
        fs::remove_file(path).ok();
    }

    #[test]
    fn a_record_length_that_runs_past_the_file_ends_the_walk() {
        // A damaged length must not loop or read forever.
        let mut bytes = recording_bytes(640, 480, &[(0, 500_000, 0)]);
        // Rewrite that record's length to something enormous.
        let length_at = HEADER_LEN as usize + 9;
        bytes[length_at..length_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let path = write_temp("damaged.rvr", &bytes);
        let r = probe(&path).unwrap();
        assert_eq!(r.duration, Some(Duration::from_micros(500_000)));
        assert!(!r.complete);
        fs::remove_file(path).ok();
    }

    #[test]
    fn durations_read_the_way_people_say_them() {
        assert_eq!(duration_text(Duration::from_secs(9)), "9s");
        assert_eq!(duration_text(Duration::from_secs(134)), "2m 14s");
        assert_eq!(
            duration_text(Duration::from_secs(3 * 3600 + 5 * 60)),
            "3h 05m"
        );
    }

    #[test]
    fn the_exporters_progress_line_is_recognised() {
        assert_eq!(
            parse_progress("1234 frames, 41.1 s of video"),
            Some((1234, 41.1))
        );
        assert_eq!(parse_progress("wrote talk.mp4"), None);
        assert_eq!(parse_progress(""), None);
    }

    #[test]
    fn the_videos_folder_comes_from_user_dirs() {
        let home = Path::new("/home/someone");
        let text =
            "# comment\nXDG_VIDEOS_DIR=\"$HOME/Media/Video\"\nXDG_MUSIC_DIR=\"$HOME/Music\"\n";
        assert_eq!(
            parse_user_dir(text, "XDG_VIDEOS_DIR", home),
            Some(PathBuf::from("/home/someone/Media/Video"))
        );
        assert_eq!(
            parse_user_dir("XDG_VIDEOS_DIR=\"/mnt/video\"\n", "XDG_VIDEOS_DIR", home),
            Some(PathBuf::from("/mnt/video"))
        );
        assert_eq!(parse_user_dir("", "XDG_VIDEOS_DIR", home), None);
        // A commented-out line is not a setting.
        assert_eq!(
            parse_user_dir("#XDG_VIDEOS_DIR=\"/x\"\n", "XDG_VIDEOS_DIR", home),
            None
        );
    }
}
