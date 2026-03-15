//! Human-readable and JSON output formatters.

use crate::types::{format_bytes, format_duration_ns, GpuOutput};

/// Render `GpuOutput` as a human-readable table to the given writer.
///
/// Output goes to stdout; logs go to stderr.
pub fn render_human<W: std::io::Write>(
  out: &mut W,
  data: &GpuOutput,
) -> std::io::Result<()> {
  // Header
  writeln!(out, "metalps — GPU Process Monitor")?;
  writeln!(
    out,
    "Sample interval: {:.1}s",
    data.sample_interval_ms as f64 / 1000.0
  )?;
  writeln!(out)?;

  // Process table
  if data.processes.is_empty() {
    writeln!(
      out,
      "No GPU activity detected.  (Run with --all to include idle processes.)"
    )?;
  } else {
    // Column widths
    let name_width = data
      .processes
      .iter()
      .map(|p| p.name.len())
      .max()
      .unwrap_or(4)
      .max(4)
      .min(30);

    writeln!(
      out,
      "{:<7} {:<width$}  {:>7}  {:>10}  {:>8}",
      "PID",
      "NAME",
      "GPU%",
      "GPU TIME",
      "VRAM",
      width = name_width,
    )?;
    writeln!(
      out,
      "{:-<7} {:-<width$}  {:->7}  {:->10}  {:->8}",
      "",
      "",
      "",
      "",
      "",
      width = name_width,
    )?;

    for p in &data.processes {
      let name = if p.name.len() > name_width {
        format!("{}…", &p.name[..name_width.saturating_sub(1)])
      } else {
        p.name.clone()
      };

      writeln!(
        out,
        "{:<7} {:<width$}  {:>6.1}%  {:>10}  {:>8}",
        p.pid,
        name,
        p.gpu_percent,
        format_duration_ns(p.gpu_time_ns),
        p.vram_human(),
        width = name_width,
      )?;
    }
  }

  // Device summary
  if !data.devices.is_empty() {
    writeln!(out)?;
    for dev in &data.devices {
      write!(out, "Device: {}", dev.name)?;
      if let Some(pct) = dev.gpu_percent {
        write!(out, " | GPU: {pct:.1}%")?;
      }
      if dev.vram_used_bytes.is_some() || dev.vram_total_bytes.is_some() {
        write!(
          out,
          " | VRAM: {} / {}",
          format_bytes(dev.vram_used_bytes),
          format_bytes(dev.vram_total_bytes),
        )?;
      }
      writeln!(out)?;
    }
  }

  Ok(())
}

/// Render `GpuOutput` as pretty-printed JSON to the given writer.
pub fn render_json<W: std::io::Write>(
  out: &mut W,
  data: &GpuOutput,
) -> std::io::Result<()> {
  let json = serde_json::to_string_pretty(data)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
  writeln!(out, "{json}")
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
  use super::*;
  use crate::types::{DeviceGpuInfo, GpuProcessInfo};

  fn sample_output() -> GpuOutput {
    GpuOutput {
      timestamp_ms: 1_700_000_000_000,
      sample_interval_ms: 1000,
      processes: vec![
        GpuProcessInfo {
          pid: 1234,
          name: "metalps-gpu-load".to_string(),
          gpu_time_ns: 5_000_000_000,
          gpu_percent: 42.5,
          vram_bytes: Some(128 * 1024 * 1024),
        },
        GpuProcessInfo {
          pid: 5678,
          name: "WindowServer".to_string(),
          gpu_time_ns: 120_000_000_000,
          gpu_percent: 3.1,
          vram_bytes: Some(45 * 1024 * 1024),
        },
      ],
      devices: vec![DeviceGpuInfo {
        name: "Apple M3 Max".to_string(),
        gpu_percent: Some(45.6),
        vram_total_bytes: Some(18 * 1024 * 1024 * 1024),
        vram_used_bytes: Some(173 * 1024 * 1024),
      }],
    }
  }

  #[test]
  fn test_render_human_contains_key_fields() {
    let data = sample_output();
    let mut buf = Vec::new();
    render_human(&mut buf, &data).unwrap();
    let text = String::from_utf8(buf).unwrap();

    assert!(text.contains("1234"), "should contain PID");
    assert!(text.contains("metalps-gpu-load"), "should contain name");
    assert!(text.contains("42.5"), "should contain gpu%");
    assert!(text.contains("128.0M"), "should contain vram");
    assert!(text.contains("Apple M3 Max"), "should contain device name");
  }

  #[test]
  fn test_render_json_is_valid() {
    let data = sample_output();
    let mut buf = Vec::new();
    render_json(&mut buf, &data).unwrap();
    let text = String::from_utf8(buf).unwrap();

    let parsed: serde_json::Value =
      serde_json::from_str(&text).expect("output must be valid JSON");
    assert!(parsed["processes"].is_array());
    assert_eq!(parsed["processes"][0]["pid"], 1234);
    assert_eq!(parsed["processes"][0]["name"], "metalps-gpu-load");
    assert!(parsed["devices"].is_array());
  }

  #[test]
  fn test_render_human_empty_processes() {
    let mut data = sample_output();
    data.processes.clear();
    let mut buf = Vec::new();
    render_human(&mut buf, &data).unwrap();
    let text = String::from_utf8(buf).unwrap();
    assert!(text.contains("No GPU activity"));
  }
}
