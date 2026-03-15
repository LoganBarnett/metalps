//! GPU stats via IOKit IOAccelerator registry entries.
//!
//! Two queries are provided:
//!
//! 1. `query_client_gpu_times()` — per-process cumulative GPU time by
//!    iterating `AGXDeviceUserClient` children of each accelerator.  Each
//!    client carries an `AppUsage` array whose `accumulatedGPUTime` fields
//!    (nanoseconds) sum to the total GPU execution time for that process.
//!    This is the source Activity Monitor uses on Apple Silicon.
//!
//! 2. `query_devices()` — device-level GPU% and VRAM from the
//!    `PerformanceStatistics` dictionary on the accelerator itself.
//!
//! Known PerformanceStatistics keys (vary by GPU family):
//!   Apple Silicon (AGXAccelerator):
//!     "Device Utilization %"    — float, overall GPU %
//!     "In use system memory"    — int, bytes used by Metal clients
//!     "Allocated system memory" — int, bytes allocated
//!   Intel / AMD (discrete):
//!     "Device Utilization %"    — float
//!     "vramUsedBytes"           — int
//!     "vramFreeBytes"           — int
//!     "vramTotalBytes"          — int
//!     "model"                   — string (renderer name)

use crate::types::DeviceGpuInfo;
use core_foundation_sys::{
  array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef},
  base::{CFRelease, CFTypeRef},
  dictionary::{CFDictionaryGetValue, CFDictionaryRef},
  number::{
    kCFNumberFloat64Type, kCFNumberSInt64Type, CFNumberGetValue, CFNumberRef,
  },
  string::{
    kCFStringEncodingUTF8, CFStringCreateWithCString, CFStringGetCString,
    CFStringGetLength, CFStringGetMaximumSizeForEncoding, CFStringRef,
  },
};
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};

// ── IOKit FFI ─────────────────────────────────────────────────────────────────

type IOObject = u32;
type IoIterator = u32;
type KernReturn = i32;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
  fn IOServiceGetMatchingServices(
    main_port: u32,
    matching: *mut c_void,
    existing: *mut IoIterator,
  ) -> KernReturn;

  fn IOServiceMatching(name: *const c_char) -> *mut c_void;

  fn IOIteratorNext(iterator: IoIterator) -> IOObject;

  fn IORegistryEntryCreateCFProperties(
    entry: IOObject,
    properties: *mut *mut c_void,
    allocator: *const c_void,
    options: u32,
  ) -> KernReturn;

  fn IORegistryEntryGetChildIterator(
    entry: IOObject,
    plane: *const c_char,
    iterator: *mut IoIterator,
  ) -> KernReturn;

  fn IORegistryEntryGetName(entry: IOObject, name: *mut c_char) -> KernReturn;

  fn IOObjectRelease(object: IOObject) -> KernReturn;
}

const IO_OBJECT_NULL: IOObject = 0;
// kIOMasterPortDefault == 0
const K_IO_MASTER_PORT_DEFAULT: u32 = 0;

// ── Public API ────────────────────────────────────────────────────────────────

/// Per-process cumulative GPU time from the IOKit client registry.
pub struct ClientGpuSample {
  pub pid: i32,
  /// Sum of `accumulatedGPUTime` across all command queues and all client
  /// connections for this PID, in nanoseconds (cumulative since process start).
  pub accumulated_gpu_time_ns: u64,
}

/// Query `AGXDeviceUserClient` children of every `IOAccelerator` to collect
/// per-process cumulative GPU execution time.
///
/// Returns one entry per PID that has an active GPU client connection.
/// Never fails — returns an empty vec if IOKit is unavailable.
pub fn query_client_gpu_times() -> Vec<ClientGpuSample> {
  unsafe { query_client_gpu_times_inner() }
}

