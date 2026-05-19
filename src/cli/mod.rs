//! CLI command definitions and argument parsing.
//!
//! # Bounded Context: CLI Surface
//!
//! Owns argument definitions, subcommand dispatch, and flag
//! validation. Translates user input into typed values consumed
//! by `main.rs` — never performs I/O beyond printing help/errors.
//!
//! Uses clap derive macros for ergonomic argument definitions.

pub mod args;

use nitpik::license::{LicenseClaims, TokenKind};

/// License banner with ANSI styling for clap help output.
/// Bold "nitpik", dimmed rest. (Static — used for --help only.)
pub const LICENSE_BANNER_STYLED: &str = "\x1b[92m●\x1b[0m \x1b[1mnitpik\x1b[0m \x1b[2m· Free for personal & open-source use. Commercial use requires a license.\x1b[0m";

/// Print the license/thank-you banner to stderr.
///
/// When `claims` is `Some`, shows a thank-you message scoped to the
/// current plan and (for offline tokens) the upcoming expiry.
/// Otherwise shows the default "free for personal use" notice.
pub fn print_banner(claims: Option<&LicenseClaims>) {
    use colored::Colorize;
    use std::io::Write;
    let stderr = std::io::stderr();
    let mut handle = stderr.lock();
    let _ = writeln!(handle);
    match claims {
        Some(c) => {
            let suffix = match c.kind {
                TokenKind::Online => format!("· Licensed ({}). Thank you for supporting nitpik! ♥", c.plan),
                TokenKind::Offline => format!(
                    "· Licensed ({}, offline token). Thank you for supporting nitpik! ♥",
                    c.plan
                ),
            };
            let _ = writeln!(
                handle,
                "  {} {} {}",
                "●".bright_green(),
                "nitpik".bold(),
                suffix.dimmed(),
            );
        }
        None => {
            let _ = writeln!(
                handle,
                "  {} {} {}",
                "●".bright_green(),
                "nitpik".bold(),
                "· Free for personal & open-source use. Commercial use requires a license."
                    .dimmed(),
            );
        }
    }
    let _ = writeln!(handle);
    let _ = handle.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_online() -> LicenseClaims {
        LicenseClaims {
            user_id: "usr_test".into(),
            subscription_id: "sub_test".into(),
            plan: "monthly".into(),
            expires_at: 9_999_999_999,
            kind: TokenKind::Online,
            kid: "ed25519-2026-01".into(),
        }
    }

    #[test]
    fn print_banner_without_license() {
        print_banner(None);
    }

    #[test]
    fn print_banner_with_license() {
        print_banner(Some(&sample_online()));
    }

    #[test]
    fn print_banner_with_offline_token() {
        let mut c = sample_online();
        c.kind = TokenKind::Offline;
        print_banner(Some(&c));
    }

    #[test]
    fn license_banner_styled_is_non_empty() {
        assert!(LICENSE_BANNER_STYLED.contains("nitpik"));
    }
}
