use std::fmt::Write as _;

use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Osc8UrlDenialReason {
    Malformed,
    UnsafeCharacters,
    InvalidWebUrl,
    EmptyEmailAddress,
    LocalFile,
    UnsupportedScheme,
}

impl Osc8UrlDenialReason {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Malformed => "The target is not an absolute URL with a scheme.",
            Self::UnsafeCharacters => {
                "The target contains whitespace, control, or invisible characters."
            }
            Self::InvalidWebUrl => {
                "The web target must contain a valid host and no embedded credentials."
            }
            Self::EmptyEmailAddress => "The email target does not contain an address.",
            Self::LocalFile => "Opening local files from terminal links is blocked.",
            Self::UnsupportedScheme => "Opening this URL scheme from terminal links is blocked.",
        }
    }
}

/// A denied OSC 8 target: what to show the user, with unsafe characters made
/// visible, and why it was denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Osc8UrlDenial {
    pub(crate) display: String,
    pub(crate) reason: Osc8UrlDenialReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Osc8UrlDecision {
    /// Open, and display, exactly this serialized URL.
    Allow(String),
    Deny(Osc8UrlDenial),
}

/// Classifies a URL supplied by terminal-controlled OSC 8 output.
///
/// Raw character checks intentionally happen before URL parsing. The WHATWG
/// parser strips tabs, newlines, and surrounding whitespace and drops some
/// zero-width host characters, which would otherwise erase the evidence needed
/// to reject a deceptive target.
pub(crate) fn evaluate_osc8_url(raw: &str) -> Osc8UrlDecision {
    if raw.is_empty() {
        return deny(raw, Osc8UrlDenialReason::Malformed);
    }
    if raw.chars().any(is_unsafe_url_character) {
        return deny(raw, Osc8UrlDenialReason::UnsafeCharacters);
    }

    let Ok(url) = Url::parse(raw) else {
        return deny(raw, Osc8UrlDenialReason::Malformed);
    };

    match url.scheme() {
        scheme @ ("http" | "https") => {
            if !has_explicit_web_authority(raw, scheme)
                || !url.host_str().is_some_and(|host| !host.is_empty())
                || !url.username().is_empty()
                || url.password().is_some()
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
    Osc8UrlDecision::Deny(Osc8UrlDenial {
        display: escape_unsafe_characters(raw),
        reason,
    })
}

/// The WHATWG parser accepts `https:relative` (host `relative`) and
/// `https:///host`. Require the literal `scheme://` spelling followed by a host
/// character so the target means what it looks like.
fn has_explicit_web_authority(raw: &str, scheme: &str) -> bool {
    let bytes = raw.as_bytes();
    let authority_start = scheme.len() + "://".len();
    bytes
        .get(..scheme.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(scheme.as_bytes()))
        && bytes.get(scheme.len()..authority_start) == Some(b"://")
        && bytes
            .get(authority_start)
            .is_some_and(|first| !matches!(first, b'/' | b'\\'))
}

/// Renders unsafe characters as `\u{…}` so a denied target can be shown and
/// copied without carrying whitespace, control, or invisible characters.
fn escape_unsafe_characters(raw: &str) -> String {
    let mut display = String::with_capacity(raw.len());
    for character in raw.chars() {
        if is_unsafe_url_character(character) {
            write!(display, "\\u{{{:X}}}", character as u32)
                .expect("writing to a String cannot fail");
        } else {
            display.push(character);
        }
    }
    display
}

/// Whitespace (never valid in a URI), C0/C1 controls, and the bidirectional,
/// zero-width, and line-separator scalars that can reorder or conceal parts of
/// a displayed URL.
fn is_unsafe_url_character(character: char) -> bool {
    character.is_whitespace()
        || matches!(
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
    use super::{Osc8UrlDecision, Osc8UrlDenial, Osc8UrlDenialReason, evaluate_osc8_url};

    fn denied(display: &str, reason: Osc8UrlDenialReason) -> Osc8UrlDecision {
        Osc8UrlDecision::Deny(Osc8UrlDenial {
            display: display.to_string(),
            reason,
        })
    }

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
                evaluate_osc8_url(raw),
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
            (
                "https://trusted.example@evil.example/",
                Osc8UrlDenialReason::InvalidWebUrl,
            ),
            (
                "https://:secret@evil.example/",
                Osc8UrlDenialReason::InvalidWebUrl,
            ),
            ("mailto:", Osc8UrlDenialReason::EmptyEmailAddress),
            ("file:///tmp/report.txt", Osc8UrlDenialReason::LocalFile),
            (
                "vscode://file/tmp/report.txt",
                Osc8UrlDenialReason::UnsupportedScheme,
            ),
        ];

        for (raw, expected_reason) in cases {
            assert_eq!(
                evaluate_osc8_url(raw),
                denied(raw, expected_reason),
                "unexpected decision for {raw:?}"
            );
        }
    }

    #[test]
    fn rejects_and_escapes_whitespace_wherever_it_appears() {
        // The WHATWG parser would silently trim the outer cases and
        // percent-encode the inner one; a URI with raw whitespace is malformed.
        let cases = [
            (" https://example.com/", "\\u{20}https://example.com/"),
            ("https://example.com/ ", "https://example.com/\\u{20}"),
            ("https://example.com/a b", "https://example.com/a\\u{20}b"),
            (
                "\u{2003}mailto:user@example.com\u{A0}",
                "\\u{2003}mailto:user@example.com\\u{A0}",
            ),
        ];

        for (raw, expected_display) in cases {
            assert_eq!(
                evaluate_osc8_url(raw),
                denied(expected_display, Osc8UrlDenialReason::UnsafeCharacters),
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
                evaluate_osc8_url(&raw),
                denied(&expected_display, Osc8UrlDenialReason::UnsafeCharacters),
                "unexpected decision for {raw:?}"
            );
        }
    }
}
