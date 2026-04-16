use metalps_lib::{DeviceGpuInfo, GpuOutput, GpuProcessInfo, SortKey};
use prometheus::{GaugeVec, Opts, Registry};
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;
use tracing::warn;

pub struct GpuMetrics {
  registry: Arc<Registry>,
  device_gpu_utilization: GaugeVec,
  device_vram_used: GaugeVec,
  device_vram_total: GaugeVec,
  process_gpu_utilization: GaugeVec,
  process_gpu_time: GaugeVec,
  process_vram: GaugeVec,
}

impl GpuMetrics {
  pub fn new() -> Self {
    let registry = Registry::new();

    let device_gpu_utilization = GaugeVec::new(
      Opts::new(
        "metalps_device_gpu_utilization_ratio",
        "Device-wide GPU utilization as a ratio (0.0-1.0).",
      ),
      &["device"],
    )
    .expect("Metric creation should not fail for valid opts.");

    let device_vram_used = GaugeVec::new(
      Opts::new(
        "metalps_device_vram_used_bytes",
        "In-use GPU memory in bytes.",
      ),
      &["device"],
    )
    .expect("Metric creation should not fail for valid opts.");

    let device_vram_total = GaugeVec::new(
      Opts::new(
        "metalps_device_vram_total_bytes",
        "Total GPU-accessible memory in bytes.",
      ),
      &["device"],
    )
    .expect("Metric creation should not fail for valid opts.");

    let process_gpu_utilization = GaugeVec::new(
      Opts::new(
        "metalps_process_gpu_utilization_ratio",
        "Per-process GPU utilization as a ratio (0.0-1.0).",
      ),
      &["pid", "name"],
    )
    .expect("Metric creation should not fail for valid opts.");

    let process_gpu_time = GaugeVec::new(
      Opts::new(
        "metalps_process_gpu_time_seconds_total",
        "Cumulative GPU time in seconds since process start.",
      ),
      &["pid", "name"],
    )
    .expect("Metric creation should not fail for valid opts.");

    let process_vram = GaugeVec::new(
      Opts::new(
        "metalps_process_vram_bytes",
        "Per-process GPU memory footprint in bytes.",
      ),
      &["pid", "name"],
    )
    .expect("Metric creation should not fail for valid opts.");

    registry
      .register(Box::new(device_gpu_utilization.clone()))
      .expect("Registration should not fail for fresh registry.");
    registry
      .register(Box::new(device_vram_used.clone()))
      .expect("Registration should not fail for fresh registry.");
    registry
      .register(Box::new(device_vram_total.clone()))
      .expect("Registration should not fail for fresh registry.");
    registry
      .register(Box::new(process_gpu_utilization.clone()))
      .expect("Registration should not fail for fresh registry.");
    registry
      .register(Box::new(process_gpu_time.clone()))
      .expect("Registration should not fail for fresh registry.");
    registry
      .register(Box::new(process_vram.clone()))
      .expect("Registration should not fail for fresh registry.");

    Self {
      registry: Arc::new(registry),
      device_gpu_utilization,
      device_vram_used,
      device_vram_total,
      process_gpu_utilization,
      process_gpu_time,
      process_vram,
    }
  }

  pub fn registry(&self) -> Arc<Registry> {
    self.registry.clone()
  }

  /// Replace all gauge values with fresh data from the latest collection.
  ///
  /// The reset-then-set pattern ensures that processes which have exited
  /// (and therefore no longer appear in `output`) are immediately removed
  /// from the exported metrics rather than lingering with stale values.
  pub fn update(&self, output: &GpuOutput) {
    self.device_gpu_utilization.reset();
    self.device_vram_used.reset();
    self.device_vram_total.reset();
    self.process_gpu_utilization.reset();
    self.process_gpu_time.reset();
    self.process_vram.reset();

    for device in &output.devices {
      self.update_device(device);
    }

    for process in &output.processes {
      self.update_process(process);
    }
  }

  fn update_device(&self, device: &DeviceGpuInfo) {
    if let Some(pct) = device.gpu_percent {
      self
        .device_gpu_utilization
        .with_label_values(&[&device.name])
        .set(pct / 100.0);
    }
    if let Some(used) = device.vram_used_bytes {
      self
        .device_vram_used
        .with_label_values(&[&device.name])
        .set(used as f64);
    }
    if let Some(total) = device.vram_total_bytes {
      self
        .device_vram_total
        .with_label_values(&[&device.name])
        .set(total as f64);
    }
  }

  fn update_process(&self, process: &GpuProcessInfo) {
    let pid = process.pid.to_string();

    self
      .process_gpu_utilization
      .with_label_values(&[&pid, &process.name])
      .set(process.gpu_percent / 100.0);

    self
      .process_gpu_time
      .with_label_values(&[&pid, &process.name])
      .set(process.gpu_time_ns as f64 / 1_000_000_000.0);

    if let Some(vram) = process.vram_bytes {
      self
        .process_vram
        .with_label_values(&[&pid, &process.name])
        .set(vram as f64);
    }
  }
}

