//! Per-process GPU stats via Mach task info.
//!
//! Uses:
//! - `task_for_pid` to acquire the task port for a process
//! - `task_info(TASK_POWER_INFO_V2)` for GPU time
//! - `task_info(TASK_VM_INFO)` for graphics memory footprint
//!
//! `task_for_pid` requires the calling process and target to share a UID,
//! or root. On non-sandboxed developer machines it works for all own processes.

use crate::collector::CollectorError;
use crate::macos::RawSample;
use std::ffi::c_void;
use std::time::{SystemTime, UNIX_EPOCH};

// ── Mach constants ────────────────────────────────────────────────────────────

const TASK_POWER_INFO_V2: u32 = 26;
const TASK_VM_INFO: u32 = 22;
// KERN_SUCCESS
const KERN_SUCCESS: i32 = 0;

// ── Struct definitions (from <mach/task_info.h> macOS 15 SDK) ────────────────

#[repr(C)]
struct TaskPowerInfo {
  total_user: u64,
  total_system: u64,
  task_interrupt_wakeups: u64,
  task_platform_idle_wakeups: u64,
  task_timer_wakeups_bin_1: u64,
  task_timer_wakeups_bin_2: u64,
}

#[repr(C)]
struct GpuEnergyData {
  /// Cumulative GPU time for this task in nanoseconds.
  task_gpu_utilisation: u64,
  _reserved0: u64,
  _reserved1: u64,
  _reserved2: u64,
}

/// Layout matches the arm64 variant of `task_power_info_v2` which includes
/// `task_energy` (only present on __arm__ / __arm64__).
#[cfg(target_arch = "aarch64")]
#[repr(C)]
struct TaskPowerInfoV2 {
  cpu_energy: TaskPowerInfo,
  gpu_energy: GpuEnergyData,
  task_energy: u64,
  task_ptime: u64,
  task_pset_switches: u64,
}

#[cfg(target_arch = "x86_64")]
#[repr(C)]
struct TaskPowerInfoV2 {
  cpu_energy: TaskPowerInfo,
  gpu_energy: GpuEnergyData,
  task_ptime: u64,
  task_pset_switches: u64,
}

/// Full `task_vm_info` struct from macOS 15 SDK (rev7).
/// We only read up to `ledger_tag_graphics_footprint` (rev3 field).
#[repr(C)]
struct TaskVmInfo {
  virtual_size: u64,
  region_count: i32,
  page_size: i32,
  resident_size: u64,
  resident_size_peak: u64,
  device: u64,
  device_peak: u64,
  internal: u64,
  internal_peak: u64,
  external: u64,
  external_peak: u64,
  reusable: u64,
  reusable_peak: u64,
  purgeable_volatile_pmap: u64,
  purgeable_volatile_resident: u64,
  purgeable_volatile_virtual: u64,
  compressed: u64,
  compressed_peak: u64,
  compressed_lifetime: u64,
  // rev1
  phys_footprint: u64,
  // rev2
  min_address: u64,
  max_address: u64,
  // rev3
  ledger_phys_footprint_peak: i64,
  ledger_purgeable_nonvolatile: i64,
  ledger_purgeable_novolatile_compressed: i64,
  ledger_purgeable_volatile: i64,
  ledger_purgeable_volatile_compressed: i64,
  ledger_tag_network_nonvolatile: i64,
  ledger_tag_network_nonvolatile_compressed: i64,
  ledger_tag_network_volatile: i64,
  ledger_tag_network_volatile_compressed: i64,
  ledger_tag_media_footprint: i64,
  ledger_tag_media_footprint_compressed: i64,
  ledger_tag_media_nofootprint: i64,
  ledger_tag_media_nofootprint_compressed: i64,
  /// Graphics (Metal/GPU) memory footprint for this task, in bytes.
  ledger_tag_graphics_footprint: i64,
  ledger_tag_graphics_footprint_compressed: i64,
  ledger_tag_graphics_nofootprint: i64,
  ledger_tag_graphics_nofootprint_compressed: i64,
  ledger_tag_neural_footprint: i64,
  ledger_tag_neural_footprint_compressed: i64,
  ledger_tag_neural_nofootprint: i64,
  ledger_tag_neural_nofootprint_compressed: i64,
  // rev4
  limit_bytes_remaining: u64,
  // rev5
  decompressions: i32,
  _pad: i32,
  // rev6
  ledger_swapins: i64,
  // rev7
  ledger_tag_neural_nofootprint_total: i64,
  ledger_tag_neural_nofootprint_peak: i64,
}

// ── FFI declarations ──────────────────────────────────────────────────────────

