//! Live readings from the graphics cards and the CPU.
//!
//! Two sources, because the two vendors expose different things:
//!
//! * **AMD and Intel** put everything in `sysfs` — `gpu_busy_percent` next
//!   to the device, temperature and power under its `hwmon`. No tool needs
//!   to be installed and no process is spawned.
//! * **NVIDIA** puts nothing useful in `sysfs`. `nvidia-smi` is the only
//!   way, so it is run once a tick, in one call that asks for every field
//!   at once rather than one call per number.
//!
//! Every field is an `Option`. A reading that is not available is drawn as
//! a dash, never as a zero: a card sitting at 0% and a card that will not
//! say are different things, and showing them the same way is how a
//! monitor starts lying.

use std::process::Command;

use crate::gpu::{Gpu, Vendor, read_num, read_text};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuReading {
    /// 0–100.
    pub utilization: Option<u32>,
    pub temperature_c: Option<f64>,
    pub power_w: Option<f64>,
    pub core_mhz: Option<u32>,
    pub memory_used_mb: Option<u64>,
    pub memory_total_mb: Option<u64>,
    pub fan_percent: Option<u32>,
}

impl GpuReading {
    pub fn is_empty(&self) -> bool {
        *self == GpuReading::default()
    }

    pub fn memory_percent(&self) -> Option<f64> {
        let (used, total) = (self.memory_used_mb?, self.memory_total_mb?);
        (total > 0).then(|| used as f64 / total as f64 * 100.0)
    }
}

pub fn read_gpu(gpu: &Gpu) -> GpuReading {
    match gpu.vendor {
        Vendor::Nvidia => read_nvidia(gpu),
        _ => read_sysfs(gpu),
    }
}

/// AMD and Intel, out of `/sys/class/drm/cardN/device`.
fn read_sysfs(gpu: &Gpu) -> GpuReading {
    let Some(card) = &gpu.card else {
        return GpuReading::default();
    };
    let device = std::path::Path::new("/sys/class/drm")
        .join(card)
        .join("device");
    let mut reading = GpuReading {
        utilization: read_num(device.join("gpu_busy_percent")).map(|v| v as u32),
        memory_used_mb: read_num(device.join("mem_info_vram_used")).map(|b| b / 1_048_576),
        memory_total_mb: read_num(device.join("mem_info_vram_total")).map(|b| b / 1_048_576),
        core_mhz: current_clock_mhz(&device),
        ..GpuReading::default()
    };
    // hwmon names its files in millidegrees, microwatts and percent-of-255.
    if let Some(hwmon) = first_subdirectory(device.join("hwmon")) {
        reading.temperature_c = read_num(hwmon.join("temp1_input")).map(|m| m as f64 / 1000.0);
        reading.power_w = read_num(hwmon.join("power1_average"))
            .or_else(|| read_num(hwmon.join("power1_input")))
            .map(|uw| uw as f64 / 1_000_000.0);
        reading.fan_percent = read_num(hwmon.join("pwm1")).map(|pwm| (pwm * 100 / 255) as u32);
    }
    reading
}

/// The starred line of `pp_dpm_sclk`: `1: 1200Mhz *`.
fn current_clock_mhz(device: &std::path::Path) -> Option<u32> {
    let text = read_text(device.join("pp_dpm_sclk"))?;
    let line = text.lines().find(|l| l.trim_end().ends_with('*'))?;
    parse_clock_line(line)
}

fn parse_clock_line(line: &str) -> Option<u32> {
    let value = line.split_whitespace().nth(1)?;
    let digits: String = value.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn first_subdirectory(dir: impl AsRef<std::path::Path>) -> Option<std::path::PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    entries.into_iter().next()
}

/// The fields asked of `nvidia-smi`, in the order they come back.
const NVIDIA_FIELDS: &str =
    "utilization.gpu,temperature.gpu,power.draw,clocks.sm,memory.used,memory.total,fan.speed";

