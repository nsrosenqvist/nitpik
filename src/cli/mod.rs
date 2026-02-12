//! CLI command definitions and argument parsing.
//!
//! Uses clap derive macros for ergonomic argument definitions.

pub mod args;

use crate::license::LicenseClaims;

/// License banner with ANSI styling for clap help output.
/// Bold "nitpik", dimmed rest. (Static — used for --help only.)
pub const LICENSE_BANNER_STYLED: &str =
    "\x1b[1mnitpik\x1b[0m \x1b[2m· Free for personal & open-source use. Commercial use requires a license.\x1b[0m";

/// Print the license/thank-you banner to stderr.
///
/// When `claims` is `Some`, shows a personalised thank-you message.
/// Otherwise shows the default "free for personal use" notice.
pub fn print_banner(claims: Option<&LicenseClaims>) {
    use colored::Colorize;
    use std::io::Write;
    let stderr = std::io::stderr();
    let mut handle = stderr.lock();
    let _ = writeln!(handle);
    match claims {
        Some(c) => {
            let _ = writeln!(
                handle,
                "  {} {}",
                "nitpik".bold(),
                format!("· Licensed to {}. Thank you for supporting nitpik! ♥", c.customer_name)
                    .dimmed(),
            );
        }
        None => {
            let _ = writeln!(
                handle,
                "  {} {}",
                "nitpik".bold(),
                "· Free for personal & open-source use. Commercial use requires a license."
                    .dimmed(),
            );
        }
    }
    let _ = writeln!(handle);
    let _ = handle.flush();
}

