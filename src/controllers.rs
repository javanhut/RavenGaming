//! Game controllers: what is plugged in, whether the system will let a
//! game see it, and a way to prove every button works.
//!
//! # Why `/proc/bus/input/devices` and not libudev
//!
//! The kernel already publishes everything this page needs in one text
//! file: the name, the bus, the vendor and product ids, which event and
//! joystick nodes it owns, and the capability bitmaps that say whether a
//! thing is a gamepad at all. Reading it needs no library, no daemon, and
//! no permissions beyond `/proc`.
//!
//! # Telling a gamepad from a keyboard
//!
//! Not by name — "Microsoft X-Box 360 pad" and "8BitDo Ultimate" have
//! nothing in common, and half the things that call themselves controllers
//! are really keyboards. The kernel's own answer is the `KEY` bitmap: the
//! codes from `BTN_GAMEPAD` (0x130) up are the face buttons, and only a
//! gamepad claims them. A joystick or wheel claims `BTN_TRIGGER` (0x120)
//! instead. Both are checked; nothing else is guessed.
//!
//! # The tester
//!
//! Opening `/dev/input/eventN` and reading it is how every game sees a
//! controller, so if the tester can read it, a game can. That makes the
//! tester a real answer to "is it my controller or is it the game", which
//! is the question the page exists for.

use std::fs;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// The first face button. Everything from here to 0x13f is a gamepad
/// button, and claiming any of them is what makes a device a gamepad.
const BTN_GAMEPAD: usize = 0x130;
/// The first joystick button, for sticks, wheels and flight gear.
const BTN_JOYSTICK: usize = 0x120;
/// Absolute axes — sticks and triggers.
const EV_ABS: u32 = 0x03;
/// Force feedback: rumble.
const EV_FF: u32 = 0x15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Usb,
    Bluetooth,
    Virtual,
    Other,
}

