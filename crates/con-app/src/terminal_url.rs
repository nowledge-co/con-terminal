use std::fmt::Write as _;

use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Osc8UrlDenialReason {
    Malformed,
    InvalidEncoding,
    UnsafeCharacters,
    SurroundingWhitespace,
    InvalidWebUrl,
    EmptyEmailAddress,
    LocalFile,
    UnsupportedScheme,
}

impl Osc8UrlDenialReason {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Malformed => "The target is not an absolute URL with a scheme.",
            Self::InvalidEncoding => "The target is not valid UTF-8.",
            Self::UnsafeCharacters => "The target contains invisible or line-breaking characters.",
            Self::SurroundingWhitespace => "The target contains leading or trailing whitespace.",
            Self::InvalidWebUrl => {
                "The web target must use http:// or https:// and contain a valid host."
            }
            Self::EmptyEmailAddress => "The email target does not contain an address.",
            Self::LocalFile => "Opening local files from terminal links is blocked.",
            Self::UnsupportedScheme => "Opening this URL scheme from terminal links is blocked.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Osc8UrlDecision {
    Allow(String),
    Deny {
        display: String,
        reason: Osc8UrlDenialReason,
    },
}

/// Classifies a URL supplied by terminal-controlled OSC 8 output.
///
/// Raw Unicode checks intentionally happen before URL parsing. The WHATWG
/// parser removes tabs, newlines, and some zero-width host characters, which
/// would otherwise erase the evidence needed to reject a deceptive target.
pub(crate) fn evaluate_osc8_url(raw: &[u8]) -> Osc8UrlDecision {
    let Ok(raw) = std::str::from_utf8(raw) else {
        return Osc8UrlDecision::Deny {
            display: escape_unsafe_url_bytes(raw),
            reason: Osc8UrlDenialReason::InvalidEncoding,
        };
    };
    if raw.is_empty() {
        return deny(raw, Osc8UrlDenialReason::Malformed);
    }
    if raw.chars().any(is_unsafe_url_character) {
        return deny(raw, Osc8UrlDenialReason::UnsafeCharacters);
    }
    if raw.trim() != raw {
        return Osc8UrlDecision::Deny {
            display: escape_surrounding_whitespace(raw),
            reason: Osc8UrlDenialReason::SurroundingWhitespace,
        };
    }

    let Ok(url) = Url::parse(raw) else {
        return deny(raw, Osc8UrlDenialReason::Malformed);
    };

    match url.scheme() {
        "http" => {
            if !has_explicit_web_authority(raw, "http://")
                || !url.host_str().is_some_and(|host| !host.is_empty())
            {
                return deny(raw, Osc8UrlDenialReason::InvalidWebUrl);
            }
            Osc8UrlDecision::Allow(url.into())
        }
        "https" => {
            if !has_explicit_web_authority(raw, "https://")
                || !url.host_str().is_some_and(|host| !host.is_empty())
            {
                return deny(raw, Osc8UrlDenialReason::InvalidWebUrl);
            }
            Osc8UrlDecision::Allow(url.into())
        }
        "mailto" => {
            if url.path().is_empty() {
                return deny(raw, Osc8UrlDenialReason::EmptyEmailAddress);
            }
            Osc8UrlDecision::Allow(url.into())
        }
        "file" => deny(raw, Osc8UrlDenialReason::LocalFile),
        _ => deny(raw, Osc8UrlDenialReason::UnsupportedScheme),
    }
}

fn deny(raw: &str, reason: Osc8UrlDenialReason) -> Osc8UrlDecision {
    Osc8UrlDecision::Deny {
        display: escape_unsafe_url_characters(raw),
        reason,
    }
}

fn has_explicit_web_authority(raw: &str, prefix: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix.as_bytes()))
        && bytes
            .get(prefix.len())
            .is_some_and(|first| !matches!(first, b'/' | b'\\'))
}

fn escape_unsafe_url_characters(raw: &str) -> String {
    let mut display = String::with_capacity(raw.len());
    append_escaped_url_characters(&mut display, raw);
    display
}

fn escape_surrounding_whitespace(raw: &str) -> String {
    let leading_end = raw.len() - raw.trim_start().len();
    let trailing_start = raw.trim_end().len();
    let mut display = String::with_capacity(raw.len());
    for (offset, character) in raw.char_indices() {
        if offset < leading_end || offset >= trailing_start {
            write!(display, "\\u{{{:X}}}", character as u32)
                .expect("writing to a String cannot fail");
        } else {
            display.push(character);
        }
    }
    display
}

fn escape_unsafe_url_bytes(mut raw: &[u8]) -> String {
    let mut display = String::with_capacity(raw.len());
    while !raw.is_empty() {
        match std::str::from_utf8(raw) {
            Ok(valid) => {
                append_escaped_url_characters(&mut display, valid);
                break;
            }
            Err(error) => {
                let (valid, invalid) = raw.split_at(error.valid_up_to());
                append_escaped_url_characters(
                    &mut display,
                    std::str::from_utf8(valid).expect("prefix before UTF-8 error must be valid"),
                );
                let invalid_len = error.error_len().unwrap_or(invalid.len());
                for byte in &invalid[..invalid_len] {
                    write!(display, "\\x{{{byte:02X}}}").expect("writing to a String cannot fail");
                }
                raw = &invalid[invalid_len..];
            }
        }
    }
    display
}

