//! Integration tests for metalps-lib.
//!
//! These tests exercise the public API surface.  GPU-specific tests are
//! gated on `target_os = "macos"` because the underlying Mach/IOKit
//! calls are only available there.

use metalps_lib::types::{DeviceGpuInfo, GpuOutput, GpuProcessInfo, SortKey};

#[test]
fn test_sort_key_round_trip() {
  for input in ["gpu", "time", "vram", "pid"] {
    let key: SortKey = input.parse().expect("valid sort key");
    assert!(format!("{key:?}").len() > 0);
  }
}

#[test]
fn test_gpu_output_serializes_to_json() {
  let output = GpuOutput {
    timestamp_ms: 1000,
    sample_interval_ms: 500,
    devices: vec![DeviceGpuInfo {
      name: "Test GPU".to_string(),
      gpu_percent: Some(42.0),
      vram_total_bytes: Some(8 * 1024 * 1024 * 1024),
      vram_used_bytes: Some(2 * 1024 * 1024 * 1024),
    }],
    processes: vec![GpuProcessInfo {
      pid: 100,
      name: "test".to_string(),
      gpu_time_ns: 500_000_000,
      gpu_percent: 10.0,
      vram_bytes: Some(64 * 1024 * 1024),
    }],
  };

  let json =
    serde_json::to_string(&output).expect("serialization should succeed");
  let parsed: serde_json::Value =
    serde_json::from_str(&json).expect("output should be valid JSON");

  assert_eq!(parsed["devices"][0]["name"], "Test GPU");
  assert_eq!(parsed["processes"][0]["pid"], 100);
}

#[cfg(target_os = "macos")]
mod macos_tests {
  use metalps_lib::collector;
  use metalps_lib::types::SortKey;
  use std::time::Duration;

  #[test]
  fn test_collect_with_short_interval() {
    // A very short interval should succeed (even if GPU% is near zero).
    let result = collector::collect_with_interval(
      Duration::from_millis(100),
      SortKey::Pid,
      None,
    );
    assert!(
      result.is_ok(),
      "collection should succeed on macOS: {:?}",
      result.err()
    );

    let output = result.unwrap();
    assert!(output.devices.len() > 0, "should detect at least one GPU");
  }
}
