//! Open the active HTML file in the default browser, independently of its file association.

use std::{io, path::Path};

use gpui::*;
use gpui_component::{
    ActiveTheme, Icon, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    notification::Notification,
};

use crate::{editor_syntax, editor_view::EditorView, pane_tree::PaneId, ui_scale::mono_icon_px};

#[cfg(any(target_os = "linux", all(test, unix)))]
#[path = "editor_browser_linux.rs"]
mod linux;

fn is_html(path: &Path) -> bool {
    editor_syntax::language_for_path(path) == Some("html")
}

fn file_url(path: &Path) -> io::Result<url::Url> {
    let absolute = std::path::absolute(path)?;
    url::Url::from_file_path(absolute)
        .map_err(|()| io::Error::new(io::ErrorKind::InvalidInput, "Invalid HTML file path"))
}

pub(crate) fn render_button(
    pane_id: PaneId,
    view: &Entity<EditorView>,
    cx: &App,
) -> Option<AnyElement> {
    if !view.read(cx).active_path().is_some_and(is_html) {
        return None;
    }
    let theme = cx.theme();
    let view = view.clone();
    Some(
        // Isolate pointer presses from the merged bar's pane drag handler.
        div()
            .id(("editor-browser-action", pane_id))
            .debug_selector(|| "editor-browser-action".into())
            .flex_shrink_0()
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(
                Button::new(("editor-open-browser", pane_id))
                    .accessibility_label("Open in browser")
                    .ghost()
                    .small()
                    .w(px(24.0))
                    .h(px(20.0))
                    .p_0()
                    .rounded(px(4.0))
                    .icon(
                        Icon::default()
                            .path("phosphor/arrow-square-out.svg")
                            .size(mono_icon_px(theme, 12.0))
                            .text_color(theme.muted_foreground),
                    )
                    .tooltip("Open in browser")
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let Some(path) = view.read(cx).active_path().filter(|p| is_html(p)) else {
                            return;
                        };
                        let path = path.to_path_buf();
                        let task = cx.background_executor().spawn(async move {
                            let url = file_url(&path)?;
                            #[cfg(target_os = "linux")]
                            return linux::open(&url);
                            #[cfg(not(target_os = "linux"))]
                            webbrowser::open(url.as_str())
                        });
                        window
                            .spawn(cx, async move |cx| {
                                if let Err(error) = task.await {
                                    let _ = cx.update(|window, cx| {
                                        window.push_notification(
                                            Notification::new()
                                                .title("Could not open browser")
                                                .message(error.to_string()),
                                            cx,
                                        );
                                    });
                                }
                            })
                            .detach();
                    }),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::{file_url, is_html};
    use std::path::Path;

    #[test]
    fn html_action_accepts_both_extensions_and_rejects_other_files() {
        for path in ["index.html", "page.htm", "INDEX.HTML", "page.HtM"] {
            assert!(is_html(Path::new(path)), "{path}");
        }
        for path in ["index.html.txt", "README.md", "image.svg", "html", ".html"] {
            assert!(!is_html(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn html_file_url_round_trips_reserved_characters_and_unicode() {
        let path = std::env::temp_dir().join("HTML 预览 #1?100%.html");
        let url = file_url(&path).unwrap();
        assert_eq!(url.scheme(), "file");
        assert_eq!(url.fragment(), None);
        assert_eq!(url.query(), None);
        assert_eq!(url.to_file_path().unwrap(), path);
    }

    #[test]
    fn relative_html_path_becomes_an_absolute_file_url() {
        let url = file_url(Path::new("examples/page.html")).unwrap();
        assert_eq!(
            url.to_file_path().unwrap(),
            std::env::current_dir().unwrap().join("examples/page.html")
        );
    }
}