fn read_nvidia(gpu: &Gpu) -> GpuReading {
    let Some(smi) = crate::drivers::which("nvidia-smi") else {
        return GpuReading::default();
    };
    // `--id` by PCI address, so a second NVIDIA card is not read as the
    // first. nvidia-smi takes the bus address in exactly the form sysfs
    // gives it.
    let out = Command::new(smi)
        .args([
            &format!("--query-gpu={NVIDIA_FIELDS}"),
            "--format=csv,noheader,nounits",
            &format!("--id={}", gpu.address),
        ])
        .output();
    let Ok(out) = out else {
        return GpuReading::default();
    };
    if !out.status.success() {
        return GpuReading::default();
    }
    parse_nvidia_csv(&String::from_utf8_lossy(&out.stdout))
}

/// One line of `nvidia-smi --format=csv,noheader,nounits`.
///
/// A field the card does not have comes back as `[N/A]`, which must read
/// as "no value" and not as a parse failure that discards the whole line —
/// a laptop card with no fan sensor still has a temperature worth showing.
fn parse_nvidia_csv(stdout: &str) -> GpuReading {
    let Some(line) = stdout.lines().find(|l| !l.trim().is_empty()) else {
        return GpuReading::default();
    };
    let fields: Vec<&str> = line.split(',').map(str::trim).collect();
    let get = |index: usize| -> Option<&str> {
        fields
            .get(index)
            .filter(|v| !v.is_empty() && !v.starts_with("[N/A") && **v != "N/A")
            .copied()
    };
    GpuReading {
        utilization: get(0).and_then(|v| v.parse().ok()),
        temperature_c: get(1).and_then(|v| v.parse().ok()),
        power_w: get(2).and_then(|v| v.parse().ok()),
        core_mhz: get(3).and_then(|v| v.parse().ok()),
        memory_used_mb: get(4).and_then(|v| v.parse().ok()),
        memory_total_mb: get(5).and_then(|v| v.parse().ok()),
        fan_percent: get(6).and_then(|v| v.parse().ok()),
    }
}

// ---- CPU -----------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct CpuReading {
    /// 0–100, over the interval since the previous reading.
    pub utilization: Option<f64>,
    pub governor: Option<String>,
    pub temperature_c: Option<f64>,
}

/// `/proc/stat`'s first line is cumulative jiffies since boot, so a
/// percentage needs two readings. The previous totals are kept by the
/// caller and handed back in.
#[derive(Debug, Clone, Copy, Default)]
pub struct CpuTotals {
    busy: u64,
    total: u64,
}

pub fn read_cpu(previous: CpuTotals) -> (CpuReading, CpuTotals) {
    let totals = cpu_totals().unwrap_or_default();
    let utilization = cpu_percent(previous, totals);
    let reading = CpuReading {
        utilization,
        governor: read_text("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor"),
        temperature_c: cpu_temperature(),
    };
    (reading, totals)
}

fn cpu_totals() -> Option<CpuTotals> {
    let stat = read_text("/proc/stat")?;
    let line = stat.lines().next()?.strip_prefix("cpu ")?;
    let values: Vec<u64> = line
        .split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    if values.len() < 4 {
        return None;
    }
    let total: u64 = values.iter().sum();
    // Fields 3 and 4 are idle and iowait; everything else is work.
    let idle = values[3] + values.get(4).copied().unwrap_or(0);
    Some(CpuTotals {
        busy: total.saturating_sub(idle),
        total,
    })
}

fn cpu_percent(previous: CpuTotals, now: CpuTotals) -> Option<f64> {
    let total = now.total.checked_sub(previous.total)?;
    let busy = now.busy.checked_sub(previous.busy)?;
    (total > 0).then(|| (busy as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

/// The package temperature, from whichever hwmon claims to be the CPU.
fn cpu_temperature() -> Option<f64> {
    let entries = std::fs::read_dir("/sys/class/hwmon").ok()?;
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        let name = read_text(path.join("name")).unwrap_or_default();
        if matches!(
            name.as_str(),
            "k10temp" | "coretemp" | "zenpower" | "cpu_thermal"
        ) && let Some(milli) = read_num(path.join("temp1_input"))
        {
            return Some(milli as f64 / 1000.0);
        }
    }
    None
}

// ---- memory and storage --------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub used: u64,
    pub total: u64,
}

impl Usage {
    pub fn fraction(self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.used as f64 / self.total as f64).clamp(0.0, 1.0)
        }
    }

    pub fn is_known(self) -> bool {
        self.total > 0
    }
}

