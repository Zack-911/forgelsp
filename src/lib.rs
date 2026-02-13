//! Library entry point for ForgeLSP.
//!
//! On native targets, this simply re-exports modules used by the binary.
//! On WASM, this provides `#[wasm_bindgen]` exports for browser integration.

// Modules shared between native and WASM targets:
pub mod metadata;
pub mod parser;
pub mod utils;

// Modules used only by the native LSP server:
#[cfg(not(target_arch = "wasm32"))]
pub mod commands;
pub mod completion;
#[cfg(not(target_arch = "wasm32"))]
pub mod definition;
#[cfg(not(target_arch = "wasm32"))]
pub mod depth;
#[cfg(not(target_arch = "wasm32"))]
pub mod diagnostics;
#[cfg(not(target_arch = "wasm32"))]
pub mod folding_range;
#[cfg(not(target_arch = "wasm32"))]
pub mod hover;
pub mod semantic;
#[cfg(not(target_arch = "wasm32"))]
pub mod server;
#[cfg(not(target_arch = "wasm32"))]
pub mod signature_help;

// WASM-specific API module:
#[cfg(target_arch = "wasm32")]
mod wasm_api;

#[cfg(target_arch = "wasm32")]
pub use wasm_api::*;
