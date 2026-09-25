//! Conservative screen evidence for Kimi 2.1.1's interactive TUI.
#[derive(Debug, PartialEq)]
pub(super) enum Readiness {
    Ready,
    TrustPending,
    Timeout,
}

pub(super) fn wait_kimi_ready(lines: &[String]) -> Readiness {
    if lines.iter().any(|line| {
        let lower = line.to_lowercase();
        lower.contains("trust this folder") || lower.contains("don't trust")
    }) {
        return Readiness::TrustPending;
    }
    if lines
        .iter()
        .any(|line| line.contains("Welcome to Kimi Code") || line.contains("Session:"))
        && input_text(lines).is_some_and(|text| text.is_empty())
    {
        Readiness::Ready
    } else {
        Readiness::Timeout
    }
}

fn content(line: &str) -> &str {
    line.trim().trim_matches('│').trim()
}

fn input_text(lines: &[String]) -> Option<String> {
    let index = lines.iter().rposition(|line| {
        let text = content(line);
        text == ">" || text == "❯" || text.starts_with("> ") || text.starts_with("❯ ")
    })?;
    let first = content(&lines[index]);
    let mut text = first.chars().skip(1).collect::<String>().trim().to_owned();
    // Kimi 2.1.1 wraps editor text inside the same box. Do not mistake
    // transcript/footer lines for a continuation of a bare fixture prompt.
    if lines[index].trim_start().starts_with('│') {
        for line in &lines[index + 1..] {
            if !line.trim_start().starts_with('│') {
                break;
            }
            text.push_str(content(line));
        }
    }
    Some(text)
}