impl Transport {
    /// The `BUS_*` constants from `input.h`.
    fn from_bus(bus: u16) -> Transport {
        match bus {
            0x03 => Transport::Usb,
            0x05 => Transport::Bluetooth,
            0x06 => Transport::Virtual,
            _ => Transport::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Transport::Usb => "USB",
            Transport::Bluetooth => "Bluetooth",
            Transport::Virtual => "Virtual",
            Transport::Other => "Other",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Controller {
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub transport: Transport,
    /// `/dev/input/eventN`, which is what a game opens.
    pub event: Option<PathBuf>,
    /// `/dev/input/jsN`, the old joystick interface some games still want.
    pub joystick: Option<PathBuf>,
    /// The kernel path, used to find the driver and the battery.
    pub sysfs: String,
    pub has_rumble: bool,
    /// Whether it is a full gamepad rather than a stick or a wheel.
    pub is_gamepad: bool,
    pub axes: u32,
    pub buttons: u32,
}

impl Controller {
    /// The kernel module driving it: `xpad`, `hid-playstation`,
    /// `hid-nintendo`, `hid-generic`. The last one is the interesting
    /// case — a controller on `hid-generic` works, but usually without its
    /// rumble, gyro or LEDs.
    pub fn driver(&self) -> Option<String> {
        let mut path = PathBuf::from("/sys").join(self.sysfs.trim_start_matches('/'));
        // Walk up from the input node until something has a driver.
        for _ in 0..6 {
            if let Ok(link) = fs::read_link(path.join("driver"))
                && let Some(name) = link.file_name()
            {
                return Some(name.to_string_lossy().into_owned());
            }
            if !path.pop() {
                break;
            }
        }
        None
    }

    /// Battery percentage, for a wireless pad that reports one.
    pub fn battery(&self) -> Option<u8> {
        peripheral_batteries()
            .into_iter()
            .find(|(path, _)| {
                // The battery lives beside the input node under the same
                // HID device, so one is a prefix of the other once both
                // are made absolute.
                let hid = Path::new("/sys").join(self.sysfs.trim_start_matches('/'));
                shared_hid_device(&hid, path)
            })
            .map(|(_, percent)| percent)
    }

    pub fn kind(&self) -> &'static str {
        if self.is_gamepad {
            "Gamepad"
        } else {
            "Stick or wheel"
        }
    }
}

/// Batteries belonging to a peripheral rather than to the laptop.
///
/// `scope` is the kernel's own way of saying which is which: `Device`
/// means the battery is in something plugged in, `System` means it powers
/// the computer. Without that check a controller page happily reports the
/// laptop's own battery as the gamepad's.
fn peripheral_batteries() -> Vec<(PathBuf, u8)> {
    fs::read_dir("/sys/class/power_supply")
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| crate::gpu::read_text(path.join("scope")).as_deref() == Some("Device"))
        .filter_map(|path| {
            let percent = crate::gpu::read_num(path.join("capacity"))? as u8;
            let real = fs::canonicalize(&path).unwrap_or(path);
            Some((real, percent))
        })
        .collect()
}

/// Whether an input node and a battery hang off the same HID device.
fn shared_hid_device(input: &Path, battery: &Path) -> bool {
    let hid_of = |path: &Path| -> Option<String> {
        path.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .find(|part| part.matches(':').count() == 2 && part.len() > 10)
    };
    match (hid_of(input), hid_of(battery)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Every controller the kernel can see.
pub fn discover() -> Vec<Controller> {
    let text = fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
    parse_devices(&text)
}

fn parse_devices(text: &str) -> Vec<Controller> {
    let mut controllers = Vec::new();
    for block in text.split("\n\n") {
        if let Some(controller) = parse_device(block) {
            controllers.push(controller);
        }
    }
    controllers
}

fn parse_device(block: &str) -> Option<Controller> {
    let mut bus = 0u16;
    let mut vendor = 0u16;
    let mut product = 0u16;
    let mut name = String::new();
    let mut sysfs = String::new();
    let mut handlers: Vec<String> = Vec::new();
    let mut ev = 0u32;
    let mut key_bits: Vec<u64> = Vec::new();
    let mut abs_bits: Vec<u64> = Vec::new();

    for line in block.lines() {
        let line = line.trim_end();
        match line.get(..3) {
            Some("I: ") => {
                for field in line[3..].split_whitespace() {
                    let Some((key, value)) = field.split_once('=') else {
                        continue;
                    };
                    let parsed = u16::from_str_radix(value, 16).unwrap_or(0);
                    match key {
                        "Bus" => bus = parsed,
                        "Vendor" => vendor = parsed,
                        "Product" => product = parsed,
                        _ => {}
                    }
                }
            }
            Some("N: ") => {
                name = line[3..]
                    .trim_start_matches("Name=")
                    .trim_matches('"')
                    .to_string();
            }
            Some("S: ") => sysfs = line[3..].trim_start_matches("Sysfs=").to_string(),
            Some("H: ") => {
                handlers = line[3..]
                    .trim_start_matches("Handlers=")
                    .split_whitespace()
                    .map(String::from)
                    .collect();
            }
            Some("B: ") => {
                let rest = &line[3..];
                if let Some(value) = rest.strip_prefix("EV=") {
                    ev = u32::from_str_radix(value.trim(), 16).unwrap_or(0);
                } else if let Some(value) = rest.strip_prefix("KEY=") {
                    key_bits = bitmap(value);
                } else if let Some(value) = rest.strip_prefix("ABS=") {
                    abs_bits = bitmap(value);
                }
            }
            _ => {}
        }
    }

    if name.is_empty() {
        return None;
    }
    let is_gamepad = (BTN_GAMEPAD..=BTN_GAMEPAD + 0x0f).any(|bit| bit_set(&key_bits, bit));
    let is_joystick = (BTN_JOYSTICK..=BTN_JOYSTICK + 0x0f).any(|bit| bit_set(&key_bits, bit));
    let has_axes = ev & (1 << EV_ABS) != 0;
    let has_js = handlers.iter().any(|h| h.starts_with("js"));
    // A gamepad claims face buttons; a stick or wheel claims trigger
    // buttons and axes. A `js` node alone is enough too — the kernel has
    // already made the same judgement to create one.
    if !(is_gamepad || (is_joystick && has_axes) || has_js) {
        return None;
    }

    let node = |prefix: &str| -> Option<PathBuf> {
        handlers
            .iter()
            .find(|h| {
                h.starts_with(prefix) && h[prefix.len()..].chars().all(|c| c.is_ascii_digit())
            })
            .map(|h| PathBuf::from("/dev/input").join(h))
    };
    Some(Controller {
        vendor,
        product,
        transport: Transport::from_bus(bus),
        event: node("event"),
        joystick: node("js"),
        has_rumble: ev & (1 << EV_FF) != 0,
        is_gamepad,
        axes: abs_bits.iter().map(|word| word.count_ones()).sum(),
        buttons: key_bits.iter().map(|word| word.count_ones()).sum(),
        name,
        sysfs,
    })
}

/// A kernel capability bitmap, printed high word first.
///
/// Reversed on the way in so index 0 holds bits 0..63, which is the order
/// every lookup here wants and the opposite of the order it is written in.
fn bitmap(text: &str) -> Vec<u64> {
    let mut words: Vec<u64> = text
        .split_whitespace()
        .map(|word| u64::from_str_radix(word, 16).unwrap_or(0))
        .collect();
    words.reverse();
    words
}

fn bit_set(bitmap: &[u64], bit: usize) -> bool {
    bitmap
        .get(bit / 64)
        .is_some_and(|word| word >> (bit % 64) & 1 == 1)
}

// ---- the system's side ---------------------------------------------------

/// Whether the udev rules that let a non-Steam game open a controller are
/// installed.
///
/// Without them a controller is owned by root and only Steam — which runs
/// its own helper — can use it. The symptom is a pad that works in Steam
/// and in nothing else, which nobody attributes to a missing package.
pub fn steam_input_rules() -> bool {
    [
        "/usr/lib/udev/rules.d",
        "/etc/udev/rules.d",
        "/lib/udev/rules.d",
    ]
    .iter()
    .any(|dir| {
        fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.contains("steam-input") || name.contains("steam-devices")
        })
    })
}

/// Whether this session may open an event node — which is what a game
/// does. Checked by actually opening one, because the group membership,
/// the udev rules and logind's own grant all have to line up and only the
/// open call knows whether they did.
pub fn can_read(controller: &Controller) -> bool {
    controller
        .event
        .as_ref()
        .is_some_and(|path| fs::File::open(path).is_ok())
}

// ---- the live tester -----------------------------------------------------

/// One thing a controller just did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A button went down or up. The code is the kernel's `BTN_*`.
    Button { code: u16, pressed: bool },
    /// An axis moved. The value is raw; [`Axis::normalise`] scales it.
    Axis { code: u16, value: i32 },
}

