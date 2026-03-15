//! Device-level GPU stats via IOKit IOAccelerator registry entries.
//!
//! Queries `IOAccelerator` services (which includes `AGXAccelerator` on Apple
//! Silicon) and reads the `PerformanceStatistics` property dictionary.
//!
//! Known keys (vary by GPU family):
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

  fn IORegistryEntryGetName(entry: IOObject, name: *mut c_char) -> KernReturn;

  fn IOObjectRelease(object: IOObject) -> KernReturn;
}

const IO_OBJECT_NULL: IOObject = 0;
// kIOMasterPortDefault == 0
const K_IO_MASTER_PORT_DEFAULT: u32 = 0;

// ── Public API ────────────────────────────────────────────────────────────────

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
