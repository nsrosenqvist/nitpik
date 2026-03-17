//! Centralized HTTP client construction.
//!
//! # Bounded Context: HTTP Infrastructure
//!
//! Owns `reqwest::Client` construction with consistent timeout,
//! user-agent, and TLS settings. Other modules call `http::client()`
//! rather than building their own clients.
//!
//! Ensures all outgoing HTTP requests share consistent timeout and
//! user-agent settings, and avoids bare `reqwest::Client::new()` calls
//! that can hang indefinitely on slow networks.

use std::time::Duration;

use crate::constants::{HTTP_CONNECT_TIMEOUT, HTTP_TIMEOUT, USER_AGENT};

/// Build a pre-configured HTTP client with sensible defaults.
///
/// All outgoing HTTP should go through this factory so that timeout,
/// user-agent, and proxy settings are centralized.
pub fn build_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
}

/// Build an HTTP client configured for Bitbucket Pipelines.
///
/// Inside Pipelines, requests must be routed through the local proxy.
pub fn build_bitbucket_pipelines_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(HTTP_CONNECT_TIMEOUT)
        .user_agent(USER_AGENT)
        .proxy(reqwest::Proxy::all("http://localhost:29418").expect("static proxy URL"))
        .build()
}
