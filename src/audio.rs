//! Game audio: the devices, and the one setting that decides whether the
//! sound arrives when the picture does.
//!
//! # Why latency is the whole page
//!
//! A desktop wants a big audio buffer, because a big buffer never
//! underruns and nobody notices twenty milliseconds on a notification. A
//! game wants a small one, because twenty milliseconds is the gap between
//! the gun firing and the bang, and in a rhythm game it is the difference
//! between hitting the note and missing it. PipeWire's default of 1024
//! frames is the desktop's answer, and this page is where it can be given
//! the game's one.
//!
//! The cost is real and is stated plainly in the UI: a buffer too small
//! for the machine crackles. That is why the change is applied live first,
//! so it can be heard and undone, and only written to disk on top of that.
//!
//! # How it talks to PipeWire
//!
//! Through `pw-metadata` and `pw-dump`, the tools PipeWire ships, rather
//! than by linking libpipewire. The same reason Raven Store drives `rvn`
//! as a process: one code path that a person can also run in a terminal
//! and see the same answer from.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use crate::drivers::which;

/// Where a persisted change goes. One file, owned by this app, in the
/// user's own config — no root, and deleting it restores PipeWire's
/// defaults exactly.
const CONFIG_FILE: &str = "pipewire.conf.d/99-raven-gaming.conf";

/// The buffer sizes offered, in frames.
///
/// Powers of two from PipeWire's own minimum to its default. 1024 is what
/// a desktop ships with; 256 is the usual sweet spot for a game on
/// hardware from the last decade; 64 is for people who know what they are
/// doing and have somewhere to put the blame.
pub const QUANTA: [u32; 5] = [1024, 512, 256, 128, 64];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Latency {
    pub quantum: u32,
    pub rate: u32,
}

impl Latency {
    /// The buffer's length in milliseconds, which is the number that
    /// actually means something to a person.
    pub fn milliseconds(self) -> Option<f64> {
        (self.rate > 0 && self.quantum > 0).then(|| self.quantum as f64 / self.rate as f64 * 1000.0)
    }

    pub fn text(self) -> String {
        match self.milliseconds() {
            Some(ms) => format!("{} frames · {ms:.1} ms", self.quantum),
            None => "—".into(),
        }
    }

    /// Whether this is low enough that a game will feel in sync.
    ///
    /// Ten milliseconds is about where a gunshot stops sounding late. It
    /// is a judgement, not a measurement, and the UI words it as one.
    pub fn is_good_for_games(self) -> bool {
        self.milliseconds().is_some_and(|ms| ms <= 10.5)
    }
}

/// What PipeWire is doing right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Engine {
    pub running: bool,
    pub version: Option<String>,
    pub rate: u32,
    pub quantum: u32,
    /// Non-zero when something has pinned the buffer, which is what this
    /// app does.
    pub forced_quantum: u32,
    pub forced_rate: u32,
    pub min_quantum: u32,
    pub max_quantum: u32,
}

impl Engine {
    pub fn read() -> Engine {
        let Some(settings) = metadata("settings") else {
            return Engine::default();
        };
        let number = |key: &str| -> u32 {
            settings
                .get(key)
                .and_then(|v| v.parse().ok())
                .unwrap_or_default()
        };
        Engine {
            running: true,
            version: version(),
            rate: number("clock.rate"),
            quantum: number("clock.quantum"),
            forced_quantum: number("clock.force-quantum"),
            forced_rate: number("clock.force-rate"),
            min_quantum: number("clock.min-quantum"),
            max_quantum: number("clock.max-quantum"),
        }
    }

    /// The buffer actually in use: the forced one when there is one.
    pub fn latency(&self) -> Latency {
        Latency {
            quantum: if self.forced_quantum > 0 {
                self.forced_quantum
            } else {
                self.quantum
            },
            rate: if self.forced_rate > 0 {
                self.forced_rate
            } else {
                self.rate
            },
        }
    }
}

fn version() -> Option<String> {
    let out = Command::new(which("pw-cli")?)
        .args(["info", "0"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|line| line.trim().strip_prefix("core.version = "))
        .map(|v| v.trim_matches('"').to_string())
}

/// Reads one of PipeWire's metadata stores into a map.
fn metadata(store: &str) -> Option<BTreeMap<String, String>> {
    let out = Command::new(which("pw-metadata")?)
        .args(["-n", store])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_metadata(&String::from_utf8_lossy(&out.stdout)))
}