extern "C" {
  // Global Mach port for the current task. The C macro `mach_task_self()`
  // expands to reading this global.
  static mach_task_self_: u32;

  fn task_for_pid(target_tport: u32, pid: i32, t: *mut u32) -> i32;

  fn task_info(
    task: u32,
    flavor: u32,
    info: *mut c_void,
    count: *mut u32,
  ) -> i32;

  fn mach_port_deallocate(task: u32, name: u32) -> i32;

  fn proc_name(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
}

fn mach_task_self() -> u32 {
  unsafe { mach_task_self_ }
}

// ── Count constants (sizeof struct / sizeof natural_t where natural_t = u32) ─

fn task_power_info_v2_count() -> u32 {
  (std::mem::size_of::<TaskPowerInfoV2>() / std::mem::size_of::<u32>()) as u32
}

fn task_vm_info_count() -> u32 {
  (std::mem::size_of::<TaskVmInfo>() / std::mem::size_of::<u32>()) as u32
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Collect a GPU sample for a single process.
///
/// Returns `CollectorError::PermissionDenied` if we cannot inspect the
/// process (different user, sandboxed, SIP-protected).
pub fn sample_process(pid: i32) -> Result<RawSample, CollectorError> {
  let wall_ns = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos() as u64;

  let name = process_name(pid);

  // Acquire the task port.
  let task = task_port(pid)?;

  let gpu_time_ns = query_gpu_time(task).unwrap_or(0);
  let vram_bytes = query_vram(task).ok();

  // Release the task port we borrowed.
  unsafe {
    mach_port_deallocate(mach_task_self(), task);
  }

  // If GPU time is zero, this process hasn't touched the GPU.
  if gpu_time_ns == 0 && vram_bytes.map(|v| v == 0).unwrap_or(true) {
    return Err(CollectorError::PermissionDenied { pid });
  }

  Ok(RawSample {
    pid,
    name,
    gpu_time_ns,
    vram_bytes,
    wall_ns,
  })
}

/// Like `sample_process` but never returns PermissionDenied — always returns
/// a sample (with zeroed GPU fields if inaccessible). Used by the collector
/// when the caller explicitly requests a specific PID.
pub fn sample_process_force(pid: i32) -> RawSample {
  let wall_ns = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos() as u64;

  let name = process_name(pid);

  let Ok(task) = task_port(pid) else {
    return RawSample {
      pid,
      name,
      gpu_time_ns: 0,
      vram_bytes: None,
      wall_ns,
    };
  };

  let gpu_time_ns = query_gpu_time(task).unwrap_or(0);
  let vram_bytes = query_vram(task).ok();

  unsafe {
    mach_port_deallocate(mach_task_self(), task);
  }

  RawSample {
    pid,
    name,
    gpu_time_ns,
    vram_bytes,
    wall_ns,
  }
}

// ── Internals ─────────────────────────────────────────────────────────────────

fn task_port(pid: i32) -> Result<u32, CollectorError> {
  let mut task: u32 = 0;
  let kr = unsafe { task_for_pid(mach_task_self(), pid, &mut task) };
  if kr != KERN_SUCCESS {
    return Err(CollectorError::PermissionDenied { pid });
  }
  Ok(task)
}

fn process_name(pid: i32) -> String {
  let mut buf = [0u8; 256];
  let ret = unsafe {
    proc_name(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32)
  };
  if ret <= 0 {
    return format!("<{pid}>");
  }
  let len = ret as usize;
  String::from_utf8_lossy(&buf[..len]).to_string()
}

fn query_gpu_time(task: u32) -> Result<u64, CollectorError> {
  let mut info = std::mem::MaybeUninit::<TaskPowerInfoV2>::zeroed();
  let mut count = task_power_info_v2_count();

  let kr = unsafe {
    task_info(
      task,
      TASK_POWER_INFO_V2,
      info.as_mut_ptr() as *mut c_void,
      &mut count,
    )
  };

  if kr != KERN_SUCCESS {
    return Err(CollectorError::GpuQuery {
      detail: format!("task_info(TASK_POWER_INFO_V2) returned {kr}"),
    });
  }

  let info = unsafe { info.assume_init() };
  Ok(info.gpu_energy.task_gpu_utilisation)
}

fn query_vram(task: u32) -> Result<u64, CollectorError> {
  let mut info = std::mem::MaybeUninit::<TaskVmInfo>::zeroed();
  let mut count = task_vm_info_count();

  let kr = unsafe {
    task_info(task, TASK_VM_INFO, info.as_mut_ptr() as *mut c_void, &mut count)
  };

  if kr != KERN_SUCCESS {
    return Err(CollectorError::GpuQuery {
      detail: format!("task_info(TASK_VM_INFO) returned {kr}"),
    });
  }

  let info = unsafe { info.assume_init() };
  // The ledger value can be negative if underflow occurs; clamp to 0.
  let graphics_bytes = info.ledger_tag_graphics_footprint.max(0) as u64;
  Ok(graphics_bytes)
}
