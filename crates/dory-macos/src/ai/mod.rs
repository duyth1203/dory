//! Apple Intelligence through `swift/DoryAI.swift`. `ffi` holds every `unsafe` line of the bridge,
//! and the modules beside it are safe Rust over it.

mod ffi;
pub mod foundation_models;
