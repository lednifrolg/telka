//! What telka prints. Colors use the terminal's ANSI palette, so they follow the terminal theme,
//! and disappear when the output is not a terminal or `NO_COLOR` is set.

use std::error::Error as _;
use std::fmt::Display;
use std::io::Write;

use anstream::{eprintln, println};
use anstyle::{AnsiColor, Style};
use serde::Serialize;

use crate::error::AppError;

pub const DIM: Style = Style::new().dimmed();
pub const BOLD: Style = Style::new().bold();
pub const GREEN: Style = AnsiColor::Green.on_default();
pub const YELLOW: Style = AnsiColor::Yellow.on_default();
pub const RED: Style = AnsiColor::Red.on_default();
pub const BLUE: Style = AnsiColor::Blue.on_default();

pub fn paint(style: Style, text: impl Display) -> String {
    format!("{style}{text}{style:#}")
}

/// The error with the causes that add something, in one line, without the operating system's
/// error numbers: "could not connect to the TV: connection refused".
pub fn message(error: &AppError) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(inner) = cause {
        let text = inner.to_string();
        if !message.contains(&text) {
            message = format!("{message}: {text}");
        }
        cause = inner.source();
    }
    match message.split_once(" (os error") {
        Some((before, after)) => {
            let rest = after.split_once(')').map_or("", |(_, rest)| rest);
            format!("{before}{rest}")
        }
        None => message,
    }
}

pub fn error(error: &AppError) {
    eprintln!("{} {}", paint(RED.bold(), "error:"), message(error));
}

pub fn note(text: impl Display) {
    eprintln!("{}", paint(DIM, text));
}

pub fn warn(text: impl Display) {
    eprintln!("{} {text}", paint(YELLOW.bold(), "warning:"));
}

pub fn success(text: impl Display) {
    println!("{} {text}", paint(GREEN.bold(), "✓"));
}

pub fn line(text: impl Display) {
    println!("{text}");
}

/// Writes `bytes` to standard output as they are, such as a completion script.
pub fn raw(bytes: &[u8]) -> Result<(), AppError> {
    std::io::stdout()
        .lock()
        .write_all(bytes)
        .map_err(|e| AppError::io("standard output", e))
}

/// Prints `value` as JSON. A reader that went away, like `head`, is an error rather than a
/// panic.
pub fn json(value: &impl Serialize) -> Result<(), AppError> {
    let text = serde_json::to_string_pretty(value).expect("telka's views always serialize");
    writeln!(std::io::stdout().lock(), "{text}").map_err(|e| AppError::io("standard output", e))
}

/// `text` padded to `width` characters, counting characters rather than bytes.
pub fn pad(text: &str, width: usize) -> String {
    let length = text.chars().count();
    format!("{text}{}", " ".repeat(width.saturating_sub(length)))
}

/// A bar of `width` characters, filled for `value` out of `max`.
pub fn bar(value: u8, max: u8, width: usize) -> String {
    let filled = (usize::from(value) * width)
        .checked_div(usize::from(max.max(1)))
        .unwrap_or(0)
        .min(width);
    format!(
        "{}{}",
        paint(BLUE, "━".repeat(filled)),
        paint(DIM, "─".repeat(width - filled))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_counts_characters() {
        assert_eq!(pad("Kuchyňa", 9), "Kuchyňa  ");
        assert_eq!(pad("too long", 3), "too long");
    }

    #[test]
    fn os_error_numbers_are_left_out() {
        let error = AppError::Unreachable(
            "could not connect to the TV: Connection refused (os error 111)".into(),
        );
        assert_eq!(
            message(&error),
            "could not connect to the TV: Connection refused"
        );
    }
}