/// An open event node, read without blocking.
pub struct Reader {
    file: fs::File,
}

/// `struct input_event` on a 64-bit kernel: two 64-bit times, then type,
/// code and value.
const EVENT_SIZE: usize = 24;
const EV_KEY: u16 = 0x01;
const EV_ABS_TYPE: u16 = 0x03;

impl Reader {
    pub fn open(controller: &Controller) -> Result<Reader, String> {
        let path = controller
            .event
            .as_ref()
            .ok_or("This controller has no event device to read")?;
        let file = fs::File::open(path).map_err(|e| {
            format!(
                "Cannot open {}: {e}. Being in the `input` group, or having the steam-devices rules installed, is what grants this.",
                path.display()
            )
        })?;
        // Non-blocking: the tester is polled from the main loop, and a
        // blocking read there would freeze the window between presses.
        // SAFETY: the descriptor is owned by `file` and outlives the call.
        unsafe {
            let fd = file.as_raw_fd();
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err("Could not set the controller to non-blocking".into());
            }
        }
        Ok(Reader { file })
    }

    /// Everything that has happened since the last call. Empty when the
    /// controller has been still, which is the normal case.
    pub fn poll(&mut self) -> Vec<Input> {
        let mut events = Vec::new();
        let mut buffer = [0u8; EVENT_SIZE * 32];
        loop {
            let read = match self.file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                // Nothing waiting, which is what non-blocking means.
                Err(_) => break,
            };
            events.extend(parse_events(&buffer[..read]));
            if read < buffer.len() {
                break;
            }
        }
        events
    }
}

