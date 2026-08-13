//! Terminal styles shared by the commands.
//!
//! One place for the verdict styling, because every command ends with a
//! coloured label and they would otherwise each carry their own `Style` --
//! which is how two of them end up disagreeing on what green means.

use owo_colors::{OwoColorize, Stream::Stdout, Style};

const SUCCESS: Style = Style::new().green().bold();
const FAILURE: Style = Style::new().red().bold();

/// A verdict label, coloured when the terminal supports it.
///
/// Returns a `String` rather than printing: the callers each follow the label
/// with different detail, and formatting that here would mean one function per
/// command.
#[must_use]
pub fn ok(label: &str) -> String {
    label
        .if_supports_color(Stdout, |text| SUCCESS.style(text))
        .to_string()
}

#[must_use]
pub fn failed(label: &str) -> String {
    label
        .if_supports_color(Stdout, |text| FAILURE.style(text))
        .to_string()
}
