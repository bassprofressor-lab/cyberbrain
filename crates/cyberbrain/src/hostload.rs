//! What a model call costs the machine it runs on.
//!
//! Three things are measurable from here and they are not the same thing, so they are kept
//! apart and labelled:
//!
//! * **Wall time and tokens** — exact, and already in the audit log.
//! * **Machine-wide CPU and memory** around the call, from `/proc/stat` and `/proc/meminfo`.
//!   Portable and honest, but it attributes nothing: anything else running on the box lands
//!   in the same number.
//! * **The endpoint's own cgroup**, when the operator names one in `inference.load_cgroup`.
//!   That is exact attribution — CPU microseconds and resident bytes of that container and
//!   nothing else. It is configured rather than guessed, because the only way to find it
//!   automatically here would be to reason about Docker's proxy process, and a wrong
//!   attribution presented as an exact one is the failure this file exists to avoid.
//!
//! Everything degrades to `None`. A missing sample is a missing sample, never a zero.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One reading of the machine, taken before and after a call.
#[derive(Debug, Clone, Copy)]
pub struct HostSample {
    /// Busy jiffies across all CPUs: everything in `/proc/stat` except idle and iowait.
    pub busy_jiffies: u64,
    /// KiB the kernel believes is available without swapping.
    pub mem_available_kb: u64,
}

/// One reading of the endpoint's cgroup, when one is configured.
#[derive(Debug, Clone, Copy)]
pub struct CgroupSample {
    pub cpu_usage_us: u64,
    pub mem_current: u64,
    pub mem_peak: Option<u64>,
}

/// What one call cost, as far as it could be measured.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadRow {
    pub at: String,
    /// The task the call served, e.g. `contradiction-check`.
    pub task: String,
    pub wall_ms: u64,
    /// Cores busy on average across the whole machine during the call. Includes everything
    /// else that ran; on a busy host this says more about the host than about the model.
    pub machine_cores: Option<f64>,
    /// MiB of available memory lost across the call, machine-wide. Negative means memory
    /// came back.
    pub machine_mem_delta_mb: Option<i64>,
    /// Cores the configured cgroup burned on average. Exact attribution when present.
    pub endpoint_cores: Option<f64>,
    /// Resident bytes of that cgroup at the end of the call.
    pub endpoint_mem_bytes: Option<u64>,
    /// High-water mark the cgroup has seen since it started, not since this call.
    pub endpoint_mem_peak_bytes: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct LoadSummary {
    pub calls: u64,
    pub wall_ms: u64,
    /// Averages over the calls that carried the field.
    pub machine_cores_avg: Option<f64>,
    pub endpoint_cores_avg: Option<f64>,
    /// Last reading, which is the one worth showing beside a live status.
    pub last: Option<LoadRow>,
    /// Calls where no cgroup was configured or it could not be read.
    pub calls_without_attribution: u64,
    /// Cores the machine has, so a core count can be read as a share.
    pub cores_total: Option<usize>,
}

pub fn read_host() -> Option<HostSample> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let v: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|f| f.parse().ok())
        .collect();
    if v.len() < 5 {
        return None;
    }
    // user nice system [idle] [iowait] irq softirq steal ...
    let busy: u64 = v.iter().sum::<u64>().saturating_sub(v[3] + v[4]);
    let mem = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mem_available_kb = mem
        .lines()
        .find(|l| l.starts_with("MemAvailable:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(HostSample {
        busy_jiffies: busy,
        mem_available_kb,
    })
}