fn parse_events(bytes: &[u8]) -> Vec<Input> {
    let (records, _remainder) = bytes.as_chunks::<EVENT_SIZE>();
    records
        .iter()
        .filter_map(|chunk| {
            let kind = u16::from_ne_bytes(chunk[16..18].try_into().ok()?);
            let code = u16::from_ne_bytes(chunk[18..20].try_into().ok()?);
            let value = i32::from_ne_bytes(chunk[20..24].try_into().ok()?);
            match kind {
                // Value 2 is auto-repeat, which a controller does not do
                // and a keyboard does; treated as still held either way.
                EV_KEY => Some(Input::Button {
                    code,
                    pressed: value != 0,
                }),
                EV_ABS_TYPE => Some(Input::Axis { code, value }),
                _ => None,
            }
        })
        .collect()
}

/// The name of a button code, for the tester to show.
pub fn button_name(code: u16) -> String {
    match code {
        0x130 => "A / Cross".into(),
        0x131 => "B / Circle".into(),
        0x133 => "X / Square".into(),
        0x134 => "Y / Triangle".into(),
        0x136 => "Left bumper".into(),
        0x137 => "Right bumper".into(),
        0x138 => "Left trigger".into(),
        0x139 => "Right trigger".into(),
        0x13a => "Select".into(),
        0x13b => "Start".into(),
        0x13c => "Guide".into(),
        0x13d => "Left stick".into(),
        0x13e => "Right stick".into(),
        0x220 => "D-pad up".into(),
        0x221 => "D-pad down".into(),
        0x222 => "D-pad left".into(),
        0x223 => "D-pad right".into(),
        other => format!("Button {other:#x}"),
    }
}

