//! metalps-lib — macOS GPU process monitoring library
//!
//! # LLM Development Guidelines
//! When modifying this code:
//! - Keep platform FFI in the `macos` module
//! - Use semantic error types with thiserror - NO anyhow blindly wrapping errors
//! - Add context at each error site explaining WHAT failed and WHY
//! - Keep all public types in `types` so callers don't need to know about
//!   platform internals
//! - The collector samples GPU stats twice and computes deltas for GPU%

pub mod collector;
pub mod logging;
pub mod output;
pub mod types;

#[cfg(target_os = "macos")]
mod macos;

pub use logging::{LogFormat, LogLevel};
pub use types::{DeviceGpuInfo, GpuOutput, GpuProcessInfo, SortKey};