/// System memory in use.
///
/// `MemTotal - MemAvailable`, not `MemFree`: the kernel keeps every page it
/// can as cache, so `MemFree` on a healthy machine is near zero and a gauge
/// built on it sits at 99% saying nothing. `MemAvailable` is the kernel's
/// own estimate of what a new program could actually get, which is the
/// question being asked.
pub fn read_memory() -> Usage {
    let Some(text) = read_text("/proc/meminfo") else {
        return Usage::default();
    };
    parse_meminfo(&text)
}

fn parse_meminfo(text: &str) -> Usage {
    let field = |name: &str| -> Option<u64> {
        text.lines()
            .find(|line| line.starts_with(name) && line[name.len()..].starts_with(':'))?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    };
    let Some(total) = field("MemTotal") else {
        return Usage::default();
    };
    // A kernel too old for MemAvailable falls back to the crude sum, which
    // is what MemAvailable was introduced to replace.
    let available = field("MemAvailable").or_else(|| {
        Some(field("MemFree")? + field("Cached").unwrap_or(0) + field("Buffers").unwrap_or(0))
    });
    Usage {
        used: total.saturating_sub(available.unwrap_or(0).min(total)),
        total,
    }
}

/// How full the filesystem holding `path` is.
pub fn read_storage(path: &std::path::Path) -> Usage {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return Usage::default();
    };
    // SAFETY: `statvfs` writes into the struct and reads the NUL-terminated
    // path, both of which live for the call. A failure is reported by the
    // return value and leaves the struct as the zeroes it started as.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return Usage::default();
    }
    let block = if stat.f_frsize > 0 {
        stat.f_frsize as u64
    } else {
        stat.f_bsize as u64
    };
    let total = (stat.f_blocks as u64).saturating_mul(block);
    // `f_bavail`, not `f_bfree`: the reserved blocks are not free to us.
    let free = (stat.f_bavail as u64).saturating_mul(block);
    Usage {
        used: total.saturating_sub(free),
        total,
    }
}

// ---- formatting ----------------------------------------------------------

/// A number with its unit, or an em dash when there is no number. Every
/// live label in the app goes through one of these, so "not available"
/// looks the same everywhere.
pub fn percent(value: Option<impl Into<f64>>) -> String {
    match value {
        Some(v) => format!("{:.0}%", v.into()),
        None => "—".into(),
    }
}

pub fn degrees(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.0}°C"),
        None => "—".into(),
    }
}

pub fn watts(value: Option<f64>) -> String {
    match value {
        Some(v) => format!("{v:.1} W"),
        None => "—".into(),
    }
}

pub fn megahertz(value: Option<u32>) -> String {
    match value {
        Some(v) if v >= 1000 => format!("{:.2} GHz", v as f64 / 1000.0),
        Some(v) => format!("{v} MHz"),
        None => "—".into(),
    }
}

