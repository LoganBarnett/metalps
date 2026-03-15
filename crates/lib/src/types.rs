//! Public data types for GPU process monitoring output.

use serde::{Deserialize, Serialize};

/// Sort key for process output ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
  /// Sort by GPU utilization percentage (highest first).
  #[default]
  GpuPercent,
  /// Sort by cumulative GPU time (highest first).
  GpuTime,
  /// Sort by VRAM usage (highest first).
  Vram,
  /// Sort by process ID (ascending).
  Pid,
}

impl std::str::FromStr for SortKey {
  type Err = String;

  fn from_str(s: &str) -> Result<Self, Self::Err> {
    match s.to_lowercase().as_str() {
      "gpu" | "gpu-percent" | "gpu_percent" => Ok(SortKey::GpuPercent),
      "time" | "gpu-time" | "gpu_time" => Ok(SortKey::GpuTime),
      "vram" | "mem" | "memory" => Ok(SortKey::Vram),
      "pid" => Ok(SortKey::Pid),
      other => Err(format!(
        "Unknown sort key: {other}. Valid values: gpu, time, vram, pid"
      )),
    }
  }
}

/// Per-process GPU statistics, computed from two samples.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuProcessInfo {
  /// Process ID.
  pub pid: i32,
  /// Process name (truncated to 15 chars by the kernel).
  pub name: String,
  /// Cumulative GPU time used since process start, in nanoseconds.
  pub gpu_time_ns: u64,
  /// GPU utilization during the sample interval (0.0 – 100.0).
  pub gpu_percent: f64,
  /// GPU / graphics memory footprint in bytes, if available.
  /// On Apple Silicon this is the Metal buffer ledger entry.
  /// Returns None if the process could not be queried (permissions, etc.).
  pub vram_bytes: Option<u64>,
}

impl GpuProcessInfo {
  /// Format VRAM as a short human-readable string (e.g. "128.5M").
  pub fn vram_human(&self) -> String {
    format_bytes(self.vram_bytes)
  }

  /// Format GPU time as a short human-readable string (e.g. "2m30s").
  pub fn gpu_time_human(&self) -> String {
    format_duration_ns(self.gpu_time_ns)
  }
}

/// Device-level GPU statistics from IOKit.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeviceGpuInfo {
  /// Renderer / GPU model name (e.g. "Apple M3 Max").
  pub name: String,
  /// Device-wide GPU utilization percentage (0.0 – 100.0), if available.
  pub gpu_percent: Option<f64>,
  /// Total VRAM / GPU-accessible memory in bytes, if reported.
  pub vram_total_bytes: Option<u64>,
  /// In-use VRAM in bytes, if reported.
  pub vram_used_bytes: Option<u64>,
}

/// Top-level output bundle — everything metalps collects in one run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuOutput {
  /// Unix timestamp of the second sample (milliseconds).
  pub timestamp_ms: u64,
  /// Milliseconds between the two samples used to compute GPU%.
  pub sample_interval_ms: u64,
  /// Per-process GPU information.
  pub processes: Vec<GpuProcessInfo>,
  /// Device-level GPU info (one entry per GPU).
  pub devices: Vec<DeviceGpuInfo>,
}

// ── formatting helpers ────────────────────────────────────────────────────────

pub fn format_bytes(bytes: Option<u64>) -> String {
  match bytes {
    None => "-".to_string(),
    Some(b) if b < 1024 => format!("{b}B"),
    Some(b) if b < 1024 * 1024 => {
      format!("{:.1}K", b as f64 / 1024.0)
    }
    Some(b) if b < 1024 * 1024 * 1024 => {
      format!("{:.1}M", b as f64 / (1024.0 * 1024.0))
    }
    Some(b) => format!("{:.1}G", b as f64 / (1024.0 * 1024.0 * 1024.0)),
  }
}

pub fn format_duration_ns(ns: u64) -> String {
  let ms = ns / 1_000_000;
  if ms == 0 {
    return "0ms".to_string();
  }
  if ms < 1_000 {
    return format!("{ms}ms");
  }
  if ms < 60_000 {
    return format!("{:.1}s", ms as f64 / 1_000.0);
  }
  let minutes = ms / 60_000;
  let secs = (ms % 60_000) / 1_000;
  if minutes < 60 {
    return format!("{minutes}m{secs}s");
  }
  let hours = minutes / 60;
  let mins = minutes % 60;
  format!("{hours}h{mins}m")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn test_format_bytes() {
    assert_eq!(format_bytes(None), "-");
    assert_eq!(format_bytes(Some(0)), "0B");
    assert_eq!(format_bytes(Some(1023)), "1023B");
    assert_eq!(format_bytes(Some(1024)), "1.0K");
    assert_eq!(format_bytes(Some(1024 * 1024)), "1.0M");
    assert_eq!(format_bytes(Some(1024 * 1024 * 1024)), "1.0G");
  }

  #[test]
  fn test_format_duration_ns() {
    assert_eq!(format_duration_ns(0), "0ms");
    assert_eq!(format_duration_ns(500_000_000), "500ms");
    assert_eq!(format_duration_ns(1_500_000_000), "1.5s");
    assert_eq!(format_duration_ns(90_000_000_000), "1m30s");
    assert_eq!(format_duration_ns(3_600_000_000_000), "1h0m");
  }

  #[test]
  fn test_sort_key_from_str() {
    assert_eq!("gpu".parse::<SortKey>().unwrap(), SortKey::GpuPercent);
    assert_eq!("time".parse::<SortKey>().unwrap(), SortKey::GpuTime);
    assert_eq!("vram".parse::<SortKey>().unwrap(), SortKey::Vram);
    assert_eq!("pid".parse::<SortKey>().unwrap(), SortKey::Pid);
    assert!("invalid".parse::<SortKey>().is_err());
  }
}
