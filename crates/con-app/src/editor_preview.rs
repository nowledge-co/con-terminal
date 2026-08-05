//! Scrollable markdown preview body for the built-in editor pane.
//!
//! Whole-document rendering on top of the chat markdown renderer; local
//! images resolve against the markdown file's directory. Block-level
//! virtualization is intentionally left out — adopt the agent panel's
//! per-block `ChatMarkdownBlockView` pattern if large documents demand it.

use std::path::Path;

use gpui::{
    AnyElement, InteractiveElement, IntoElement, ParentElement, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, div, px,
};
use gpui_component::Theme;

use crate::chat_markdown::{ParsedChatMarkdown, render_parsed_chat_markdown_file_preview};

/// Render the parsed markdown document inside a vertical scroll container
/// tracked by `scroll_handle`.
pub(crate) fn render_markdown_preview(
    document: &ParsedChatMarkdown,
    base_dir: &Path,
    theme: &Theme,
    scroll_handle: &ScrollHandle,
    copy_namespace: &str,
) -> AnyElement {
    if document.block_count() == 0 {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.muted_foreground.opacity(0.6))
            .child("Nothing to preview")
            .into_any_element();
    }

    div()
        // The id is namespaced per editor view + tab: multiple editor panes
        // can show previews in the same window, and element ids must be
        // unique or scroll state cross-wires.
        .id(SharedString::from(format!(
            "editor-markdown-preview-{copy_namespace}"
        )))
        .size_full()
        .overflow_y_scroll()
        .track_scroll(scroll_handle)
        .child(div().w_full().px(px(20.0)).py(px(16.0)).child(
            render_parsed_chat_markdown_file_preview(
                document,
                base_dir,
                theme,
                copy_namespace.to_string(),
            ),
        ))
        .into_any_element()
}
