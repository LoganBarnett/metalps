//! macOS-specific GPU data collection.
//!
//! Three data sources are combined:
//!
//! 1. **IOKit `AGXDeviceUserClient`** — per-process:
//!    - `AppUsage[].accumulatedGPUTime` → cumulative GPU execution time (ns).
//!    - This is the source Activity Monitor uses on Apple Silicon.
//!
//! 2. **Mach `task_info(TASK_VM_INFO)`** — per-process:
//!    - `ledger_tag_graphics_footprint` → Metal memory footprint (bytes).
//!    - Requires same-user processes on non-sandboxed machines.
//!
//! 3. **IOKit `IOAccelerator` `PerformanceStatistics`** — device-level:
//!    - `"Device Utilization %"` → overall GPU utilization.
//!    - `"In use system memory"` → unified memory used by GPU clients.

pub mod iokit;
pub mod mach;
pub mod proc;

use crate::collector::CollectorError;
use crate::types::{DeviceGpuInfo, GpuProcessInfo};
use std::time::{SystemTime, UNIX_EPOCH};

/// A raw per-process GPU sample collected at one point in time.
#[derive(Debug, Clone)]
pub struct RawSample {
  pub pid: i32,
  pub name: String,
  /// Cumulative GPU execution time in nanoseconds (since process start).
  pub gpu_time_ns: u64,
  /// Metal memory footprint in bytes, if readable.
  pub vram_bytes: Option<u64>,
  /// Wall-clock nanoseconds when this sample was taken.
  pub wall_ns: u64,
}

/// Collect raw GPU samples for all processes with active GPU connections,
/// plus device-level info.
pub fn collect() -> Result<(Vec<RawSample>, Vec<DeviceGpuInfo>), CollectorError>
{
  let wall_ns = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos() as u64;

  let client_times = iokit::query_client_gpu_times();

  let samples = client_times
    .iter()
    .map(|c| RawSample {
      pid: c.pid,
      name: mach::process_name(c.pid),
      gpu_time_ns: c.accumulated_gpu_time_ns,
      vram_bytes: mach::vram_for_pid(c.pid),
      wall_ns,
    })
    .collect();

  let devices = iokit::query_devices();
  Ok((samples, devices))
}

/// Build a zero-GPU-time `RawSample` for a PID not yet in the IOKit client
/// list.  Used when `--pid` is requested but the process has no active GPU
/// connection yet.
pub fn seed_sample(pid: i32) -> RawSample {
  let wall_ns = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos() as u64;
  RawSample {
    pid,
    name: mach::process_name(pid),
    gpu_time_ns: 0,
    vram_bytes: mach::vram_for_pid(pid),
    wall_ns,
  }
}

/// Compute `GpuProcessInfo` for each process that appears in both snapshots,
/// using the wall-clock delta to calculate GPU utilization.
pub fn compute_gpu_info(
  first: &[RawSample],
  second: &[RawSample],
) -> Vec<GpuProcessInfo> {
  second
    .iter()
    .filter_map(|s2| {
      let gpu_percent = first
        .iter()
        .find(|s1| s1.pid == s2.pid)
        .map(|s1| {
          let wall_delta = s2.wall_ns.saturating_sub(s1.wall_ns);
          let gpu_delta = s2.gpu_time_ns.saturating_sub(s1.gpu_time_ns);
          if wall_delta > 0 {
            (gpu_delta as f64 / wall_delta as f64) * 100.0
          } else {
            0.0
          }
        })
        .unwrap_or(0.0);

      // Skip processes with no GPU activity (GPU time, active work, or VRAM).
      let has_vram = s2.vram_bytes.map(|v| v > 0).unwrap_or(false);
      if s2.gpu_time_ns == 0 && gpu_percent == 0.0 && !has_vram {
        return None;
      }

      Some(GpuProcessInfo {
        pid: s2.pid,
        name: s2.name.clone(),
        gpu_time_ns: s2.gpu_time_ns,
        gpu_percent,
        vram_bytes: s2.vram_bytes,
      })
    })
    .collect()
}
