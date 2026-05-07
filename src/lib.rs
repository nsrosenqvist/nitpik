//! nitpik — AI-powered code review CLI (library crate).
//!
//! The lib is treated as semi-private: items the binary (`main.rs`)
//! and integration tests touch are `pub`; everything else is
//! `pub(crate)` so the next major can reshuffle internals without
//! breaking external consumers.

pub mod agents;
pub mod audit;
pub mod cache;
pub(crate) mod ci;
pub mod config;
pub mod constants;
pub mod context;
pub mod diff;
pub mod env;
pub(crate) mod http;
pub mod license;
pub mod models;
pub mod orchestrator;
pub mod output;
pub mod progress;
pub mod providers;
pub mod security;
pub mod telemetry;
pub mod threat;
pub(crate) mod tools;
pub mod update;
