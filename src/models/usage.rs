//! Token usage accounting shared across provider boundaries.
//!
//! `TokenUsage` mirrors the field set of `rig_core::completion::Usage`
//! (input, output, cached input, cache creation) but lives in the
//! domain model so the orchestrator, output renderers, and progress
//! display do not depend on rig-core types.

use std::ops::{Add, AddAssign};

use serde::{Deserialize, Serialize};

/// Aggregated token usage for one or more LLM calls.
///
/// Fields mirror `rig_core::completion::Usage`. Values are additive:
/// summing two `TokenUsage`s yields the combined usage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Input (prompt) tokens billed by the provider, including any
    /// tokens that hit the cache.
    pub input: u64,
    /// Output (completion) tokens.
    pub output: u64,
    /// Subset of `input` that was served from the provider's prompt
    /// cache. Always `<= input`.
    pub cached_input: u64,
    /// Tokens written to the provider's prompt cache during this call
    /// (Anthropic-style `cache_creation_input_tokens`).
    pub cache_creation: u64,
}

impl TokenUsage {
    /// Construct an empty usage record.
    pub const fn new() -> Self {
        Self {
            input: 0,
            output: 0,
            cached_input: 0,
            cache_creation: 0,
        }
    }

    /// Total tokens (input + output). Useful for display when the
    /// caller does not need the cache breakdown.
    pub const fn total(&self) -> u64 {
        self.input + self.output
    }

    /// Cache hit ratio over input tokens, in `[0.0, 1.0]`.
    /// Returns `0.0` when no input tokens have been counted.
    pub fn cache_hit_ratio(&self) -> f64 {
        if self.input == 0 {
            0.0
        } else {
            self.cached_input as f64 / self.input as f64
        }
    }

    /// Format the usage as a one-line summary suitable for the
    /// terminal token report. The output omits cache fields when zero
    /// so non-caching providers produce a concise line.
    ///
    /// Sample outputs:
    /// * `Tokens: 1.2K↑ in, 340↓ out`
    /// * `Tokens: 12.0K↑ in, 1.5K↓ out (8.0K cached, 67% hit)`
    pub fn format_summary(&self) -> String {
        let mut line = format!(
            "Tokens: {}↑ in, {}↓ out",
            format_count(self.input),
            format_count(self.output),
        );
        if self.cached_input > 0 {
            line.push_str(&format!(
                " ({} cached, {:.0}% hit)",
                format_count(self.cached_input),
                self.cache_hit_ratio() * 100.0,
            ));
        }
        if self.cache_creation > 0 {
            line.push_str(&format!(
                " (+{} cache write)",
                format_count(self.cache_creation)
            ));
        }
        line
    }
}

/// Render a token count compactly: `1234` → `1.2K`, `1234567` → `1.2M`.
pub(crate) fn format_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

impl Add for TokenUsage {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self {
            input: self.input + rhs.input,
            output: self.output + rhs.output,
            cached_input: self.cached_input + rhs.cached_input,
            cache_creation: self.cache_creation + rhs.cache_creation,
        }
    }
}

impl AddAssign for TokenUsage {
    fn add_assign(&mut self, rhs: Self) {
        self.input += rhs.input;
        self.output += rhs.output;
        self.cached_input += rhs.cached_input;
        self.cache_creation += rhs.cache_creation;
    }
}

impl From<rig::completion::Usage> for TokenUsage {
    fn from(u: rig::completion::Usage) -> Self {
        Self {
            input: u.input_tokens,
            output: u.output_tokens,
            cached_input: u.cached_input_tokens,
            cache_creation: u.cache_creation_input_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_add_assign_sum_fields() {
        let a = TokenUsage {
            input: 10,
            output: 5,
            cached_input: 3,
            cache_creation: 1,
        };
        let b = TokenUsage {
            input: 20,
            output: 7,
            cached_input: 4,
            cache_creation: 2,
        };
        let sum = a + b;
        assert_eq!(sum.input, 30);
        assert_eq!(sum.output, 12);
        assert_eq!(sum.cached_input, 7);
        assert_eq!(sum.cache_creation, 3);

        let mut c = a;
        c += b;
        assert_eq!(c, sum);
    }

    #[test]
    fn total_combines_input_and_output() {
        let u = TokenUsage {
            input: 100,
            output: 50,
            cached_input: 0,
            cache_creation: 0,
        };
        assert_eq!(u.total(), 150);
    }

    #[test]
    fn cache_hit_ratio_handles_zero_input() {
        let u = TokenUsage::new();
        assert_eq!(u.cache_hit_ratio(), 0.0);
    }

    #[test]
    fn cache_hit_ratio_computes_fraction() {
        let u = TokenUsage {
            input: 1000,
            output: 0,
            cached_input: 250,
            cache_creation: 0,
        };
        assert!((u.cache_hit_ratio() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn from_rig_usage_maps_all_fields() {
        let mut r = rig::completion::Usage::new();
        r.input_tokens = 11;
        r.output_tokens = 22;
        r.total_tokens = 33;
        r.cached_input_tokens = 5;
        r.cache_creation_input_tokens = 2;
        let t: TokenUsage = r.into();
        assert_eq!(
            t,
            TokenUsage {
                input: 11,
                output: 22,
                cached_input: 5,
                cache_creation: 2,
            }
        );
    }

    #[test]
    fn default_is_zero() {
        let u = TokenUsage::default();
        assert_eq!(u, TokenUsage::new());
        assert_eq!(u.total(), 0);
    }

    #[test]
    fn format_summary_basic_no_cache() {
        let u = TokenUsage {
            input: 1234,
            output: 56,
            cached_input: 0,
            cache_creation: 0,
        };
        assert_eq!(u.format_summary(), "Tokens: 1.2K↑ in, 56↓ out");
    }

    #[test]
    fn format_summary_includes_cache_hit_ratio() {
        let u = TokenUsage {
            input: 12_000,
            output: 1_500,
            cached_input: 8_000,
            cache_creation: 0,
        };
        let s = u.format_summary();
        assert!(s.contains("Tokens: 12.0K↑ in, 1.5K↓ out"));
        assert!(s.contains("8.0K cached"));
        assert!(s.contains("67% hit"));
    }

    #[test]
    fn format_summary_includes_cache_creation_when_present() {
        let u = TokenUsage {
            input: 1000,
            output: 100,
            cached_input: 0,
            cache_creation: 500,
        };
        let s = u.format_summary();
        assert!(s.contains("+500 cache write"));
    }

    #[test]
    fn format_count_handles_thresholds() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1.0K");
        assert_eq!(format_count(2_500_000), "2.5M");
    }
}
