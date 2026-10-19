//! How many cores are idle right now, so a parallel run (`Jobs::Auto`) knows whether another
//! worker process would find a core to run on.
//!
//! The coordinator asks [`CpuSampler::idle_cores`] every few seconds: the answer is the idle
//! fraction of all CPU time since the previous call, times the number of cores. It is std only
//! on purpose: a dependency such as `sysinfo` is a large tree for one number, and the three
//! operating-system calls below are a handful of lines each. The sources are
//! `GetSystemTimes` on Windows, the first `cpu ` line of `/proc/stat` on Linux and
//! `host_statistics(HOST_CPU_LOAD_INFO)` on macOS; every other platform, and every failed
//! reading, gives `None`, and the caller then falls back to growing by one worker per interval.
//!
//! For example, a test machine that runs a build in the background shows few idle cores, so the
//! coordinator keeps its worker count; the same machine at rest shows nearly all of them idle
//! and the count grows toward the cap.

/// Cumulative CPU time counters at one moment, in the platform's own tick unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Totals {
    /// Ticks spent idle (Linux: idle plus iowait).
    idle: u64,
    /// Ticks spent in every state, idle included.
    total: u64,
}

/// Measures how many cores were idle between two calls.
pub struct CpuSampler {
    /// The counters at the previous call (`None` when the first reading failed).
    last: Option<Totals>,
}

impl CpuSampler {
    /// Takes the first sample.
    pub(crate) fn new() -> Self {
        Self {
            last: read_totals(),
        }
    }

    /// Idle cores (0.0..=cores) over the time since the previous call, `None` when the
    /// platform is unsupported or a reading failed.
    pub(crate) fn idle_cores(&mut self) -> Option<f64> {
        let now = read_totals();
        let previous = std::mem::replace(&mut self.last, now);
        idle_cores_between(previous?, now?, core_count())
    }
}

/// The number of logical cores (at least 1).
fn core_count() -> f64 {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    f64::from(u32::try_from(cores).unwrap_or(u32::MAX))
}

/// Idle cores from two counter readings: the idle share of the elapsed ticks times `cores`.
/// `None` when no time passed or a counter went backwards (a wrap or a reset), because the
/// share is then meaningless.
#[expect(
    clippy::cast_precision_loss,
    reason = "the operands are tick differences between two samples seconds apart, far below 2^52"
)]
fn idle_cores_between(previous: Totals, now: Totals, cores: f64) -> Option<f64> {
    let total = now.total.checked_sub(previous.total)?;
    let idle = now.idle.checked_sub(previous.idle)?;
    if total == 0 {
        return None;
    }
    // Counters read at slightly different instants can show more idle than total ticks.
    let fraction = (idle as f64 / total as f64).clamp(0.0, 1.0);
    Some(fraction * cores)
}

/// Windows: `GetSystemTimes` fills idle, kernel and user times; kernel time includes idle.
#[cfg(windows)]
fn read_totals() -> Option<Totals> {
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn ticks(self) -> u64 {
            (u64::from(self.high) << 32) | u64::from(self.low)
        }
    }

    unsafe extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    }

    let mut idle = FileTime::default();
    let mut kernel = FileTime::default();
    let mut user = FileTime::default();
    // SAFETY: the three pointers refer to live, writable `FileTime` values whose layout matches
    // the Win32 `FILETIME` (two u32), and `GetSystemTimes` only writes through them.
    let ok = unsafe { GetSystemTimes(&raw mut idle, &raw mut kernel, &raw mut user) };
    if ok == 0 {
        return None;
    }
    Some(Totals {
        idle: idle.ticks(),
        total: kernel.ticks().checked_add(user.ticks())?,
    })
}

/// Linux: the aggregate `cpu ` line of `/proc/stat`.
#[cfg(target_os = "linux")]
fn read_totals() -> Option<Totals> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    parse_proc_stat(&text)
}

