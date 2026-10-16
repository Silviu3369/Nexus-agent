//! Machine facts for the dashboard: CPU load and physical memory, read
//! through Win32 when asked. Read-only; nothing here changes the system.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use nexus_protocol::SystemStatusView;
use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    ALL_PROCESSOR_GROUPS, GetActiveProcessorCount, GetSystemTimes,
};

/// A first reading has no earlier one to compare with, so it waits this
/// long for a second sample.
const FIRST_SAMPLE: Duration = Duration::from_millis(250);
/// Readings closer together than this reuse the last load instead of
/// measuring a few milliseconds of noise.
const MIN_INTERVAL: Duration = Duration::from_millis(500);

const MIB: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CpuTimes {
    idle: u64,
    /// Kernel time includes idle time on Windows.
    kernel: u64,
    user: u64,
}

impl CpuTimes {
    fn read() -> Option<Self> {
        let mut idle = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut kernel = idle;
        let mut user = idle;
        // SAFETY: three valid, writable FILETIME out-pointers.
        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        (ok != 0).then(|| Self {
            idle: ticks(idle),
            kernel: ticks(kernel),
            user: ticks(user),
        })
    }
}

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

/// Busy share of the time between two readings, 0..=100.
fn busy_percent(before: CpuTimes, after: CpuTimes) -> Option<f32> {
    let idle = after.idle.checked_sub(before.idle)?;
    let total = after
        .kernel
        .checked_sub(before.kernel)?
        .checked_add(after.user.checked_sub(before.user)?)?;
    if total == 0 {
        return None;
    }
    let busy = total.saturating_sub(idle) as f64 / total as f64;
    Some((busy * 100.0).clamp(0.0, 100.0) as f32)
}

struct Sample {
    at: Instant,
    times: CpuTimes,
    percent: Option<f32>,
}

#[derive(Default)]
pub struct SystemMonitor {
    last: Mutex<Option<Sample>>,
}

impl SystemMonitor {
    pub async fn status(&self) -> SystemStatusView {
        let cpu_percent = self.cpu_percent().await;
        let (memory_used_mib, memory_total_mib) = memory_mib();
        // SAFETY: plain query, no pointers.
        let processors = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
        SystemStatusView {
            host_version: env!("CARGO_PKG_VERSION").to_string(),
            cpu_percent,
            memory_used_mib,
            memory_total_mib,
            logical_processors: (processors > 0).then_some(processors),
        }
    }

    async fn cpu_percent(&self) -> Option<f32> {
        let previous = {
            let last = self.last.lock().expect("monitor lock");
            match last.as_ref() {
                Some(s) if s.at.elapsed() < MIN_INTERVAL => return s.percent,
                Some(s) => Some((s.at, s.times)),
                None => None,
            }
        };
        let before = match previous {
            Some((_, times)) => times,
            None => {
                let first = CpuTimes::read()?;
                tokio::time::sleep(FIRST_SAMPLE).await;
                first
            }
        };
        let now = CpuTimes::read()?;
        let percent = busy_percent(before, now);
        *self.last.lock().expect("monitor lock") = Some(Sample {
            at: Instant::now(),
            times: now,
            percent,
        });
        percent
    }
}

fn memory_mib() -> (Option<u32>, Option<u32>) {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        dwMemoryLoad: 0,
        ullTotalPhys: 0,
        ullAvailPhys: 0,
        ullTotalPageFile: 0,
        ullAvailPageFile: 0,
        ullTotalVirtual: 0,
        ullAvailVirtual: 0,
        ullAvailExtendedVirtual: 0,
    };
    // SAFETY: a writable MEMORYSTATUSEX whose dwLength is set, as required.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 || status.ullTotalPhys == 0 {
        return (None, None);
    }
    let total = status.ullTotalPhys / MIB;
    let used = status.ullTotalPhys.saturating_sub(status.ullAvailPhys) / MIB;
    (u32::try_from(used).ok(), u32::try_from(total).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn times(idle: u64, kernel: u64, user: u64) -> CpuTimes {
        CpuTimes { idle, kernel, user }
    }

    #[test]
    fn busy_share_counts_kernel_minus_idle_plus_user() {
        // 100 ticks elapsed (kernel 60 incl. 50 idle, user 40): 50 % busy.
        assert_eq!(busy_percent(times(0, 0, 0), times(50, 60, 40)), Some(50.0));
        assert_eq!(busy_percent(times(0, 0, 0), times(100, 100, 0)), Some(0.0));
        assert_eq!(busy_percent(times(0, 0, 0), times(0, 30, 70)), Some(100.0));
    }

    #[test]
    fn no_elapsed_time_or_a_counter_going_back_gives_no_reading() {
        assert_eq!(busy_percent(times(5, 5, 5), times(5, 5, 5)), None);
        assert_eq!(busy_percent(times(9, 9, 9), times(1, 20, 20)), None);
    }

    #[tokio::test]
    async fn the_real_machine_reports_plausible_figures() {
        let monitor = SystemMonitor::default();
        let first = monitor.status().await;
        assert_eq!(first.host_version, env!("CARGO_PKG_VERSION"));
        let cpu = first.cpu_percent.expect("GetSystemTimes works");
        assert!((0.0..=100.0).contains(&cpu));
        let total = first.memory_total_mib.expect("GlobalMemoryStatusEx works");
        let used = first.memory_used_mib.expect("used memory");
        assert!(total > 0 && used <= total);
        assert!(first.logical_processors.unwrap_or(0) >= 1);
        // A second reading right away reuses the last load.
        assert_eq!(monitor.status().await.cpu_percent, Some(cpu));
    }
}