unsafe fn query_client_gpu_times_inner() -> Vec<ClientGpuSample> {
  let service_name = CStr::from_bytes_with_nul(b"IOAccelerator\0").unwrap();
  let matching = IOServiceMatching(service_name.as_ptr());
  if matching.is_null() {
    return vec![];
  }

  let mut acc_iter: IoIterator = 0;
  let kr = IOServiceGetMatchingServices(
    K_IO_MASTER_PORT_DEFAULT,
    matching,
    &mut acc_iter,
  );
  if kr != 0 {
    return vec![];
  }

  // Accumulate GPU time by PID; a process may have multiple client connections.
  let mut pid_map: HashMap<i32, u64> = HashMap::new();

  loop {
    let accelerator = IOIteratorNext(acc_iter);
    if accelerator == IO_OBJECT_NULL {
      break;
    }

    let plane = CStr::from_bytes_with_nul(b"IOService\0").unwrap();
    let mut child_iter: IoIterator = 0;
    let kr2 = IORegistryEntryGetChildIterator(
      accelerator,
      plane.as_ptr(),
      &mut child_iter,
    );
    if kr2 == 0 {
      loop {
        let child = IOIteratorNext(child_iter);
        if child == IO_OBJECT_NULL {
          break;
        }
        if let Some((pid, gpu_ns)) = extract_client_gpu_time(child) {
          *pid_map.entry(pid).or_insert(0) += gpu_ns;
        }
        IOObjectRelease(child);
      }
      IOObjectRelease(child_iter);
    }

    IOObjectRelease(accelerator);
  }
  IOObjectRelease(acc_iter);

  pid_map
    .into_iter()
    .map(|(pid, accumulated_gpu_time_ns)| ClientGpuSample {
      pid,
      accumulated_gpu_time_ns,
    })
    .collect()
}

/// Read `IOUserClientCreator` and `AppUsage` from a single child entry.
/// Returns `None` if the entry is not a GPU user client.
unsafe fn extract_client_gpu_time(service: IOObject) -> Option<(i32, u64)> {
  let mut props_ptr: *mut c_void = std::ptr::null_mut();
  let kr = IORegistryEntryCreateCFProperties(
    service,
    &mut props_ptr,
    std::ptr::null(),
    0,
  );
  if kr != 0 || props_ptr.is_null() {
    return None;
  }
  let props = props_ptr as CFDictionaryRef;

  // Read creator string before any early return so we always release props.
  let creator = get_cf_string(props, "IOUserClientCreator");
  let gpu_ns = sum_app_usage_gpu_time(props);
  CFRelease(props as CFTypeRef);

  let pid = creator.as_deref().and_then(parse_creator_pid)?;
  Some((pid, gpu_ns))
}

/// Parse the PID from `IOUserClientCreator` = `"pid 427, WindowServer"`.
fn parse_creator_pid(s: &str) -> Option<i32> {
  s.strip_prefix("pid ")?
    .split(',')
    .next()?
    .trim()
    .parse()
    .ok()
}

/// Sum `accumulatedGPUTime` (ns) across all entries in the `AppUsage` CFArray.
unsafe fn sum_app_usage_gpu_time(dict: CFDictionaryRef) -> u64 {
  let cf_key = make_cf_string("AppUsage");
  let val = CFDictionaryGetValue(dict, cf_key as *const c_void) as CFArrayRef;
  CFRelease(cf_key as CFTypeRef);
  if val.is_null() {
    return 0;
  }

  let mut total: u64 = 0;
  for i in 0..CFArrayGetCount(val) {
    let elem = CFArrayGetValueAtIndex(val, i) as CFDictionaryRef;
    if !elem.is_null() {
      total += get_cf_u64(elem, "accumulatedGPUTime").unwrap_or(0);
    }
  }
  total
}

/// Query all `IOAccelerator` entries and return device-level GPU info.
/// Never fails — returns an empty vec if IOKit is unavailable.
pub fn query_devices() -> Vec<DeviceGpuInfo> {
  unsafe { query_devices_inner() }
}

unsafe fn query_devices_inner() -> Vec<DeviceGpuInfo> {
  let service_name = CStr::from_bytes_with_nul(b"IOAccelerator\0").unwrap();
  let matching = IOServiceMatching(service_name.as_ptr());
  if matching.is_null() {
    return vec![];
  }

  let mut iterator: IoIterator = 0;
  let kr = IOServiceGetMatchingServices(
    K_IO_MASTER_PORT_DEFAULT,
    matching,
    &mut iterator,
  );
  if kr != 0 {
    return vec![];
  }

  let mut devices = Vec::new();

  loop {
    let service = IOIteratorNext(iterator);
    if service == IO_OBJECT_NULL {
      break;
    }

    if let Some(info) = extract_device_info(service) {
      devices.push(info);
    }

    IOObjectRelease(service);
  }

  IOObjectRelease(iterator);
  devices
}

