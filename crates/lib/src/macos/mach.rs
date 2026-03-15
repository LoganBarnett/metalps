//! Per-process GPU memory (VRAM) via Mach task info.
//!
//! Uses `task_for_pid` + `task_info(TASK_VM_INFO)` to read the
//! `ledger_tag_graphics_footprint` field, which reports the Metal/GPU memory
//! footprint for a task in bytes.
//!
//! GPU execution time is obtained separately via IOKit `AGXDeviceUserClient`
//! entries (see `iokit.rs`), which is the source that Activity Monitor uses
//! on Apple Silicon.  `TASK_POWER_INFO_V2.task_gpu_utilisation` is not
//! populated for Metal compute workloads on Apple Silicon.

use crate::collector::CollectorError;
use std::ffi::c_void;

// ── Constants ─────────────────────────────────────────────────────────────────

const TASK_VM_INFO: u32 = 22;
const KERN_SUCCESS: i32 = 0;

// ── Struct definitions (from <mach/task_info.h> macOS 15 SDK) ────────────────

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

fn task_vm_info_count() -> u32 {
  (std::mem::size_of::<TaskVmInfo>() / std::mem::size_of::<u32>()) as u32
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Return the full process name for `pid`, or `"<pid>"` if unavailable.
pub fn process_name(pid: i32) -> String {
  let mut buf = [0u8; 256];
  let ret = unsafe {
    proc_name(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32)
  };
  if ret <= 0 {
    return format!("<{pid}>");
  }
  String::from_utf8_lossy(&buf[..ret as usize]).to_string()
}

/// Return the Metal/GPU memory footprint for `pid` in bytes, or `None` if the
/// process is inaccessible (different user, sandboxed, etc.).
pub fn vram_for_pid(pid: i32) -> Option<u64> {
  let task = task_port(pid).ok()?;
  let vram = query_vram(task).ok();
  unsafe {
    mach_port_deallocate(mach_task_self(), task);
  }
  vram
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
  Ok(info.ledger_tag_graphics_footprint.max(0) as u64)
}