fn compact(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// Retry only the submit key, once, when the exact instruction remains in the
/// editor. Re-pasting after an uncertain write could duplicate a user turn.
pub(super) fn kimi_instruction_pending(lines: &[String], instruction: &str) -> bool {
    !lines.iter().any(|line| {
        line.to_lowercase().contains("trust this folder")
            || line.to_lowercase().contains("don't trust")
    }) && input_text(lines).is_some_and(|text| {
        compact(&text)
            .trim_start_matches("[200~")
            .trim_end_matches("[201~")
            == compact(instruction)
    })
}

/// Empty input alone also describes the pre-write screen. Require a new,
/// visible instruction echo outside the input editor as well. Unknown layouts
/// deliberately fall back instead of treating PTY acceptance as submission.
pub(super) fn confirm_kimi_submit(before: &[String], after: &[String], instruction: &str) -> bool {
    let needle = compact(instruction);
    let previous = compact(
        &before
            .iter()
            .map(|line| content(line))
            .collect::<Vec<_>>()
            .join(""),
    );
    let visible = compact(
        &after
            .iter()
            .map(|line| content(line))
            .collect::<Vec<_>>()
            .join(""),
    );
    wait_kimi_ready(after) == Readiness::Ready
        && ((!previous.contains(&needle) && visible.contains(&needle))
            || (session(before).is_none() && session(after).is_some()))
        && !after.iter().any(|line| line.contains("Error: LLM not set"))
}

/// A new native session banner is independent submit evidence. Existing
/// sessions must instead expose a new transcript echo and empty editor.
fn session(lines: &[String]) -> Option<&str> {
    lines
        .iter()
        .filter_map(|line| content(line).strip_prefix("Session:"))
        .map(str::trim)
        .find(|id| id.starts_with("session_") || uuid::Uuid::parse_str(id).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn screen(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|s| (*s).to_owned()).collect()
    }
    #[test]
    fn welcome_needs_an_empty_editor() {
        assert_eq!(
            wait_kimi_ready(&screen(&["Welcome to Kimi Code", "> "])),
            Readiness::Ready
        );
        assert_eq!(
            wait_kimi_ready(&screen(&["Welcome to Kimi Code"])),
            Readiness::Timeout
        );
        assert_eq!(wait_kimi_ready(&screen(&[])), Readiness::Timeout);
        assert_eq!(
            wait_kimi_ready(&screen(&["Session: abc", "> draft"])),
            Readiness::Timeout
        );
    }
    #[test]
    fn trust_overrides_welcome_and_session() {
        assert_eq!(
            wait_kimi_ready(&screen(&[
                "Welcome to Kimi Code",
                "Session: abc",
                ">",
                "Trust this folder",
                "Don't trust"
            ])),
            Readiness::TrustPending
        );
    }
    #[test]
    fn submit_requires_new_echo_outside_editor() {
        let before = screen(&["Welcome to Kimi Code", ">"]);
        assert!(!confirm_kimi_submit(&before, &before, "Read context"));
        assert!(!confirm_kimi_submit(
            &before,
            &screen(&["Session: abc", "> Read context"]),
            "Read context"
        ));
        let after = screen(&["Session: abc", "Read context", ">"]);
        assert!(confirm_kimi_submit(&before, &after, "Read context"));
        assert!(!confirm_kimi_submit(&after, &after, "Read context"));
    }
    #[test]
    fn real_boxed_editor_and_wrapped_transcript() {
        let before = screen(&["│ Welcome to Kimi Code! │", "│ Session: │", "│ >   │"]);
        assert_eq!(wait_kimi_ready(&before), Readiness::Ready);
        let pending = screen(&["│ Session: │", "│ > Read con │", "│ text │"]);
        assert!(!confirm_kimi_submit(&before, &pending, "Read context"));
        let submitted = screen(&["│ Session: session_new │", "Read con", "text", "│ > │"]);
        assert!(confirm_kimi_submit(&before, &submitted, "Read context"));
        let existing = screen(&["│ Session: session_old │", "│ > │"]);
        let after = screen(&["│ Session: session_old │", "Read con", "text", "│ > │"]);
        assert!(confirm_kimi_submit(&existing, &after, "Read context"));
    }

    #[test]
    fn session_creation_requires_empty_editor_and_no_trust() {
        let before = screen(&["Welcome to Kimi Code", ">"]);
        let after = screen(&["Session: session_new", ">"]);
        assert!(confirm_kimi_submit(&before, &after, "Read context"));
        assert!(!confirm_kimi_submit(
            &before,
            &screen(&["Session: session_new", "> draft"]),
            "Read context"
        ));
        assert!(!confirm_kimi_submit(
            &before,
            &screen(&["Session: session_new", ">", "Trust this folder"]),
            "Read context"
        ));
    }
    #[test]
    fn retry_only_exact_pending_instruction_without_trust() {
        let pending = screen(&["│ Session: │", "│ > Read con │", "│ text │", "╰────╯"]);
        assert!(kimi_instruction_pending(&pending, "Read context"));
        assert!(!kimi_instruction_pending(&pending, "Read something else"));
        assert!(!kimi_instruction_pending(
            &screen(&[">", "Read context"]),
            "Read context"
        ));
        assert!(!kimi_instruction_pending(
            &screen(&["Trust this folder", "> Read context"]),
            "Read context"
        ));
        let failed = screen(&["Session: session_new", "Error: LLM not set", ">"]);
        assert!(!confirm_kimi_submit(
            &screen(&["Session:", ">"]),
            &failed,
            "Read context"
        ));
    }
    #[test]
    fn pending_tolerates_literal_paste_markers_but_not_other_drafts() {
        for text in [
            "> [200~Read context[201~",
            "> [200~Read context",
            "> Read context[201~",
        ] {
            assert!(kimi_instruction_pending(&screen(&[text]), "Read context"));
        }
        assert!(!kimi_instruction_pending(
            &screen(&["> extra [200~Read context[201~"]),
            "Read context"
        ));
    }
    #[test]
    fn isolated_kimi_211_pty_screens_never_claim_failed_turn_success() {
        let lines = |text: &str| text.lines().map(str::to_owned).collect::<Vec<_>>();
        let trust = lines(include_str!("fixtures/kimi-trust.txt"));
        let ready = lines(include_str!("fixtures/kimi-ready.txt"));
        let pending = lines(include_str!("fixtures/kimi-pending.txt"));
        let failed = lines(include_str!("fixtures/kimi-no-model.txt"));
        assert_eq!(wait_kimi_ready(&trust), Readiness::TrustPending);
        assert_eq!(wait_kimi_ready(&ready), Readiness::Ready);
        assert!(kimi_instruction_pending(
            &pending,
            "Say exactly: CON_PTY_SMOKE_782"
        ));
        assert!(!confirm_kimi_submit(
            &ready,
            &pending,
            "Say exactly: CON_PTY_SMOKE_782"
        ));
        assert!(!confirm_kimi_submit(
            &ready,
            &failed,
            "Say exactly: CON_PTY_SMOKE_782"
        ));
    }
}