unsafe fn extract_device_info(service: IOObject) -> Option<DeviceGpuInfo> {
  // Read the IOKit registry entry name (e.g. "AGXAcceleratorG14X")
  let mut entry_name_buf = [0i8; 128];
  IORegistryEntryGetName(service, entry_name_buf.as_mut_ptr());
  let entry_name = CStr::from_ptr(entry_name_buf.as_ptr())
    .to_string_lossy()
    .to_string();

  // Get the full property dictionary for this IOAccelerator entry.
  let mut props_ptr: *mut c_void = std::ptr::null_mut();
  let kr = IORegistryEntryCreateCFProperties(
    service,
    &mut props_ptr,
    std::ptr::null(),
    0,
  );
  if kr != 0 || props_ptr.is_null() {
    return None;
  }

  let props = props_ptr as CFDictionaryRef;

  // Try to find a PerformanceStatistics sub-dictionary.
  let perf_dict = get_cf_dict(props, "PerformanceStatistics").unwrap_or(props);

  // Device name: "model" is a top-level property (e.g. "Apple M1 Pro"),
  // not inside PerformanceStatistics.  Fall back to IOClass or entry name.
  let name = get_cf_string(props, "model")
    .or_else(|| get_cf_string(props, "IOClass"))
    .unwrap_or(entry_name);

  let gpu_percent = get_cf_f64(perf_dict, "Device Utilization %");

  // VRAM: discrete GPU keys first, then Apple Silicon unified-memory keys.
  let vram_total_bytes = get_cf_u64(perf_dict, "vramTotalBytes")
    .or_else(|| get_cf_u64(perf_dict, "Allocated system memory"));

  let vram_used_bytes = get_cf_u64(perf_dict, "vramUsedBytes")
    .or_else(|| get_cf_u64(perf_dict, "In use system memory"));

  CFRelease(props as CFTypeRef);

  Some(DeviceGpuInfo {
    name,
    gpu_percent,
    vram_total_bytes,
    vram_used_bytes,
  })
}

// ── CF dictionary helpers (raw C-level, no generics) ─────────────────────────

/// Create a CFStringRef from a Rust &str.
/// Caller is responsible for CFRelease.
unsafe fn make_cf_string(s: &str) -> CFStringRef {
  let cstr = CString::new(s).unwrap_or_default();
  CFStringCreateWithCString(
    std::ptr::null(),
    cstr.as_ptr(),
    kCFStringEncodingUTF8,
  )
}

/// Look up `key` in `dict` and return the value as a CFDictionaryRef,
/// or None if not found or not a dictionary.
unsafe fn get_cf_dict(
  dict: CFDictionaryRef,
  key: &str,
) -> Option<CFDictionaryRef> {
  let cf_key = make_cf_string(key);
  let val =
    CFDictionaryGetValue(dict, cf_key as *const c_void) as CFDictionaryRef;
  CFRelease(cf_key as CFTypeRef);
  if val.is_null() {
    None
  } else {
    Some(val)
  }
}

/// Extract an f64 from a CFNumber value in `dict[key]`.
unsafe fn get_cf_f64(dict: CFDictionaryRef, key: &str) -> Option<f64> {
  let cf_key = make_cf_string(key);
  let val = CFDictionaryGetValue(dict, cf_key as *const c_void);
  CFRelease(cf_key as CFTypeRef);
  if val.is_null() {
    return None;
  }
  let mut result: f64 = 0.0;
  let ok = CFNumberGetValue(
    val as CFNumberRef,
    kCFNumberFloat64Type,
    &mut result as *mut f64 as *mut c_void,
  );
  if ok {
    Some(result)
  } else {
    None
  }
}

/// Extract a u64 from a CFNumber value in `dict[key]`.
unsafe fn get_cf_u64(dict: CFDictionaryRef, key: &str) -> Option<u64> {
  let cf_key = make_cf_string(key);
  let val = CFDictionaryGetValue(dict, cf_key as *const c_void);
  CFRelease(cf_key as CFTypeRef);
  if val.is_null() {
    return None;
  }
  let mut result: i64 = 0;
  let ok = CFNumberGetValue(
    val as CFNumberRef,
    kCFNumberSInt64Type,
    &mut result as *mut i64 as *mut c_void,
  );
  if ok {
    Some(result.max(0) as u64)
  } else {
    None
  }
}

/// Extract a String from a CFString value in `dict[key]`.
unsafe fn get_cf_string(dict: CFDictionaryRef, key: &str) -> Option<String> {
  let cf_key = make_cf_string(key);
  let val = CFDictionaryGetValue(dict, cf_key as *const c_void) as CFStringRef;
  CFRelease(cf_key as CFTypeRef);
  if val.is_null() {
    return None;
  }
  let len = CFStringGetMaximumSizeForEncoding(
    CFStringGetLength(val),
    kCFStringEncodingUTF8,
  ) + 1;
  let mut buf = vec![0u8; len as usize];
  let ok = CFStringGetCString(
    val,
    buf.as_mut_ptr() as *mut c_char,
    len,
    kCFStringEncodingUTF8,
  );
  if ok != 0 {
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..end].to_vec()).ok()
  } else {
    None
  }
}
