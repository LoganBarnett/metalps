//! Integration tests for metalps.
//!
//! The `gpu_detection` test starts `metalps-gpu-load` to warm up the GPU,
//! then runs `metalps --json` and verifies the GPU load process appears in
//! the output with non-zero GPU activity.
//!
//! Both binaries run as the same OS user, so `task_for_pid` can inspect
//! the GPU load process without elevated privileges.

use std::{
  path::PathBuf,
  process::{Child, Command},
  time::Duration,
};

fn binary_path(name: &str) -> PathBuf {
  // Navigate from the test executable location to the target directory.
  let mut path =
    std::env::current_exe().expect("Failed to get test executable path");
  path.pop(); // remove test binary name
  if path.ends_with("deps") {
    path.pop(); // remove deps/
  }
  path.push(name);

  if !path.exists() {
    // Try one level up (debug vs release)
    path.pop();
    path.pop();
    path.push("debug");
    path.push(name);
  }

  path
}

fn metalps_path() -> PathBuf {
  binary_path("metalps")
}

fn gpu_load_path() -> PathBuf {
  binary_path("metalps-gpu-load")
}

/// Guard that kills the child process when dropped.
struct ChildGuard(Child);
impl Drop for ChildGuard {
  fn drop(&mut self) {
    let _ = self.0.kill();
    let _ = self.0.wait();
  }
}

// ── Basic CLI tests ───────────────────────────────────────────────────────────

#[test]
fn test_help_flag() {
  let path = metalps_path();
  let output = Command::new(&path)
    .arg("--help")
    .output()
    .unwrap_or_else(|e| {
      panic!(
        "Failed to run {}: {e}\nBuild with: cargo build --workspace",
        path.display()
      )
    });

  assert!(
    output.status.success(),
    "Expected success, got: {:?}\nstderr: {}",
    output.status,
    String::from_utf8_lossy(&output.stderr)
  );
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert!(
    stdout.contains("GPU process monitor") || stdout.contains("Usage:"),
    "help output should describe the tool; got:\n{stdout}"
  );
}

#[test]
fn test_version_flag() {
  let path = metalps_path();
  let output = Command::new(&path)
    .arg("--version")
    .output()
    .unwrap_or_else(|e| panic!("Failed to run {}: {e}", path.display()));

  assert!(output.status.success());
  let stdout = String::from_utf8_lossy(&output.stdout);
  assert!(
    stdout.contains("metalps"),
    "version output should contain 'metalps'; got:\n{stdout}"
  );
}

#[test]
fn test_json_output_is_valid() {
  let path = metalps_path();
  let output = Command::new(&path)
    .args(["--json", "--interval-ms", "200"])
    .output()
    .unwrap_or_else(|e| panic!("Failed to run {}: {e}", path.display()));

  // Allow non-zero exit if running without GPU (e.g. CI VM) — the output
  // should still be valid JSON in that case.
  let stdout = String::from_utf8_lossy(&output.stdout);
  let parsed: serde_json::Value =
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
      panic!(
        "stdout is not valid JSON: {e}\nstdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
      )
    });

  assert!(
    parsed.get("processes").is_some(),
    "'processes' key missing from JSON output:\n{parsed:#}"
  );
  assert!(
    parsed.get("devices").is_some(),
    "'devices' key missing from JSON output:\n{parsed:#}"
  );
  assert!(
    parsed.get("sample_interval_ms").is_some(),
    "'sample_interval_ms' key missing:\n{parsed:#}"
  );
}

#[test]
fn test_human_output_runs_without_error() {
  let path = metalps_path();
  let output = Command::new(&path)
    .args(["--interval-ms", "200"])
    .output()
    .unwrap_or_else(|e| panic!("Failed to run {}: {e}", path.display()));

  // A zero exit code or "no GPU activity" message are both acceptable.
  let stdout = String::from_utf8_lossy(&output.stdout);
  let stderr = String::from_utf8_lossy(&output.stderr);
  assert!(
    output.status.success(),
    "metalps exited with error\nstdout: {stdout}\nstderr: {stderr}"
  );
  assert!(
    stdout.contains("metalps") || stdout.contains("GPU"),
    "expected GPU-related output; got:\n{stdout}"
  );
}

// ── GPU detection test ────────────────────────────────────────────────────────

/// Sign a binary with ad-hoc code signature + `com.apple.security.get-task-allow`.
///
/// This entitlement allows OTHER processes (running as the same user) to call
/// `task_for_pid` on the signed binary without root.  Xcode adds this
/// automatically to debug builds; we replicate it here for Cargo builds.
///
/// Returns false if codesign is not available or signing fails.
fn sign_with_get_task_allow(binary: &std::path::Path) -> bool {
  // Write entitlements plist to a temp file.
  let ent_path = std::env::temp_dir().join("metalps-get-task-allow.plist");
  let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.get-task-allow</key><true/>
</dict>
</plist>"#;
  if std::fs::write(&ent_path, plist).is_err() {
    return false;
  }

  Command::new("codesign")
    .args([
      "-s",
      "-",
      "--entitlements",
      ent_path.to_str().unwrap(),
      "--force",
      binary.to_str().unwrap(),
    ])
    .status()
    .map(|s| s.success())
    .unwrap_or(false)
}