/// Spawn a background task that periodically collects GPU metrics.
///
/// The loop calls `collect_with_interval` (which itself sleeps for
/// `interval`) then pushes the result into the gauge vecs.  Transient
/// collection failures are logged and skipped so the exporter stays up.
pub fn spawn_collection_loop(
  metrics: Arc<GpuMetrics>,
  interval: Duration,
) -> JoinHandle<()> {
  tokio::spawn(async move {
    loop {
      let interval_clone = interval;
      let result = tokio::task::spawn_blocking(move || {
        metalps_lib::collector::collect_with_interval(
          interval_clone,
          SortKey::Pid,
          None,
        )
      })
      .await;

      match result {
        Ok(Ok(output)) => metrics.update(&output),
        Ok(Err(e)) => warn!("GPU collection failed: {e}"),
        Err(e) => warn!("GPU collection task panicked: {e}"),
      }
    }
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  fn sample_output() -> GpuOutput {
    GpuOutput {
      timestamp_ms: 1000,
      sample_interval_ms: 1000,
      devices: vec![DeviceGpuInfo {
        name: "Apple M1 Pro".to_string(),
        gpu_percent: Some(50.0),
        vram_total_bytes: Some(16 * 1024 * 1024 * 1024),
        vram_used_bytes: Some(4 * 1024 * 1024 * 1024),
      }],
      processes: vec![GpuProcessInfo {
        pid: 1234,
        name: "test-proc".to_string(),
        gpu_time_ns: 2_000_000_000,
        gpu_percent: 25.0,
        vram_bytes: Some(128 * 1024 * 1024),
      }],
    }
  }

  fn gauge_value(gauge_vec: &GaugeVec, labels: &[&str]) -> f64 {
    gauge_vec.with_label_values(labels).get()
  }

  #[test]
  fn test_update_sets_device_gauges() {
    let m = GpuMetrics::new();
    m.update(&sample_output());

    assert!(
      (gauge_value(&m.device_gpu_utilization, &["Apple M1 Pro"]) - 0.5).abs()
        < f64::EPSILON
    );
    assert_eq!(
      gauge_value(&m.device_vram_used, &["Apple M1 Pro"]) as u64,
      4 * 1024 * 1024 * 1024
    );
    assert_eq!(
      gauge_value(&m.device_vram_total, &["Apple M1 Pro"]) as u64,
      16 * 1024 * 1024 * 1024
    );
  }

  #[test]
  fn test_update_sets_process_gauges() {
    let m = GpuMetrics::new();
    m.update(&sample_output());

    assert!(
      (gauge_value(&m.process_gpu_utilization, &["1234", "test-proc"]) - 0.25)
        .abs()
        < f64::EPSILON
    );
    assert_eq!(
      gauge_value(&m.process_vram, &["1234", "test-proc"]) as u64,
      128 * 1024 * 1024
    );
  }

  #[test]
  fn test_stale_processes_removed() {
    let m = GpuMetrics::new();

    m.update(&sample_output());
    assert!(
      gauge_value(&m.process_gpu_utilization, &["1234", "test-proc"]) > 0.0
    );

    // Second update without the process — gauges should reset.
    let empty = GpuOutput {
      timestamp_ms: 2000,
      sample_interval_ms: 1000,
      devices: vec![],
      processes: vec![],
    };
    m.update(&empty);

    // After reset, with_label_values creates a fresh zero gauge.
    assert!(
      gauge_value(&m.process_gpu_utilization, &["1234", "test-proc"]).abs()
        < f64::EPSILON
    );
  }

  #[test]
  fn test_percent_to_ratio_conversion() {
    let m = GpuMetrics::new();
    let output = GpuOutput {
      timestamp_ms: 1000,
      sample_interval_ms: 1000,
      devices: vec![DeviceGpuInfo {
        name: "GPU".to_string(),
        gpu_percent: Some(100.0),
        vram_total_bytes: None,
        vram_used_bytes: None,
      }],
      processes: vec![],
    };
    m.update(&output);
    assert!(
      (gauge_value(&m.device_gpu_utilization, &["GPU"]) - 1.0).abs()
        < f64::EPSILON
    );
  }

  #[test]
  fn test_nanoseconds_to_seconds_conversion() {
    let m = GpuMetrics::new();
    let output = GpuOutput {
      timestamp_ms: 1000,
      sample_interval_ms: 1000,
      devices: vec![],
      processes: vec![GpuProcessInfo {
        pid: 42,
        name: "ns-test".to_string(),
        gpu_time_ns: 1_500_000_000,
        gpu_percent: 0.0,
        vram_bytes: None,
      }],
    };
    m.update(&output);
    assert!(
      (gauge_value(&m.process_gpu_time, &["42", "ns-test"]) - 1.5).abs()
        < f64::EPSILON
    );
  }
}
