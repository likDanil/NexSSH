//! A server's load (processor, memory, disk), read on a connected session's connection for
//! the status bar. One short command on a channel of its own ([`crate::exec`]), no terminal,
//! nothing left running on the server: the page asks again every few seconds while it shows
//! the session.
//!
//! The numbers come from Linux's `/proc` and `df`; on servers without them (BSD, macOS,
//! Windows) every part is `None`. The processor's use needs two readings: [`CpuTimes`] are
//! counters since boot, and the page takes the difference of two of them.

use std::time::Duration;

use serde::Serialize;

use crate::error::Result;
use crate::exec::ExecOptions;
use crate::session::{SessionId, SessionManager};

/// Lines starting with `@` mark which file follows. The same in every shell (sh, bash, zsh,
/// fish, csh): `;`, `echo` and `env` only, and `env` keeps `df`'s numbers unlocalized.
const COMMAND: &str = "echo @stat; head -n 1 /proc/stat; echo @mem; cat /proc/meminfo; \
                       echo @load; cat /proc/loadavg; echo @uptime; cat /proc/uptime; \
                       echo @df; env LC_ALL=C df -kP /";

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metrics {
    pub cpu: Option<CpuTimes>,
    pub memory: Option<Usage>,
    /// `None` also when the server has no swap.
    pub swap: Option<Usage>,
    /// The file system of `/`.
    pub disk: Option<Usage>,
    /// Load averages over 1, 5 and 15 minutes.
    pub load: Option<[f64; 3]>,
    /// Seconds since the server started.
    pub uptime: Option<u64>,
}

impl Metrics {
    /// Nothing could be read: not a Linux server.
    pub fn is_empty(&self) -> bool {
        self.cpu.is_none() && self.memory.is_none() && self.disk.is_none()
    }
}

/// The processor's time since boot, in ticks of all its cores together.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CpuTimes {
    pub total: u64,
    /// Idle, waiting for disks included.
    pub idle: u64,
}

/// Bytes used of a total; `free` is what is left for use, which for a disk leaves out the
/// blocks kept for root (so `used + free` may be less than `total`, as in `df`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub total: u64,
    pub used: u64,
    pub free: u64,
}

/// Reads the load of the server a connected session is on.
pub async fn read(sessions: &SessionManager, id: SessionId) -> Result<Metrics> {
    let options = ExecOptions {
        timeout: Duration::from_secs(10),
        max_output: 64 * 1024,
        stdin: Vec::new(),
    };
    let output = sessions.exec(id, COMMAND, options).await?;
    let (head, tail) = output.stdout.parts();
    let mut text = String::from_utf8_lossy(head).into_owned();
    text.push_str(&String::from_utf8_lossy(&tail));
    Ok(parse(&text))
}

/// Parses what [`COMMAND`] printed. Anything missing or malformed is left out; lines before the
/// first marker (what a shell's startup files print) are ignored.
pub fn parse(text: &str) -> Metrics {
    let mut metrics = Metrics::default();
    let mut section = "";
    let mut mem = Meminfo::default();
    let mut df_lines = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(name) = line.strip_prefix('@') {
            section = name;
            continue;
        }
        match section {
            "stat" => metrics.cpu = metrics.cpu.or_else(|| cpu(line)),
            "mem" => mem.line(line),
            "load" => metrics.load = metrics.load.or_else(|| load(line)),
            "uptime" => {
                metrics.uptime = metrics.uptime.or_else(|| {
                    let secs: f64 = line.split_whitespace().next()?.parse().ok()?;
                    (secs >= 0.0).then_some(secs as u64)
                });
            }
            "df" => {
                // The first line is the header (in the server's language when `env` is not
                // there).
                df_lines += 1;
                if df_lines > 1 && metrics.disk.is_none() {
                    metrics.disk = disk(line);
                }
            }
            _ => {}
        }
    }
    (metrics.memory, metrics.swap) = mem.usage();
    metrics
}

/// `cpu  user nice system idle iowait irq softirq steal guest guest_nice`. Guest time is
/// already counted in user time.
fn cpu(line: &str) -> Option<CpuTimes> {
    let mut fields = line.split_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let values: Vec<u64> = fields.take(8).map_while(|f| f.parse().ok()).collect();
    if values.len() < 4 {
        return None;
    }
    Some(CpuTimes {
        total: values.iter().sum(),
        idle: values[3] + values.get(4).copied().unwrap_or(0),
    })
}

fn load(line: &str) -> Option<[f64; 3]> {
    let mut fields = line.split_whitespace().map(|f| f.parse::<f64>().ok());
    Some([fields.next()??, fields.next()??, fields.next()??])
}

/// `Filesystem 1024-blocks Used Available Capacity Mounted-on`, read from the end: the file
/// system's name may have spaces.
fn disk(line: &str) -> Option<Usage> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    let n = fields.len();
    if n < 6 || !fields[n - 2].ends_with('%') {
        return None;
    }
    let kib = |f: &str| f.parse::<u64>().ok().map(|k| k * 1024);
    Some(Usage {
        total: kib(fields[n - 5])?,
        used: kib(fields[n - 4])?,
        free: kib(fields[n - 3])?,
    })
}

