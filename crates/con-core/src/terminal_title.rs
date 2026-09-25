//! Source-driven OSC title presentation, independent of tab names and progress reports.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleIndicator {
    Activity(char),
    Attention(char),
}

impl TitleIndicator {
    pub fn frame(self) -> char {
        match self {
            Self::Activity(frame) | Self::Attention(frame) => frame,
        }
    }
}

/// Ephemeral per-surface state. Never serialize this into session restoration.
#[derive(Default, Debug)]
pub struct TerminalTitle {
    raw: Option<String>,
    name: Option<String>,
    indicator: Option<TitleIndicator>,
}

impl TerminalTitle {
    pub fn raw(&self) -> Option<&str> {
        self.raw.as_deref()
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    pub fn indicator(&self) -> Option<TitleIndicator> {
        self.indicator
    }

    /// None means no title change; otherwise the bool says whether naming
    /// context changed. Frame-only changes must not trigger AI summaries.
    pub fn update(&mut self, raw: Option<String>) -> Option<bool> {
        if self.raw == raw {
            return None;
        }
        let mut indicator = None;
        let mut frame_only = false;
        let name = raw.as_deref().map(|title| {
            if let Some((range, candidate)) = title_indicator(title) {
                let body = without_indicator(title, range);
                // Even familiar spinner glyphs can be ordinary text. Observe
                // motion before projecting them into the independent icon slot.
                frame_only = self.raw.as_deref().is_some_and(|previous| {
                    title_indicator(previous).is_some_and(|(range, old)| {
                        without_indicator(previous, range) == body
                            && matches!(
                                (old, candidate),
                                (TitleIndicator::Activity(_), TitleIndicator::Activity(_))
                            )
                            && (old != candidate || self.indicator.is_some())
                    })
                });
                if frame_only || matches!(candidate, TitleIndicator::Attention(_)) {
                    indicator = Some(candidate);
                    return body;
                }
            }
            title.to_owned()
        });
        let content_changed = self.name != name && !frame_only;
        self.raw = raw;
        self.name = name;
        self.indicator = indicator;
        Some(content_changed)
    }
}

pub(crate) fn without_indicator(title: &str, range: std::ops::Range<usize>) -> String {
    let before = &title[..range.start];
    let after = title[range.end..].trim_start();
    match (before.trim().is_empty(), after.is_empty()) {
        (true, _) => after.strip_prefix("| ").unwrap_or(after).to_owned(),
        (_, true) => before
            .trim_end()
            .strip_suffix(" |")
            .unwrap_or(before.trim_end())
            .to_owned(),
        _ => {
            let after = if before.trim_end().ends_with('|') {
                after.strip_prefix("| ").unwrap_or(after)
            } else {
                after
            };
            format!("{before}{after}")
        }
    }
}

pub(crate) fn title_indicator(title: &str) -> Option<(std::ops::Range<usize>, TitleIndicator)> {
    // Codex's explicit input-required title segment, not a generic punctuation rule.
    for (text, frame) in [
        ("[ ! ] Action Required", '!'),
        ("[ . ] Action Required", '.'),
    ] {
        if let Some(start) = title.find(text) {
            let end = start + text.len();
            if token_boundary(title, start, end) {
                return Some((start..end, TitleIndicator::Attention(frame)));
            }
        }
    }
    let mut candidate = None;
    for (start, frame) in title.char_indices() {
        if !('\u{2801}'..='\u{28ff}').contains(&frame) && !"·✢✳✶✻✽◐◑".contains(frame)
        {
            continue;
        }
        let end = start + frame.len_utf8();
        if token_boundary(title, start, end) {
            // Multiple candidate tokens are likely text, not one status slot.
            if candidate.is_some() {
                return None;
            }
            candidate = Some((start..end, TitleIndicator::Activity(frame)));
        }
    }
    candidate
}

fn token_boundary(title: &str, start: usize, end: usize) -> bool {
    title[..start]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
        && title[end..].chars().next().is_none_or(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_do_not_change_naming_context_and_clear_on_completion() {
        let mut title = TerminalTitle::default();
        assert_eq!(title.update(Some("修复 ⠋ con".into())), Some(true));
        assert_eq!(title.name(), Some("修复 ⠋ con"));
        assert_eq!(title.indicator(), None);
        assert_eq!(title.update(Some("修复 ⠙ con".into())), Some(false));
        assert_eq!(title.name(), Some("修复 con"));
        assert_eq!(title.indicator(), Some(TitleIndicator::Activity('⠙')));
        assert_eq!(title.raw(), Some("修复 ⠙ con"));
        assert_eq!(title.update(Some("修复 ⠙ con".into())), None);
        assert_eq!(title.update(Some("修复 con".into())), Some(false));
        assert_eq!(title.indicator(), None);
        assert_eq!(title.update(None), Some(true));
        assert_eq!(title.name(), None);
    }

    #[test]
    fn action_required_is_not_running_and_preserves_the_source_frame() {
        let mut title = TerminalTitle::default();
        title.update(Some("[ ! ] Action Required | con".into()));
        assert_eq!(title.name(), Some("con"));
        assert_eq!(title.indicator(), Some(TitleIndicator::Attention('!')));
        assert_eq!(
            title.update(Some("[ . ] Action Required | con".into())),
            Some(false)
        );
        assert_eq!(title.indicator(), Some(TitleIndicator::Attention('.')));
        title.update(Some("⠋ con".into()));
        assert_eq!(title.indicator(), None);
    }

    #[test]
    fn unknown_braille_requires_motion_and_does_not_cross_title_bodies() {
        let mut title = TerminalTitle::default();
        title.update(Some("⠂ amp".into()));
        assert_eq!(title.indicator(), None);
        assert_eq!(title.update(Some("⠒ amp".into())), Some(false));
        assert_eq!(title.indicator(), Some(TitleIndicator::Activity('⠒')));
        assert_eq!(title.update(Some("⠲ amp".into())), Some(false));
        title.update(Some("⠂ unrelated".into()));
        assert_eq!(title.indicator(), None);
    }

    #[test]
    fn decorative_spinner_requires_motion_and_stops_when_removed() {
        let mut title = TerminalTitle::default();
        title.update(Some("✻ Task".into()));
        assert_eq!(title.indicator(), None);
        assert_eq!(title.update(Some("✽ Task".into())), Some(false));
        assert_eq!(title.indicator(), Some(TitleIndicator::Activity('✽')));
        assert_eq!(title.update(Some("✶ Task".into())), Some(false));
        assert_eq!(title.update(Some("Task".into())), Some(false));
        assert_eq!(title.indicator(), None);
    }

    #[test]
    fn many_frames_trigger_one_naming_change() {
        let mut title = TerminalTitle::default();
        let mut naming_changes = 0;
        for (index, frame) in "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".chars().cycle().take(10_000).enumerate()
        {
            naming_changes +=
                usize::from(title.update(Some(format!("{frame} Task"))) == Some(true));
            assert_eq!(
                title.indicator(),
                (index > 0).then_some(TitleIndicator::Activity(frame))
            );
        }
        assert_eq!(naming_changes, 1);
    }

    #[test]
    fn ordinary_unicode_and_ambiguous_tokens_remain_verbatim() {
        for raw in [
            "⠋ notes",
            "文件⠋名",
            "⠋⠙ braille",
            "⠋ ⠙ text",
            "✻ notes",
            "[ ! ] Action Requiredness",
            "",
        ] {
            let mut title = TerminalTitle::default();
            title.update(Some(raw.into()));
            assert_eq!(title.name(), Some(raw));
            assert_eq!(title.indicator(), None, "{raw}");
        }
    }

    #[test]
    fn removing_status_preserves_body_spacing_and_delimiters() {
        let mut title = TerminalTitle::default();
        title.update(Some("foo  ⠋  bar".into()));
        title.update(Some("foo  ⠙  bar".into()));
        assert_eq!(title.name(), Some("foo  bar"));
        assert_eq!(title.update(Some("foo  bar".into())), Some(false));
        for (raw, name) in [
            ("con | [ ! ] Action Required", "con"),
            ("a | [ ! ] Action Required | b", "a | b"),
            ("[ . ] Action Required", ""),
        ] {
            title.update(Some(raw.into()));
            assert_eq!(title.name(), Some(name));
        }
    }
}
