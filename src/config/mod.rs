//! Configuration loading and layering.
//!
//! # Bounded Context: Configuration
//!
//! Owns TOML deserialization, env var resolution, and multi-layer
//! merge logic (CLI → env → repo → global → defaults). Produces
//! a fully-resolved [`Config`] consumed downstream — never touches
//! LLM providers or diff parsing.
//!
//! Handles `.nitpik.toml` loading, environment variable resolution,
//! and CLI flag merging with proper priority ordering.

pub mod loader;

pub use loader::{Config, ProviderConfig};