/// `update: id:0 key:'clock.rate' value:'48000' type:''`
fn parse_metadata(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.split_once("key:'") else {
            continue;
        };
        let Some((key, rest)) = rest.1.split_once('\'') else {
            continue;
        };
        let Some(rest) = rest.split_once("value:'") else {
            continue;
        };
        let Some((value, _)) = rest.1.split_once('\'') else {
            continue;
        };
        map.insert(key.to_string(), value.to_string());
    }
    map
}

// ---- devices -------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: u32,
    /// What PipeWire calls it internally; the default is recorded by name.
    pub name: String,
    /// What to show a person.
    pub description: String,
    pub is_output: bool,
    pub is_default: bool,
    /// `alsa`, `bluez5`, `v4l2`…
    pub api: String,
}

/// Every audio sink and source PipeWire knows about.
pub fn devices() -> Vec<Device> {
    let Some(dump) = which("pw-dump") else {
        return Vec::new();
    };
    let Ok(out) = Command::new(dump).output() else {
        return Vec::new();
    };
    let defaults = metadata("default").unwrap_or_default();
    let default_sink = json_name(defaults.get("default.audio.sink"));
    let default_source = json_name(defaults.get("default.audio.source"));
    parse_dump(
        &String::from_utf8_lossy(&out.stdout),
        default_sink.as_deref(),
        default_source.as_deref(),
    )
}

/// The default is recorded as `{"name":"alsa_output.…"}`.
fn json_name(value: Option<&String>) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(value?).ok()?;
    parsed["name"].as_str().map(String::from)
}

fn parse_dump(json: &str, default_sink: Option<&str>, default_source: Option<&str>) -> Vec<Device> {
    let Ok(objects) = serde_json::from_str::<Vec<serde_json::Value>>(json) else {
        return Vec::new();
    };
    let mut devices = Vec::new();
    for object in objects {
        if object["type"] != "PipeWire:Interface:Node" {
            continue;
        }
        let props = &object["info"]["props"];
        let class = props["media.class"].as_str().unwrap_or_default();
        let is_output = match class {
            "Audio/Sink" => true,
            "Audio/Source" => false,
            _ => continue,
        };
        let name = props["node.name"].as_str().unwrap_or_default().to_string();
        // Monitor sources are the loopback of a sink, not a microphone.
        // They belong in a routing tool, not here.
        if !is_output && name.ends_with(".monitor") {
            continue;
        }
        let default_name = if is_output {
            default_sink
        } else {
            default_source
        };
        devices.push(Device {
            id: object["id"].as_u64().unwrap_or_default() as u32,
            is_default: default_name.is_some_and(|d| d == name),
            description: props["node.description"]
                .as_str()
                .or_else(|| props["node.nick"].as_str())
                .unwrap_or(&name)
                .to_string(),
            api: props["device.api"].as_str().unwrap_or("").to_string(),
            name,
            is_output,
        });
    }
    devices.sort_by(|a, b| {
        b.is_output
            .cmp(&a.is_output)
            .then(b.is_default.cmp(&a.is_default))
            .then(a.description.cmp(&b.description))
    });
    devices
}

/// Makes a device the one games will use.
pub fn set_default(device: &Device) -> Result<(), String> {
    let wpctl = which("wpctl").ok_or("wpctl is not installed")?;
    let status = Command::new(wpctl)
        .args(["set-default", &device.id.to_string()])
        .status()
        .map_err(|e| format!("Could not run wpctl: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "PipeWire would not switch to {}",
            device.description
        ))
    }
}

// ---- changing the buffer -------------------------------------------------

/// Applies a buffer size to the running server.
///
/// Live first, and only then written to disk: a buffer too small for the
/// machine crackles, and that has to be audible and undoable before it
/// becomes the setting this computer boots with.
pub fn apply_quantum(quantum: u32) -> Result<(), String> {
    if quantum != 0 && !QUANTA.contains(&quantum) {
        return Err(format!("Not a buffer size this app offers: {quantum}"));
    }
    set_metadata("clock.force-quantum", quantum)
}

pub fn apply_rate(rate: u32) -> Result<(), String> {
    if rate != 0 && !RATES.contains(&rate) {
        return Err(format!("Not a sample rate this app offers: {rate}"));
    }
    set_metadata("clock.force-rate", rate)
}

/// The sample rates offered. 48 kHz is what games and PipeWire both
/// assume; 44.1 is what older games and CD audio use.
pub const RATES: [u32; 4] = [48_000, 44_100, 96_000, 192_000];

