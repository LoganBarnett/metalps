//! macOS-specific GPU data collection.
//!
//! Two data sources are combined:
//!
//! 1. **Mach task info** (`task_for_pid` + `task_info`) — per-process:
//!    - `TASK_POWER_INFO_V2` → `gpu_energy.task_gpu_utilisation` (GPU time, ns)
//!    - `TASK_VM_INFO` → `ledger_tag_graphics_footprint` (VRAM bytes)
//!    `task_for_pid` works without root for same-user processes on
//!    non-sandboxed developer machines.
//!
//! 2. **IOKit IOAccelerator** — device-level:
//!    - `PerformanceStatistics` dictionary → GPU %, VRAM used/total

pub mod iokit;
pub mod mach;
pub mod proc;

use crate::collector::CollectorError;
use crate::types::{DeviceGpuInfo, GpuProcessInfo};

/// A raw per-process GPU sample collected at one point in time.
#[derive(Debug, Clone)]
pub struct RawSample {
  pub pid: i32,
  pub name: String,
  /// GPU time in nanoseconds (cumulative since process start).
  pub gpu_time_ns: u64,
  /// Graphics memory footprint in bytes (Metal ledger).
  pub vram_bytes: Option<u64>,
  /// Wall-clock nanoseconds when this sample was taken.
  pub wall_ns: u64,
}

/// Collect raw GPU samples for all accessible processes and device info.
pub fn collect() -> Result<(Vec<RawSample>, Vec<DeviceGpuInfo>), CollectorError>
{
  let pids = proc::list_pids()?;
  let mut samples = Vec::with_capacity(pids.len());

  for pid in pids {
    match mach::sample_process(pid) {
      Ok(s) => samples.push(s),
      Err(CollectorError::PermissionDenied { .. }) => {
        // Silently skip processes we can't inspect (different user, SIP, etc.)
      }
      Err(e) => tracing::debug!("Skipping PID {pid}: {e}"),
    }
  }

  let devices = iokit::query_devices();
  Ok((samples, devices))
}

/// Compute `GpuProcessInfo` for each process that appears in both snapshots,
/// using the wall-clock delta between them to calculate GPU utilization.
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

      // Skip processes with no GPU activity at all (reduces noise).
      // Include if the process has any GPU time, active GPU work, or
      // allocated graphics memory (VRAM on Apple Silicon).
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
