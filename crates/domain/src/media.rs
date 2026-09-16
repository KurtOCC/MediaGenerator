//! Media types and prompt validation.
//!
//! Both are needed by the web layer, the storage layer and the providers, so
//! they live here rather than in any one of them.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{error::DomainError, i18n::nb};

/// What kind of media a job produces.
///
/// The serialised spellings match the `media_type` enum in the database and the
/// values submitted by the form, so they must not be renamed casually.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    /// Still images and illustrations.
    Image,
    /// Speech, music and sound effects.
    Audio,
    /// Short video clips.
    Video,
}

impl MediaType {
    /// Returns the stable string form used in storage, forms and APIs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
        }
    }

    /// Returns the Norwegian label shown to the user.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Image => nb::MEDIA_IMAGE,
            Self::Audio => nb::MEDIA_AUDIO,
            Self::Video => nb::MEDIA_VIDEO,
        }
    }

    /// Every variant, in the order the user interface shows them.
    pub const ALL: [Self; 3] = [Self::Image, Self::Audio, Self::Video];
}

impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for MediaType {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "image" => Ok(Self::Image),
            "audio" => Ok(Self::Audio),
            "video" => Ok(Self::Video),
            _ => Err(DomainError::Validation(
                nb::ERR_UNKNOWN_MEDIA_TYPE.to_owned(),
            )),
        }
    }
}

/// Validates and normalises a prompt.
///
/// Returns the cleaned prompt: outer whitespace trimmed and `\r\n` folded to
/// `\n`, so the same text typed on Windows and on macOS is stored identically.
///
/// Tabs and newlines are kept — a prompt may reasonably be several lines — but
/// every other control character is rejected. Those cannot be typed by
/// accident, and they are exactly what a log-injection or terminal-escape
/// attempt looks like.
///
/// The limit counts characters, not bytes, because that is what the counter in
/// the interface shows the user.
///
/// # Errors
///
/// Returns [`DomainError::Validation`] with a Norwegian message when the prompt
/// is empty, too long, or contains disallowed control characters.
pub fn validate_prompt(raw: &str, max_chars: usize) -> Result<String, DomainError> {
    let normalised = raw.replace("\r\n", "\n");
    let trimmed = normalised.trim();

    if trimmed.is_empty() {
        return Err(DomainError::Validation(nb::ERR_PROMPT_EMPTY.to_owned()));
    }

    let length = trimmed.chars().count();
    if length > max_chars {
        return Err(DomainError::Validation(nb::err_prompt_too_long(max_chars)));
    }

    if trimmed
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err(DomainError::Validation(
            nb::ERR_PROMPT_CONTROL_CHARS.to_owned(),
        ));
    }

    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_prompt_is_accepted_and_trimmed() {
        assert_eq!(
            validate_prompt("  Solnedgang over Oslofjorden  ", 4000).expect("should be accepted"),
            "Solnedgang over Oslofjorden"
        );
    }

    #[test]
    fn windows_line_endings_are_folded() {
        assert_eq!(
            validate_prompt("linje ett\r\nlinje to", 4000).expect("should be accepted"),
            "linje ett\nlinje to"
        );
    }

    #[test]
    fn an_empty_or_blank_prompt_is_rejected() {
        for blank in ["", "   ", "\n\n", "\t"] {
            assert!(validate_prompt(blank, 4000).is_err(), "{blank:?}");
        }
    }

    #[test]
    fn the_limit_counts_characters_not_bytes() {
        // Each of these is two bytes but one character.
        let prompt = "æ".repeat(10);
        assert!(validate_prompt(&prompt, 10).is_ok());
        assert!(validate_prompt(&prompt, 9).is_err());
    }

    #[test]
    fn control_characters_are_rejected_but_tabs_and_newlines_are_not() {
        assert!(validate_prompt("to\nlinjer", 4000).is_ok());
        assert!(validate_prompt("med\ttabulator", 4000).is_ok());
        assert!(validate_prompt("null\u{0}byte", 4000).is_err());
        assert!(validate_prompt("escape\u{1b}[31m", 4000).is_err());
    }

    #[test]
    fn media_types_round_trip_through_their_string_form() {
        for media_type in MediaType::ALL {
            let parsed: MediaType = media_type.as_str().parse().expect("should parse");
            assert_eq!(parsed, media_type);
        }
        assert!("bilde".parse::<MediaType>().is_err());
    }
}