/// The arguments that set one key in the `settings` store.
///
/// The `0` is the subject id and is not optional. Without it `pw-metadata`
/// reads the key as the subject, treats the whole thing as a query, prints
/// the store's name and exits *successfully* having changed nothing — so
/// the buttons on this page appear to work and do not.
fn set_args(key: &str, value: u32) -> [String; 5] {
    [
        "-n".into(),
        "settings".into(),
        "0".into(),
        key.into(),
        value.to_string(),
    ]
}

fn set_metadata(key: &str, value: u32) -> Result<(), String> {
    let binary = which("pw-metadata").ok_or("pw-metadata is not installed")?;
    let output = Command::new(binary)
        .args(set_args(key, value))
        .output()
        .map_err(|e| format!("Could not run pw-metadata: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("PipeWire refused the change")
            .trim()
            .to_string());
    }
    // A successful exit is not proof: a malformed command line makes
    // pw-metadata print the store and exit zero. It echoes a `set
    // property:` line when it has actually written something.
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.contains("set property:") {
        Ok(())
    } else {
        Err(format!("PipeWire did not accept {key}"))
    }
}

fn config_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("pipewire")
        .join(CONFIG_FILE)
}

pub fn is_persisted() -> bool {
    config_path().exists()
}

/// Writes the current choice so it survives a reboot. No root: PipeWire
/// reads this out of the user's own config directory.
pub fn persist(quantum: u32, rate: u32) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&path, config_text(quantum, rate))
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn config_text(quantum: u32, rate: u32) -> String {
    let mut text = String::from(
        "# Written by Raven Gaming. Delete this file, or use Restore in the\n\
         # app, to go back to PipeWire's own settings.\n\
         context.properties = {\n",
    );
    if quantum > 0 {
        text.push_str(&format!("    default.clock.quantum       = {quantum}\n"));
        // A forced quantum outside the allowed range is ignored, so the
        // range has to move with it.
        text.push_str(&format!(
            "    default.clock.min-quantum   = {}\n",
            quantum.min(32)
        ));
    }
    if rate > 0 {
        text.push_str(&format!("    default.clock.rate          = {rate}\n"));
        text.push_str(&format!("    default.clock.allowed-rates = [ {rate} ]\n"));
    }
    text.push_str("}\n");
    text
}

/// Undoes everything: the live override and the file.
pub fn restore() -> Result<(), String> {
    let path = config_path();
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", path.display())),
    }
    set_metadata("clock.force-quantum", 0)?;
    set_metadata("clock.force-rate", 0)
}