pub fn read_cgroup(dir: &Path) -> Option<CgroupSample> {
    let stat = std::fs::read_to_string(dir.join("cpu.stat")).ok()?;
    let cpu_usage_us = stat
        .lines()
        .find(|l| l.starts_with("usage_usec"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    let num = |f: &str| -> Option<u64> {
        std::fs::read_to_string(dir.join(f))
            .ok()?
            .trim()
            .parse()
            .ok()
    };
    Some(CgroupSample {
        cpu_usage_us,
        mem_current: num("memory.current")?,
        mem_peak: num("memory.peak"),
    })
}

/// Ticks per second the kernel counts `/proc/stat` in. 100 everywhere this runs; read from
/// the C library it would be `sysconf(_SC_CLK_TCK)`, which is not worth a dependency here.
const USER_HZ: f64 = 100.0;

/// Turn a pair of samples into a row. `None` fields are the ones that could not be measured.
pub fn row(
    task: &str,
    wall: std::time::Duration,
    host: (Option<HostSample>, Option<HostSample>),
    cg: (Option<CgroupSample>, Option<CgroupSample>),
) -> LoadRow {
    let wall_s = wall.as_secs_f64().max(0.001);
    let (machine_cores, machine_mem_delta_mb) = match (host.0, host.1) {
        (Some(a), Some(b)) => (
            Some((b.busy_jiffies.saturating_sub(a.busy_jiffies) as f64) / USER_HZ / wall_s),
            Some((a.mem_available_kb as i64 - b.mem_available_kb as i64) / 1024),
        ),
        _ => (None, None),
    };
    let (endpoint_cores, endpoint_mem_bytes, endpoint_mem_peak_bytes) = match (cg.0, cg.1) {
        (Some(a), Some(b)) => (
            Some((b.cpu_usage_us.saturating_sub(a.cpu_usage_us) as f64) / 1_000_000.0 / wall_s),
            Some(b.mem_current),
            b.mem_peak,
        ),
        _ => (None, None, None),
    };
    LoadRow {
        at: crate::usage::now(),
        task: task.to_string(),
        wall_ms: wall.as_millis() as u64,
        machine_cores,
        machine_mem_delta_mb,
        endpoint_cores,
        endpoint_mem_bytes,
        endpoint_mem_peak_bytes,
    }
}

/// Append-only, beside `usage.jsonl` and for the same reason: a record, not a cache, and
/// not compliance either.
pub struct LoadLog {
    path: PathBuf,
}

impl LoadLog {
    pub fn new(root: &Path) -> Self {
        Self {
            path: root.join("load.jsonl"),
        }
    }

    pub fn append(&self, row: &LoadRow) {
        use std::io::Write;
        let Ok(line) = serde_json::to_string(row) else {
            return;
        };
        let w = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = w {
            eprintln!("cyberbrain: load log append failed: {e}");
        }
    }

    /// The most recent row belonging to any of `tasks`.
    ///
    /// Read before a recall decides whether to wait for the contradiction check: the last
    /// measurement is the best available estimate of the next one, and it survives the
    /// process, which a one-shot CLI call otherwise cannot. Several task names because a
    /// call that completed and a call that was cut off are recorded apart, and the decision
    /// needs whichever happened last.
    pub fn last_of(&self, tasks: &[&str]) -> Option<LoadRow> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        text.lines()
            .rev()
            .filter_map(|l| serde_json::from_str::<LoadRow>(l).ok())
            .find(|r| tasks.contains(&r.task.as_str()))
    }

    pub fn summary(&self) -> LoadSummary {
        let mut s = LoadSummary {
            cores_total: std::thread::available_parallelism().ok().map(|n| n.get()),
            ..LoadSummary::default()
        };
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return s;
        };
        let (mut mach, mut mach_n) = (0.0f64, 0u64);
        let (mut ep, mut ep_n) = (0.0f64, 0u64);
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let Ok(r) = serde_json::from_str::<LoadRow>(line) else {
                continue;
            };
            s.calls += 1;
            s.wall_ms += r.wall_ms;
            if let Some(c) = r.machine_cores {
                mach += c;
                mach_n += 1;
            }
            match r.endpoint_cores {
                Some(c) => {
                    ep += c;
                    ep_n += 1;
                }
                None => s.calls_without_attribution += 1,
            }
            s.last = Some(r);
        }
        if mach_n > 0 {
            s.machine_cores_avg = Some(mach / mach_n as f64);
        }
        if ep_n > 0 {
            s.endpoint_cores_avg = Some(ep / ep_n as f64);
        }
        s
    }
}