pub fn memory(used: Option<u64>, total: Option<u64>) -> String {
    match (used, total) {
        (Some(u), Some(t)) => format!("{:.1} / {:.1} GB", u as f64 / 1024.0, t as f64 / 1024.0),
        (Some(u), None) => format!("{:.1} GB", u as f64 / 1024.0),
        _ => "—".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_in_use_is_what_a_program_cannot_have() {
        let meminfo = "\
MemTotal:       24410940 kB
MemFree:          812344 kB
MemAvailable:   20121152 kB
Buffers:          104200 kB
Cached:         18900000 kB
";
        let usage = parse_meminfo(meminfo);
        assert_eq!(usage.total, 24_410_940 * 1024);
        // Not MemFree: the cache is available, and a gauge built on
        // MemFree would read 97% on an idle machine.
        assert_eq!(usage.used, (24_410_940 - 20_121_152) * 1024);
        assert!(usage.fraction() > 0.15 && usage.fraction() < 0.20);
    }

    #[test]
    fn a_kernel_without_memavailable_still_reports() {
        let old = "MemTotal: 1000 kB\nMemFree: 200 kB\nBuffers: 100 kB\nCached: 300 kB\n";
        let usage = parse_meminfo(old);
        assert_eq!(usage.used, 400 * 1024);
    }

    #[test]
    fn a_prefix_is_not_a_meminfo_field() {
        // "MemTotal" must not match "MemTotalFoo", and "SwapFree" must not
        // be read as "MemFree".
        let text = "MemTotalish: 5 kB\nMemTotal: 100 kB\nMemAvailable: 40 kB\n";
        assert_eq!(parse_meminfo(text).total, 100 * 1024);
    }

    #[test]
    fn nonsense_meminfo_reports_nothing_rather_than_guessing() {
        assert!(!parse_meminfo("").is_known());
        assert!(!parse_meminfo("garbage\n").is_known());
        assert_eq!(Usage::default().fraction(), 0.0);
    }

    #[test]
    fn storage_is_read_for_a_real_directory() {
        let root = parse_meminfo("");
        assert!(!root.is_known());
        let usage = read_storage(std::path::Path::new("/"));
        assert!(usage.is_known(), "the root filesystem should have a size");
        assert!(usage.used <= usage.total);
        // A path that does not exist reports nothing, rather than zero
        // bytes free.
        assert!(!read_storage(std::path::Path::new("/definitely/not/here/4a9f")).is_known());
    }

    #[test]
    fn nvidia_csv_is_read_field_by_field() {
        let r = parse_nvidia_csv("0, 59, 5.90, 300, 4, 6144, [N/A]\n");
        assert_eq!(r.utilization, Some(0));
        assert_eq!(r.temperature_c, Some(59.0));
        assert_eq!(r.power_w, Some(5.90));
        assert_eq!(r.core_mhz, Some(300));
        assert_eq!(r.memory_used_mb, Some(4));
        assert_eq!(r.memory_total_mb, Some(6144));
        // A laptop card with no fan reading must not poison the rest.
        assert_eq!(r.fan_percent, None);
    }

    #[test]
    fn an_empty_nvidia_answer_is_no_reading() {
        assert!(parse_nvidia_csv("").is_empty());
        assert!(parse_nvidia_csv("\n\n").is_empty());
    }

    #[test]
    fn a_short_nvidia_answer_keeps_what_it_has() {
        let r = parse_nvidia_csv("42, 70");
        assert_eq!(r.utilization, Some(42));
        assert_eq!(r.temperature_c, Some(70.0));
        assert_eq!(r.power_w, None);
    }

    #[test]
    fn the_starred_clock_is_the_current_one() {
        assert_eq!(parse_clock_line("1: 1200Mhz *"), Some(1200));
        assert_eq!(parse_clock_line("0: 400Mhz"), Some(400));
        assert_eq!(parse_clock_line("nonsense"), None);
    }

    #[test]
    fn cpu_use_is_the_change_between_two_readings() {
        let before = CpuTotals {
            busy: 100,
            total: 200,
        };
        let after = CpuTotals {
            busy: 150,
            total: 300,
        };
        assert_eq!(cpu_percent(before, after), Some(50.0));
        // The first reading of the session has nothing to compare against.
        assert_eq!(cpu_percent(after, after), None);
        // A counter that went backwards (a reset) reports nothing rather
        // than a wild number.
        assert_eq!(cpu_percent(after, before), None);
    }

    #[test]
    fn a_missing_reading_draws_a_dash_not_a_zero() {
        assert_eq!(percent(None::<f64>), "—");
        assert_eq!(percent(Some(0.0)), "0%");
        assert_eq!(degrees(None), "—");
        assert_eq!(watts(Some(5.9)), "5.9 W");
        assert_eq!(megahertz(Some(300)), "300 MHz");
        assert_eq!(megahertz(Some(1800)), "1.80 GHz");
        assert_eq!(memory(Some(2048), Some(6144)), "2.0 / 6.0 GB");
        assert_eq!(memory(None, Some(6144)), "—");
    }

    #[test]
    fn memory_percentage_needs_both_halves() {
        let r = GpuReading {
            memory_used_mb: Some(3072),
            memory_total_mb: Some(6144),
            ..Default::default()
        };
        assert_eq!(r.memory_percent(), Some(50.0));
        let r = GpuReading {
            memory_used_mb: Some(3072),
            ..Default::default()
        };
        assert_eq!(r.memory_percent(), None);
    }
}
