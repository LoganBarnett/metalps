//! High-level sampling API.
//!
//! `collect_once()` takes a single snapshot (no GPU% available).
//! `collect_with_interval()` takes two snapshots separated by `interval`
//! and computes GPU% from the delta.

use crate::types::{GpuOutput, GpuProcessInfo, SortKey};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CollectorError {
  #[error("Failed to enumerate processes: {0}")]
  ProcessList(String),

  #[error("Permission denied for PID {pid}")]
  PermissionDenied { pid: i32 },

  #[error("GPU query failed: {detail}")]
  GpuQuery { detail: String },
}

/// Collect GPU information by taking two samples separated by `interval`.
///
/// Processes that appear in both samples get a computed GPU%.
/// Only processes with any GPU activity (time > 0 or VRAM > 0) are included.
#[cfg(target_os = "macos")]
pub fn collect_with_interval(
  interval: Duration,
  sort: SortKey,
  pid_filter: Option<i32>,
) -> Result<GpuOutput, CollectorError> {
  use crate::macos;

  let (first_samples, _) = macos::collect()?;

  // If a specific PID was requested and isn't in the first snapshot, seed it.
  let first_samples = if let Some(pid) = pid_filter {
    if !first_samples.iter().any(|s| s.pid == pid) {
      let mut s = first_samples;
      s.push(macos::seed_sample(pid));
      s
    } else {
      first_samples
    }
  } else {
    first_samples
  };

  std::thread::sleep(interval);

  let (second_samples, devices) = macos::collect()?;

  // If filtering, ensure the target PID is in the second snapshot.
  let second_samples = if let Some(pid) = pid_filter {
    if !second_samples.iter().any(|s| s.pid == pid) {
      let mut s = second_samples;
      s.push(macos::seed_sample(pid));
      s
    } else {
      second_samples
    }
  } else {
    second_samples
  };

  let mut processes = macos::compute_gpu_info(&first_samples, &second_samples);

  if let Some(pid) = pid_filter {
    processes.retain(|p| p.pid == pid);
  }

  sort_processes(&mut processes, sort);

  let timestamp_ms = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_millis() as u64;

  Ok(GpuOutput {
    timestamp_ms,
    sample_interval_ms: interval.as_millis() as u64,
    processes,
    devices,
  })
}

#[cfg(not(target_os = "macos"))]
pub fn collect_with_interval(
  _interval: Duration,
  _sort: SortKey,
  _pid_filter: Option<i32>,
) -> Result<GpuOutput, CollectorError> {
  Err(CollectorError::ProcessList(
    "GPU monitoring is only supported on macOS".to_string(),
  ))
}

fn sort_processes(processes: &mut [GpuProcessInfo], sort: SortKey) {
  match sort {
    SortKey::GpuPercent => {
      processes.sort_by(|a, b| {
        b.gpu_percent
          .partial_cmp(&a.gpu_percent)
          .unwrap_or(std::cmp::Ordering::Equal)
      });
    }
    SortKey::GpuTime => {
      processes.sort_by(|a, b| b.gpu_time_ns.cmp(&a.gpu_time_ns));
    }
    SortKey::Vram => {
      processes.sort_by(|a, b| b.vram_bytes.cmp(&a.vram_bytes));
    }
    SortKey::Pid => {
      processes.sort_by_key(|p| p.pid);
    }
  }
}