pub fn axis_name(code: u16) -> String {
    match code {
        0x00 => "Left stick X".into(),
        0x01 => "Left stick Y".into(),
        0x02 => "Left trigger".into(),
        0x03 => "Right stick X".into(),
        0x04 => "Right stick Y".into(),
        0x05 => "Right trigger".into(),
        0x10 => "D-pad X".into(),
        0x11 => "D-pad Y".into(),
        other => format!("Axis {other:#x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real 360 pad's block, as the kernel prints it.
    ///
    /// `KEY=7cdb000000000000 0 0 0 0` is five 64-bit words, highest first,
    /// so the leading one is word 4 — bits 256..319. `0x7cdb` shifted up
    /// 48 places lands on 304, 305, 307, 308, 310, 311 and 314..318:
    /// A, B, X, Y, the two bumpers, select, start, guide and both sticks.
    /// Exactly a 360 pad, and the reason the bitmap has to be read from
    /// the far end rather than the near one.
    const XBOX: &str = "I: Bus=0003 Vendor=045e Product=028e Version=0114\n\
        N: Name=\"Microsoft X-Box 360 pad\"\n\
        P: Phys=usb-0000:00:14.0-3/input0\n\
        S: Sysfs=/devices/pci0000:00/0000:00:14.0/usb1/1-3/1-3:1.0/input/input20\n\
        U: Uniq=\n\
        H: Handlers=event20 js0 \n\
        B: PROP=0\n\
        B: EV=20000b\n\
        B: KEY=7cdb000000000000 0 0 0 0\n\
        B: ABS=3003f\n\
        B: FF=107030000 0";

    const KEYBOARD: &str = "I: Bus=0011 Vendor=0001 Product=0001 Version=ab83\n\
        N: Name=\"AT Translated Set 2 keyboard\"\n\
        S: Sysfs=/devices/platform/i8042/serio0/input/input5\n\
        H: Handlers=sysrq kbd leds event5 \n\
        B: EV=120013\n\
        B: KEY=402000000 3803078f800d001 feffffdfffefffff fffffffffffffffe\n\
        B: MSC=10\n\
        B: LED=7";

    #[test]
    fn a_gamepad_is_recognised_by_its_buttons() {
        let pad = parse_device(XBOX).expect("the 360 pad is a controller");
        assert_eq!(pad.name, "Microsoft X-Box 360 pad");
        assert_eq!((pad.vendor, pad.product), (0x045e, 0x028e));
        assert_eq!(pad.transport, Transport::Usb);
        assert_eq!(pad.event, Some(PathBuf::from("/dev/input/event20")));
        assert_eq!(pad.joystick, Some(PathBuf::from("/dev/input/js0")));
        assert!(pad.has_rumble);
        assert!(pad.is_gamepad);
        assert_eq!(pad.kind(), "Gamepad");
        // Six sticks and triggers, plus the two hat axes.
        assert_eq!(pad.axes, 8, "a 360 pad has eight axes");
        assert_eq!(pad.buttons, 11);
    }

    #[test]
    fn a_keyboard_is_not_a_controller() {
        // The one case that matters: a keyboard claims hundreds of key
        // codes, and a naive "has buttons" test calls it a gamepad.
        assert!(parse_device(KEYBOARD).is_none());
    }

    #[test]
    fn a_mouse_and_a_lid_switch_are_not_controllers() {
        let mouse = "I: Bus=0018 Vendor=04f3 Product=30e9 Version=0100\n\
            N: Name=\"ELAN1200:00 04F3:30E9 Mouse\"\n\
            H: Handlers=mouse0 event6 \n\
            B: EV=17\n\
            B: KEY=30000 0 0 0 0\n\
            B: ABS=0";
        assert!(parse_device(mouse).is_none());
        let lid = "I: Bus=0019 Vendor=0000 Product=0005 Version=0000\n\
            N: Name=\"Lid Switch\"\n\
            H: Handlers=event0 \n\
            B: EV=21";
        assert!(parse_device(lid).is_none());
    }

    #[test]
    fn a_flight_stick_counts_even_without_face_buttons() {
        let stick = "I: Bus=0003 Vendor=044f Product=b10a Version=0110\n\
            N: Name=\"Thrustmaster T.16000M\"\n\
            H: Handlers=event9 js0 \n\
            B: EV=1b\n\
            B: KEY=ffff00000000 0 0 0 0\n\
            B: ABS=100000f";
        let found = parse_device(stick).expect("a stick is a controller");
        assert!(!found.is_gamepad);
        assert_eq!(found.kind(), "Stick or wheel");
    }

    #[test]
    fn several_devices_are_read_from_one_file() {
        let file = format!("{KEYBOARD}\n\n{XBOX}\n\n{KEYBOARD}\n");
        let found = parse_devices(&file);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Microsoft X-Box 360 pad");
    }

    #[test]
    fn this_machines_own_devices_parse_without_panicking() {
        // Whatever is plugged in, reading the real file must not blow up
        // and must not invent a controller out of a keyboard.
        for controller in discover() {
            assert!(!controller.name.is_empty());
            assert!(controller.is_gamepad || controller.joystick.is_some() || controller.axes > 0);
        }
    }

    #[test]
    fn capability_bitmaps_are_read_from_the_right_end() {
        // Printed high word first, so the last word holds bits 0..63.
        let bits = bitmap("7cdb000000000000 0 0 0 0");
        assert!(bit_set(&bits, 0x130), "BTN_SOUTH should be set");
        assert!(bit_set(&bits, 0x13e), "BTN_THUMBR should be set");
        assert!(!bit_set(&bits, 0x132), "BTN_C is not on a 360 pad");
        assert!(!bit_set(&bits, 0x00));
        // The keyboard on the machine this was written on: four words, and
        // nothing at all above bit 255, which is what makes it not a pad.
        let keyboard = bitmap("402000000 3803078f800d001 feffffdfffefffff fffffffffffffffe");
        assert!(!bit_set(&keyboard, 0x130));
        assert!(bit_set(&keyboard, 1), "KEY_ESC is set on a keyboard");
        // One word: bit 0 and bit 3.
        let small = bitmap("9");
        assert!(bit_set(&small, 0));
        assert!(bit_set(&small, 3));
        assert!(!bit_set(&small, 1));
        // Past the end is not set, rather than a panic.
        assert!(!bit_set(&small, 5000));
        assert!(!bit_set(&[], 0));
    }

    #[test]
    fn a_bus_number_becomes_a_transport() {
        assert_eq!(Transport::from_bus(0x03), Transport::Usb);
        assert_eq!(Transport::from_bus(0x05), Transport::Bluetooth);
        assert_eq!(Transport::from_bus(0x06), Transport::Virtual);
        assert_eq!(Transport::from_bus(0x99), Transport::Other);
    }

    #[test]
    fn event_records_become_presses_and_movements() {
        let mut bytes = Vec::new();
        let mut event = |kind: u16, code: u16, value: i32| {
            bytes.extend_from_slice(&[0u8; 16]); // timestamp, unread
            bytes.extend_from_slice(&kind.to_ne_bytes());
            bytes.extend_from_slice(&code.to_ne_bytes());
            bytes.extend_from_slice(&value.to_ne_bytes());
        };
        event(EV_KEY, 0x130, 1);
        event(EV_ABS_TYPE, 0x00, -32_768);
        event(0x00, 0, 0); // EV_SYN, which carries no information here
        event(EV_KEY, 0x130, 0);
        let parsed = parse_events(&bytes);
        assert_eq!(
            parsed,
            vec![
                Input::Button {
                    code: 0x130,
                    pressed: true
                },
                Input::Axis {
                    code: 0x00,
                    value: -32_768
                },
                Input::Button {
                    code: 0x130,
                    pressed: false
                },
            ]
        );
    }

    #[test]
    fn a_half_record_is_dropped_rather_than_misread() {
        // A short read must never be interpreted as a phantom press.
        assert!(parse_events(&[0u8; 10]).is_empty());
        assert!(parse_events(&[]).is_empty());
    }

    #[test]
    fn buttons_and_axes_have_names_people_recognise() {
        assert_eq!(button_name(0x130), "A / Cross");
        assert_eq!(button_name(0x13b), "Start");
        assert!(button_name(0x999).contains("0x999"));
        assert_eq!(axis_name(0x00), "Left stick X");
        assert_eq!(axis_name(0x05), "Right trigger");
    }

    #[test]
    fn a_battery_is_matched_to_its_own_controller() {
        let input =
            Path::new("/sys/devices/pci0000:00/usb1/1-3/1-3:1.0/0003:054C:0CE6.0009/input/input25");
        let battery = Path::new(
            "/sys/devices/pci0000:00/usb1/1-3/1-3:1.0/0003:054C:0CE6.0009/power_supply/ps-controller-battery",
        );
        assert!(shared_hid_device(input, battery));
        let other = Path::new(
            "/sys/devices/pci0000:00/usb1/1-4/1-4:1.0/0003:045E:028E.000A/power_supply/xbox-battery",
        );
        assert!(!shared_hid_device(input, other));
        // The laptop's own battery shares no HID device with anything.
        assert!(!shared_hid_device(
            input,
            Path::new("/sys/class/power_supply/BAT0")
        ));
    }
}