/// Whether the 32-bit audio libraries are installed.
///
/// Its own check because the failure is so specific: a 32-bit or Proton
/// game with no `lib32` PipeWire runs perfectly and in total silence, and
/// nothing in the game's own settings explains why.
pub fn has_32bit_audio() -> bool {
    ["/usr/lib32", "/usr/lib/i386-linux-gnu"].iter().any(|dir| {
        let dir = std::path::Path::new(dir);
        dir.join("libpulse.so.0").exists()
            || dir
                .join("pipewire-0.3/libpipewire-module-protocol-pulse.so")
                .exists()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipewire_metadata_is_read_key_by_key() {
        let text = "Found \"settings\" metadata 31\n\
            update: id:0 key:'clock.rate' value:'48000' type:''\n\
            update: id:0 key:'clock.quantum' value:'1024' type:''\n\
            update: id:0 key:'clock.force-quantum' value:'0' type:''\n";
        let map = parse_metadata(text);
        assert_eq!(map.get("clock.rate").map(String::as_str), Some("48000"));
        assert_eq!(map.get("clock.quantum").map(String::as_str), Some("1024"));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn metadata_noise_is_ignored() {
        assert!(parse_metadata("").is_empty());
        assert!(parse_metadata("Found \"settings\" metadata 31\n").is_empty());
        assert!(parse_metadata("update: nonsense\n").is_empty());
    }

    #[test]
    fn a_forced_buffer_is_the_one_in_use() {
        let engine = Engine {
            running: true,
            rate: 48_000,
            quantum: 1024,
            forced_quantum: 256,
            ..Engine::default()
        };
        assert_eq!(engine.latency().quantum, 256);
        // With nothing forced, the server's own value is the answer.
        let engine = Engine {
            forced_quantum: 0,
            ..engine
        };
        assert_eq!(engine.latency().quantum, 1024);
    }

    #[test]
    fn latency_is_reported_in_milliseconds() {
        let desktop = Latency {
            quantum: 1024,
            rate: 48_000,
        };
        assert!((desktop.milliseconds().unwrap() - 21.33).abs() < 0.01);
        assert!(!desktop.is_good_for_games());
        let game = Latency {
            quantum: 256,
            rate: 48_000,
        };
        assert!((game.milliseconds().unwrap() - 5.33).abs() < 0.01);
        assert!(game.is_good_for_games());
        assert!(game.text().contains("5.3 ms"));
    }

    #[test]
    fn a_server_that_said_nothing_reports_nothing() {
        let silent = Latency {
            quantum: 0,
            rate: 0,
        };
        assert_eq!(silent.milliseconds(), None);
        assert_eq!(silent.text(), "—");
        assert!(!silent.is_good_for_games());
        assert!(!Engine::default().running);
    }

    #[test]
    fn a_metadata_write_names_its_subject() {
        // The regression this guards: without the "0", pw-metadata reads
        // the key as a subject id, does nothing, and exits successfully.
        let args = set_args("clock.force-quantum", 256);
        assert_eq!(args[0], "-n");
        assert_eq!(args[1], "settings");
        assert_eq!(args[2], "0", "the subject id is not optional");
        assert_eq!(args[3], "clock.force-quantum");
        assert_eq!(args[4], "256");
    }

    #[test]
    fn only_offered_values_are_ever_sent_to_pipewire() {
        // The numbers come from this app's own tables, so anything else is
        // a bug here and should be loud rather than handed to the server.
        assert!(apply_quantum(7).is_err());
        assert!(apply_rate(12_345).is_err());
        for quantum in QUANTA {
            assert!(!format!("{quantum}").is_empty());
        }
    }

    #[test]
    fn a_dump_gives_up_its_sinks_and_sources() {
        let json = r#"[
          {"id":54,"type":"PipeWire:Interface:Node","info":{"props":{
            "media.class":"Audio/Sink","node.name":"alsa_output.pci-1.analog-stereo",
            "node.description":"Ryzen HD Audio Analog Stereo","device.api":"alsa"}}},
          {"id":55,"type":"PipeWire:Interface:Node","info":{"props":{
            "media.class":"Audio/Source","node.name":"alsa_input.pci-1.analog-stereo",
            "node.description":"Headset Mic","device.api":"alsa"}}},
          {"id":56,"type":"PipeWire:Interface:Node","info":{"props":{
            "media.class":"Audio/Source","node.name":"alsa_output.pci-1.analog-stereo.monitor",
            "node.description":"Monitor of Analog","device.api":"alsa"}}},
          {"id":57,"type":"PipeWire:Interface:Device","info":{"props":{}}}
        ]"#;
        let devices = parse_dump(json, Some("alsa_output.pci-1.analog-stereo"), None);
        assert_eq!(devices.len(), 2, "the monitor source is not a microphone");
        assert!(devices[0].is_output);
        assert!(devices[0].is_default);
        assert_eq!(devices[0].description, "Ryzen HD Audio Analog Stereo");
        assert!(!devices[1].is_output);
        assert!(!devices[1].is_default);
    }

    #[test]
    fn a_dump_that_is_not_json_is_no_devices() {
        assert!(parse_dump("not json", None, None).is_empty());
        assert!(parse_dump("[]", None, None).is_empty());
    }

    #[test]
    fn the_default_device_is_read_out_of_its_json() {
        let value = r#"{"name":"alsa_output.pci-0000_05_00.6.analog-stereo"}"#.to_string();
        assert_eq!(
            json_name(Some(&value)).as_deref(),
            Some("alsa_output.pci-0000_05_00.6.analog-stereo")
        );
        assert_eq!(json_name(None), None);
        assert_eq!(json_name(Some(&"not json".to_string())), None);
    }

    #[test]
    fn the_written_config_says_what_was_chosen() {
        let text = config_text(256, 48_000);
        assert!(text.contains("default.clock.quantum       = 256"));
        assert!(text.contains("default.clock.rate          = 48000"));
        assert!(text.contains("allowed-rates = [ 48000 ]"));
        assert!(text.contains("Raven Gaming"));
        // Nothing chosen means nothing written for it.
        let only_quantum = config_text(512, 0);
        assert!(only_quantum.contains("quantum       = 512"));
        assert!(!only_quantum.contains("clock.rate"));
    }
}