fn append_escaped_url_characters(display: &mut String, raw: &str) {
    for character in raw.chars() {
        if is_unsafe_url_character(character) {
            write!(display, "\\u{{{:X}}}", character as u32)
                .expect("writing to a String cannot fail");
        } else {
            display.push(character);
        }
    }
}

fn is_unsafe_url_character(character: char) -> bool {
    matches!(
        character as u32,
        0x00..=0x1F
            | 0x7F..=0x9F
            | 0x061C
            | 0x200B..=0x200F
            | 0x2028..=0x2029
            | 0x202A..=0x202E
            | 0x2060
            | 0x2066..=0x2069
            | 0xFEFF
    )
}

#[cfg(test)]
mod tests {
    use super::{Osc8UrlDecision, Osc8UrlDenialReason, evaluate_osc8_url};

    #[test]
    fn allows_supported_absolute_urls_using_their_serialized_target() {
        let cases = [
            ("https://example.com/path", "https://example.com/path"),
            ("HTTP://EXAMPLE.COM", "http://example.com/"),
            ("https://exаmple.com/x", "https://xn--exmple-4nf.com/x"),
            ("mailto:user@example.com", "mailto:user@example.com"),
        ];

        for (raw, expected) in cases {
            assert_eq!(
                evaluate_osc8_url(raw.as_bytes()),
                Osc8UrlDecision::Allow(expected.to_string()),
                "unexpected decision for {raw:?}"
            );
        }
    }

    #[test]
    fn rejects_unsupported_or_ambiguous_targets() {
        let cases = [
            ("", Osc8UrlDenialReason::Malformed),
            ("example.com", Osc8UrlDenialReason::Malformed),
            ("https:relative", Osc8UrlDenialReason::InvalidWebUrl),
            (
                "https:relative?next=a://b",
                Osc8UrlDenialReason::InvalidWebUrl,
            ),
            ("https:///path", Osc8UrlDenialReason::InvalidWebUrl),
            ("https://\\example.com", Osc8UrlDenialReason::InvalidWebUrl),
            ("mailto:", Osc8UrlDenialReason::EmptyEmailAddress),
            ("file:///tmp/report.txt", Osc8UrlDenialReason::LocalFile),
            (
                "vscode://file/tmp/report.txt",
                Osc8UrlDenialReason::UnsupportedScheme,
            ),
        ];

        for (raw, expected_reason) in cases {
            assert_eq!(
                evaluate_osc8_url(raw.as_bytes()),
                Osc8UrlDecision::Deny {
                    display: raw.to_string(),
                    reason: expected_reason,
                },
                "unexpected decision for {raw:?}"
            );
        }
    }

    #[test]
    fn rejects_and_escapes_surrounding_whitespace() {
        let cases = [
            (" https://example.com/", "\\u{20}https://example.com/"),
            ("https://example.com/ ", "https://example.com/\\u{20}"),
            (
                "\u{2003}mailto:user@example.com\u{A0}",
                "\\u{2003}mailto:user@example.com\\u{A0}",
            ),
        ];

        for (raw, expected_display) in cases {
            assert_eq!(
                evaluate_osc8_url(raw.as_bytes()),
                Osc8UrlDecision::Deny {
                    display: expected_display.to_string(),
                    reason: Osc8UrlDenialReason::SurroundingWhitespace,
                },
                "unexpected decision for {raw:?}"
            );
        }
    }

    #[test]
    fn rejects_and_escapes_unsafe_characters_before_parsing() {
        let unsafe_scalars = [
            0x00, 0x09, 0x1F, 0x7F, 0x85, 0x9F, 0x061C, 0x200B, 0x200F, 0x2028, 0x2029, 0x202A,
            0x202E, 0x2060, 0x2066, 0x2069, 0xFEFF,
        ];

        for scalar in unsafe_scalars {
            let character = char::from_u32(scalar).expect("test scalar must be valid Unicode");
            let raw = format!("https://example.com/a{character}b");
            let expected_display = format!("https://example.com/a\\u{{{scalar:X}}}b");
            assert_eq!(
                evaluate_osc8_url(raw.as_bytes()),
                Osc8UrlDecision::Deny {
                    display: expected_display,
                    reason: Osc8UrlDenialReason::UnsafeCharacters,
                },
                "unexpected decision for {raw:?}"
            );
        }
    }

    #[test]
    fn rejects_and_escapes_invalid_utf8_before_parsing() {
        let raw = b"https://example.com/a\xF0\x28\x8C\x28b\xFF";

        assert_eq!(
            evaluate_osc8_url(raw),
            Osc8UrlDecision::Deny {
                display: "https://example.com/a\\x{F0}(\\x{8C}(b\\x{FF}".to_string(),
                reason: Osc8UrlDenialReason::InvalidEncoding,
            }
        );
    }
}
