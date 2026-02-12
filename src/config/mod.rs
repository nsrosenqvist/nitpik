//! Configuration loading and layering.
//!
//! Handles `.nitpik.toml` loading, environment variable resolution,
//! and CLI flag merging with proper priority ordering.

pub mod loader;

pub use loader::{Config, ProviderConfig};