/// Parses the aggregate `cpu ` line: user nice system idle iowait irq softirq steal. Guest time
/// is already part of user time, so it is not added again.
#[cfg(target_os = "linux")]
fn parse_proc_stat(text: &str) -> Option<Totals> {
    let line = text.lines().find(|line| line.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(|field| field.parse::<u64>().ok())
        .collect::<Option<_>>()?;
    if fields.len() < 5 {
        return None;
    }
    Some(Totals {
        idle: fields[3].checked_add(fields[4])?,
        total: fields.iter().try_fold(0u64, |sum, v| sum.checked_add(*v))?,
    })
}

/// macOS: `host_statistics` with `HOST_CPU_LOAD_INFO` (user, system, idle, nice ticks).
#[cfg(target_os = "macos")]
fn read_totals() -> Option<Totals> {
    const HOST_CPU_LOAD_INFO: i32 = 3;
    const HOST_CPU_LOAD_INFO_COUNT: u32 = 4;
    const KERN_SUCCESS: i32 = 0;

    unsafe extern "C" {
        fn mach_host_self() -> u32;
        fn host_statistics(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
    }

    let mut ticks = [0u32; HOST_CPU_LOAD_INFO_COUNT as usize];
    let mut count = HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: `ticks` is a live array of `HOST_CPU_LOAD_INFO_COUNT` 32-bit integers, which is
    // exactly the buffer `host_statistics` fills for `HOST_CPU_LOAD_INFO`, and `count` tells it
    // that size. `mach_host_self` takes no arguments and returns the host port.
    let status = unsafe {
        host_statistics(
            mach_host_self(),
            HOST_CPU_LOAD_INFO,
            ticks.as_mut_ptr().cast::<i32>(),
            &mut count,
        )
    };
    if status != KERN_SUCCESS {
        return None;
    }
    Some(Totals {
        idle: u64::from(ticks[2]),
        total: ticks.iter().map(|t| u64::from(*t)).sum(),
    })
}

/// Every other platform: not measurable.
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn read_totals() -> Option<Totals> {
    None
}

#[cfg(test)]
mod tests {
    use super::{CpuSampler, Totals, idle_cores_between};

    fn totals(idle: u64, total: u64) -> Totals {
        Totals { idle, total }
    }

    #[test]
    fn half_idle_on_four_cores_is_two_idle_cores() {
        let got = idle_cores_between(totals(100, 1000), totals(600, 2000), 4.0);
        assert_eq!(got, Some(2.0));
    }

    #[test]
    fn fully_busy_and_fully_idle_hit_the_bounds() {
        assert_eq!(
            idle_cores_between(totals(10, 100), totals(10, 200), 8.0),
            Some(0.0)
        );
        assert_eq!(
            idle_cores_between(totals(10, 100), totals(110, 200), 8.0),
            Some(8.0)
        );
    }

    #[test]
    fn more_idle_than_total_is_clamped_to_all_cores() {
        assert_eq!(
            idle_cores_between(totals(0, 100), totals(150, 200), 2.0),
            Some(2.0)
        );
    }

    #[test]
    fn zero_elapsed_total_is_not_measurable() {
        assert_eq!(
            idle_cores_between(totals(5, 100), totals(5, 100), 4.0),
            None
        );
    }

    #[test]
    fn a_counter_that_went_backwards_is_not_measurable() {
        assert_eq!(idle_cores_between(totals(5, 100), totals(5, 50), 4.0), None);
        assert_eq!(
            idle_cores_between(totals(50, 100), totals(5, 200), 4.0),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_stat_line_is_parsed() {
        let text = "cpu  10 20 30 400 50 6 7 8 9 10\ncpu0 1 2 3 4 5 6 7 8 9 10\n";
        let got = super::parse_proc_stat(text).expect("the line parses");
        assert_eq!(got, totals(450, 10 + 20 + 30 + 400 + 50 + 6 + 7 + 8));
        assert_eq!(super::parse_proc_stat("intr 1 2 3\n"), None);
        assert_eq!(super::parse_proc_stat("cpu  1 2 x 4 5\n"), None);
    }

    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn two_samples_apart_give_a_value_in_range() {
        let mut sampler = CpuSampler::new();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let value = sampler
            .idle_cores()
            .expect("the platform reports CPU times");
        let cores = super::core_count();
        assert!(
            (0.0..=cores).contains(&value),
            "{value} outside 0..={cores}"
        );
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    #[test]
    fn unsupported_platform_is_none() {
        assert_eq!(CpuSampler::new().idle_cores(), None);
    }
}
