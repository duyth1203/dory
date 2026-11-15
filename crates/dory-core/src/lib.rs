#![forbid(unsafe_code)]
//! Dory's hexagon. Domain types, ports, and product policy as pure functions.
//! See `docs/architecture/hexagonal.md`.

pub mod app;
pub mod capture;
pub mod domain;
pub mod models;
pub mod ports;
pub mod recorder;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use app::{Dory, Event, Options, Ports};
