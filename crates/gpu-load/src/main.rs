//! metalps-gpu-load — Metal compute workload generator
//!
//! Runs a Mandelbrot-set compute shader in a loop to keep the GPU busy.
//! Used by integration tests to create measurable GPU activity that
//! `metalps` can detect.
//!
//! Usage:
//!   metalps-gpu-load              # run forever
//!   metalps-gpu-load --duration 10  # run for 10 seconds then exit

use clap::Parser;
use metal::{
  CompileOptions, ComputePassDescriptor, Device, MTLResourceOptions, MTLSize,
};
use std::time::{Duration, Instant};
use thiserror::Error;

/// MSL compute kernel: Mandelbrot iteration over a 1024×1024 grid.
/// Each invocation performs up to MAX_ITER multiply-adds — sufficient to
/// generate measurable GPU time and VRAM utilisation.
const SHADER_SOURCE: &str = r#"
#include <metal_stdlib>
using namespace metal;

kernel void mandelbrot(
    device float* output [[buffer(0)]],
    uint2 gid [[thread_position_in_grid]])
{
    const uint WIDTH  = 1024;
    const uint HEIGHT = 1024;

    if (gid.x >= WIDTH || gid.y >= HEIGHT) return;

    float cx = (float(gid.x) / float(WIDTH))  * 3.5f - 2.5f;
    float cy = (float(gid.y) / float(HEIGHT)) * 2.0f - 1.0f;

    float x = 0.0f;
    float y = 0.0f;

    const int MAX_ITER = 512;
    int iter = 0;
    while (x * x + y * y <= 4.0f && iter < MAX_ITER) {
        float xtemp = x * x - y * y + cx;
        y = 2.0f * x * y + cy;
        x = xtemp;
        ++iter;
    }

    output[gid.y * WIDTH + gid.x] = float(iter) / float(MAX_ITER);
}
"#;

#[derive(Debug, Error)]
enum GpuLoadError {
  #[error("No Metal device found (requires Apple Silicon or discrete GPU)")]
  NoDevice,

  #[error("Failed to compile Metal shader: {0}")]
  ShaderCompile(String),

  #[error("Failed to create compute pipeline: {0}")]
  Pipeline(String),
}

#[derive(Parser, Debug)]
#[command(
  name = "metalps-gpu-load",
  about = "Metal compute workload generator for GPU monitoring tests"
)]
struct Args {
  /// Run for this many seconds then exit (0 = run forever)
  #[arg(long, default_value = "0")]
  duration: u64,

  /// Print progress every N dispatches
  #[arg(long, default_value = "10")]
  report_every: u32,
}

fn main() {
  let args = Args::parse();

  match run(args) {
    Ok(()) => {}
    Err(e) => {
      eprintln!("metalps-gpu-load error: {e}");
      std::process::exit(1);
    }
  }
}

fn run(args: Args) -> Result<(), GpuLoadError> {
  let device = Device::system_default().ok_or(GpuLoadError::NoDevice)?;

  eprintln!("GPU device: {}", device.name());

  // Compile the Mandelbrot shader.
  let opts = CompileOptions::new();
  let library = device
    .new_library_with_source(SHADER_SOURCE, &opts)
    .map_err(|e| GpuLoadError::ShaderCompile(e.to_string()))?;

  let function = library
    .get_function("mandelbrot", None)
    .map_err(|e| GpuLoadError::Pipeline(e.to_string()))?;

  let pipeline = device
    .new_compute_pipeline_state_with_function(&function)
    .map_err(|e| GpuLoadError::Pipeline(e.to_string()))?;

  // Allocate a 1024×1024 float buffer (~4 MB) on the GPU.
  let buf_len = 1024usize * 1024 * std::mem::size_of::<f32>();
  let output_buf =
    device.new_buffer(buf_len as u64, MTLResourceOptions::StorageModePrivate);

  let queue = device.new_command_queue();

  let deadline = if args.duration > 0 {
    Some(Instant::now() + Duration::from_secs(args.duration))
  } else {
    None
  };

  eprintln!(
    "Running Mandelbrot compute shader{}…",
    if args.duration > 0 {
      format!(" for {}s", args.duration)
    } else {
      " (Ctrl-C to stop)".to_string()
    }
  );

  let mut dispatches: u32 = 0;
  let start = Instant::now();

  loop {
    if let Some(d) = deadline {
      if Instant::now() >= d {
        break;
      }
    }

    // Encode and submit one compute pass.
    let cmd = queue.new_command_buffer();
    let pass_desc = ComputePassDescriptor::new();
    let encoder = cmd.compute_command_encoder_with_descriptor(pass_desc);

    encoder.set_compute_pipeline_state(&pipeline);
    encoder.set_buffer(0, Some(&output_buf), 0);

    let w = pipeline.thread_execution_width();
    let h = pipeline.max_total_threads_per_threadgroup() / w;
    let grid = MTLSize::new(1024, 1024, 1);
    let threads = MTLSize::new(w, h, 1);

    encoder.dispatch_threads(grid, threads);
    encoder.end_encoding();

    cmd.commit();
    cmd.wait_until_completed();

    dispatches += 1;
    if dispatches % args.report_every == 0 {
      let elapsed = start.elapsed().as_secs_f64();
      eprintln!(
        "  {dispatches} dispatches in {elapsed:.1}s \
         ({:.1}/s)",
        dispatches as f64 / elapsed
      );
    }
  }

  let elapsed = start.elapsed().as_secs_f64();
  eprintln!("Done. {dispatches} dispatches in {elapsed:.1}s");
  Ok(())
}