/// Start `metalps-gpu-load`, wait for it to warm up, then verify that
/// `metalps --json` reports it as a process with GPU activity.
///
/// Requires a Metal-capable GPU (any Apple Silicon or Intel Mac with GPU).
/// Skipped gracefully if the gpu-load binary is not found or codesign is
/// unavailable.
///
/// Note on macOS security: `task_for_pid` (used to read per-process GPU
/// stats) requires the TARGET process to have the
/// `com.apple.security.get-task-allow` entitlement when calling code is
/// not root.  Xcode adds this automatically for debug builds.  We use
/// `codesign` here to replicate that behaviour for Cargo builds.
#[test]
fn test_detects_gpu_load_process() {
  let gpu_load = gpu_load_path();
  if !gpu_load.exists() {
    eprintln!(
      "Skipping gpu detection test: {} not found.\n\
       Build with: cargo build --workspace",
      gpu_load.display()
    );
    return;
  }

  // Sign gpu-load with get-task-allow so metalps can inspect it via
  // task_for_pid without requiring root.
  if !sign_with_get_task_allow(&gpu_load) {
    eprintln!(
      "Skipping gpu detection test: could not sign {} with \
       get-task-allow entitlement.\n\
       Ensure codesign is available (Xcode Command Line Tools).",
      gpu_load.display()
    );
    return;
  }
  eprintln!("Signed {} with get-task-allow", gpu_load.display());

  // Spawn the GPU load generator.
  let child = Command::new(&gpu_load)
    .arg("--duration")
    .arg("30") // run for 30s so our sample window fits
    .spawn()
    .unwrap_or_else(|e| panic!("Failed to start {}: {e}", gpu_load.display()));
  let _guard = ChildGuard(child);
  let gpu_load_pid = _guard.0.id() as i32;

  // Give the GPU time to warm up.
  std::thread::sleep(Duration::from_secs(2));

  // Run metalps targeting just this PID.
  let metalps = metalps_path();
  let output = Command::new(&metalps)
    .args([
      "--json",
      "--interval-ms",
      "500",
      "--pid",
      &gpu_load_pid.to_string(),
    ])
    .output()
    .unwrap_or_else(|e| panic!("Failed to run {}: {e}", metalps.display()));

  assert!(
    output.status.success(),
    "metalps failed\nstdout: {}\nstderr: {}",
    String::from_utf8_lossy(&output.stdout),
    String::from_utf8_lossy(&output.stderr)
  );

  let stdout = String::from_utf8_lossy(&output.stdout);
  let parsed: serde_json::Value = serde_json::from_str(&stdout)
    .unwrap_or_else(|e| panic!("Not valid JSON: {e}\noutput:\n{stdout}"));

  let processes = parsed["processes"]
    .as_array()
    .expect("'processes' should be an array");

  // Find the gpu-load process in the output.
  let gpu_proc = processes
    .iter()
    .find(|p| p["pid"].as_i64() == Some(gpu_load_pid as i64));

  let gpu_proc = gpu_proc.unwrap_or_else(|| {
    panic!(
      "gpu-load PID {gpu_load_pid} not found in output.\n\
       processes: {processes:#?}"
    )
  });

  let gpu_time_ns = gpu_proc["gpu_time_ns"].as_u64().unwrap_or(0);
  let gpu_percent = gpu_proc["gpu_percent"].as_f64().unwrap_or(0.0);
  let vram_bytes = gpu_proc["vram_bytes"].as_u64().unwrap_or(0);

  eprintln!(
    "GPU load detected: PID={gpu_load_pid}, \
     gpu%={gpu_percent:.1}, gpu_time_ns={gpu_time_ns}, vram={vram_bytes}B"
  );

  // At least one GPU indicator must be positive.  On Apple Silicon,
  // `task_gpu_utilisation` may be 0 even for active Metal compute;
  // in that case VRAM allocation (ledger_tag_graphics_footprint) proves
  // GPU usage.  The gpu-load binary allocates a 4 MB MTLBuffer.
  assert!(
    gpu_time_ns > 0 || gpu_percent > 0.0 || vram_bytes > 0,
    "Expected gpu_time_ns, gpu_percent, or vram_bytes > 0 for \
     metalps-gpu-load (PID {gpu_load_pid}). \
     All are zero — task_for_pid may have failed.\n\
     Try running with: sudo cargo test"
  );
}
