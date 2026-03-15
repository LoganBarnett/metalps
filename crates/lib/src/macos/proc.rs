//! Process enumeration via libproc.

use crate::collector::CollectorError;
use std::ffi::c_void;

const PROC_ALL_PIDS: u32 = 1;

extern "C" {
  fn proc_listpids(
    type_: u32,
    typeinfo: u32,
    buffer: *mut c_void,
    buffersize: i32,
  ) -> i32;
}

/// Return a list of all active PIDs on the system.
pub fn list_pids() -> Result<Vec<i32>, CollectorError> {
  // First call with null buffer returns the number of bytes needed.
  let needed =
    unsafe { proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
  if needed <= 0 {
    return Err(CollectorError::ProcessList(format!(
      "proc_listpids sizing call returned {needed}"
    )));
  }

  // Allocate with some headroom for new processes created between calls.
  let capacity = (needed as usize / std::mem::size_of::<i32>()) + 32;
  let mut buf: Vec<i32> = vec![0i32; capacity];

  let ret = unsafe {
    proc_listpids(
      PROC_ALL_PIDS,
      0,
      buf.as_mut_ptr() as *mut c_void,
      (buf.len() * std::mem::size_of::<i32>()) as i32,
    )
  };

  if ret <= 0 {
    return Err(CollectorError::ProcessList(format!(
      "proc_listpids returned {ret}"
    )));
  }

  let count = ret as usize / std::mem::size_of::<i32>();
  buf.truncate(count);
  // PID 0 is the kernel task; skip it.
  buf.retain(|&pid| pid > 0);
  Ok(buf)
}