#[derive(Default)]
struct Meminfo {
    total: Option<u64>,
    available: Option<u64>,
    free: Option<u64>,
    buffers: u64,
    cached: u64,
    reclaimable: u64,
    swap_total: Option<u64>,
    swap_free: Option<u64>,
}

impl Meminfo {
    /// `MemTotal:        8040564 kB`
    fn line(&mut self, line: &str) {
        let Some((key, rest)) = line.split_once(':') else {
            return;
        };
        let Some(kib) = rest
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<u64>().ok())
        else {
            return;
        };
        let bytes = kib * 1024;
        match key {
            "MemTotal" => self.total = Some(bytes),
            "MemAvailable" => self.available = Some(bytes),
            "MemFree" => self.free = Some(bytes),
            "Buffers" => self.buffers = bytes,
            "Cached" => self.cached = bytes,
            "SReclaimable" => self.reclaimable = bytes,
            "SwapTotal" => self.swap_total = Some(bytes),
            "SwapFree" => self.swap_free = Some(bytes),
            _ => {}
        }
    }

    fn usage(&self) -> (Option<Usage>, Option<Usage>) {
        let memory = self.total.and_then(|total| {
            // Kernels before 3.14 have no MemAvailable: count what `free` used to.
            let available = self
                .available
                .or_else(|| Some(self.free? + self.buffers + self.cached + self.reclaimable))?
                .min(total);
            Some(Usage {
                total,
                used: total - available,
                free: available,
            })
        });
        let swap = match (self.swap_total, self.swap_free) {
            (Some(total), Some(free)) if total > 0 => {
                let free = free.min(total);
                Some(Usage {
                    total,
                    used: total - free,
                    free,
                })
            }
            _ => None,
        };
        (memory, swap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &str = "\
Welcome! (printed by .bashrc)
@stat
cpu  4705 356 584 3699176 23060 0 277 0 0 0
@mem
MemTotal:        8040564 kB
MemFree:          621404 kB
MemAvailable:    5893180 kB
Buffers:          291208 kB
Cached:          4613764 kB
SwapCached:            0 kB
SwapTotal:       2097148 kB
SwapFree:        1572860 kB
@load
0.15 0.32 0.28 1/412 98231
@uptime
1046823.51 8272671.30
@df
Filesystem     1024-blocks     Used Available Capacity Mounted on
/dev/sda1         51290592 23547880  25104996      49% /
";

    #[test]
    fn reads_a_linux_server() {
        let m = parse(UBUNTU);
        assert_eq!(
            m.cpu,
            Some(CpuTimes {
                total: 4705 + 356 + 584 + 3699176 + 23060 + 277,
                idle: 3699176 + 23060,
            })
        );
        assert_eq!(
            m.memory,
            Some(Usage {
                total: 8040564 * 1024,
                used: (8040564 - 5893180) * 1024,
                free: 5893180 * 1024,
            })
        );
        assert_eq!(
            m.swap,
            Some(Usage {
                total: 2097148 * 1024,
                used: (2097148 - 1572860) * 1024,
                free: 1572860 * 1024,
            })
        );
        assert_eq!(
            m.disk,
            Some(Usage {
                total: 51290592 * 1024,
                used: 23547880 * 1024,
                free: 25104996 * 1024,
            })
        );
        assert_eq!(m.load, Some([0.15, 0.32, 0.28]));
        assert_eq!(m.uptime, Some(1046823));
        assert!(!m.is_empty());
    }

    #[test]
    fn old_kernels_and_no_swap() {
        let m = parse(
            "@stat\ncpu  10 0 5 85\n@mem\nMemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\n\
             Cached: 250 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB\n",
        );
        assert_eq!(
            m.cpu,
            Some(CpuTimes {
                total: 100,
                idle: 85
            })
        );
        assert_eq!(
            m.memory,
            Some(Usage {
                total: 1000 * 1024,
                used: 600 * 1024,
                free: 400 * 1024,
            })
        );
        assert_eq!(m.swap, None);
        assert_eq!(m.disk, None);
    }

    #[test]
    fn file_system_names_with_spaces() {
        let m = parse(
            "@df\nFilesystem 1024-blocks Used Available Capacity Mounted on\nmy disk 100 40 50 45% /\n",
        );
        assert_eq!(
            m.disk,
            Some(Usage {
                total: 100 * 1024,
                used: 40 * 1024,
                free: 50 * 1024,
            })
        );
    }

    #[test]
    fn nothing_on_other_systems() {
        let m = parse(
            "@stat\nhead: /proc/stat: No such file or directory\n@mem\n\
             cat: /proc/meminfo: No such file or directory\n@df\n\
             Filesystem 1024-blocks Used Available Capacity Mounted on\n",
        );
        assert!(m.is_empty());
        assert_eq!(parse(""), Metrics::default());
    }
}
