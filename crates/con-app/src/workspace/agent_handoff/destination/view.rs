use super::*;
use gpui_component::{
    ActiveTheme, Disableable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
};

mod cards;
mod fallback;
mod fit;
mod select;

const HEADER_HEIGHT: Pixels = px(44.0);
/// Height of one destination row (card_row py(12) + text_sm line); caps the
/// scrollable session-candidate and model-candidate lists.
pub(super) const ROW_HEIGHT: Pixels = px(44.0);
/// Page size for progressive disclosure of session candidates; the revealed
/// list itself scrolls, and a "show more" row extends it in steps.
pub(super) const CANDIDATE_PAGE: usize = 50;

struct Palette {
    title_bar: gpui::Hsla,
    background: gpui::Hsla,
    foreground: gpui::Hsla,
    muted: gpui::Hsla,
    muted_foreground: gpui::Hsla,
    primary: gpui::Hsla,
    danger: gpui::Hsla,
    font_family: gpui::SharedString,
    mono_font_family: gpui::SharedString,
}

impl Palette {
    fn of(cx: &App) -> Self {
        let theme = cx.theme();
        Self {
            title_bar: theme.title_bar,
            background: theme.background,
            foreground: theme.foreground,
            muted: theme.muted,
            muted_foreground: theme.muted_foreground,
            primary: theme.primary,
            danger: theme.danger,
            font_family: theme.font_family.clone(),
            mono_font_family: theme.mono_font_family.clone(),
        }
    }
}

/// 44px custom header matching the Settings window chrome: transparent
/// titlebar, traffic lights aligned by `floating_titlebar_options`.
/// The title is centered across the full window width.
fn header(title: &str, palette: &Palette) -> Div {
    let mut title_area = div()
        .id("handoff-titlebar-drag-area")
        .flex()
        .items_center()
        .justify_center()
        .h_full()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.foreground)
                .child(title.to_string()),
        );
    if cfg!(target_os = "macos") {
        title_area = title_area
            .window_control_area(WindowControlArea::Drag)
            .on_click(|event, window, _cx| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            });
    }
    div()
        .flex()
        .items_center()
        .h(HEADER_HEIGHT)
        .flex_shrink_0()
        .child(title_area)
}

fn group_label(text: &str, palette: &Palette) -> Div {
    div()
        .text_size(px(10.0))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.muted_foreground.opacity(0.5))
        .px(px(2.0))
        .pb(px(2.0))
        .child(text.to_string())
}

fn card(palette: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .rounded(px(12.0))
        .overflow_hidden()
        .p(px(6.0))
        .bg(palette.background.opacity(0.74))
}

/// Rows are inset pills: the card's 6px ring plus the row's own 10px padding
/// keep content at the original 16px offset, while selected/hover fills render
/// as rounded highlights (matching the Settings nav items) instead of
/// full-bleed square bands.
fn card_row() -> Div {
    div()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .w_full()
        .rounded(px(8.0))
        .px(px(10.0))
        .py(px(12.0))
}

fn muted_note(text: String, palette: &Palette) -> Div {
    card_row()
        .text_sm()
        .text_color(palette.muted_foreground)
        .child(text)
}

fn row_icon_tinted(path: &'static str, color: gpui::Hsla) -> gpui::Svg {
    svg().path(path).size_4().flex_none().text_color(color)
}

fn agent_pair(agent: AgentKind, palette: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            svg()
                .path(agent_icon(agent))
                .size_4()
                .flex_none()
                .text_color(palette.foreground),
        )
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(agent.label()),
        )
}

/// Agent pair header on the job card: source → target with the job state
/// label on the trailing edge.
fn job_pair_row(job: &HandoffJob, palette: &Palette) -> Div {
    card_row()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(agent_pair(job.request.source_agent, palette))
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(palette.muted_foreground)
                        .child("→"),
                )
                .child(agent_pair(job.target.agent, palette)),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(palette.muted_foreground)
                .child(job.state.label()),
        )
}

impl HandoffDestinationPanel {
    fn ready(&self) -> bool {
        !self.busy && !self.loading_sessions && self.source_confirmed()
    }

    /// One full-width destination row inside a card: icon, label, muted detail.
    /// Opacity-fill hover per the design language; no borders or shadows.
    /// `selected` uses the vivid primary blue (matching the Settings selected
    /// pattern): tinted fill, primary leading icon and detail, and a trailing
    /// check so the chosen destination is unmistakable.
    #[allow(clippy::too_many_arguments)]
    fn destination_row(
        id: impl Into<ElementId>,
        icon: &'static str,
        title: String,
        detail: String,
        enabled: bool,
        selected: bool,
        palette: &Palette,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut row = card_row()
            .id(id)
            .gap_2()
            .child(row_icon_tinted(
                icon,
                if selected {
                    palette.primary
                } else {
                    palette.muted_foreground
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_ellipsis()
                    .child(title),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.5))
                    .font_weight(if selected {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(if selected {
                        palette.primary
                    } else {
                        palette.muted_foreground
                    })
                    .child(detail),
            );
        if selected {
            row = row
                .bg(palette.primary.opacity(0.10))
                .child(row_icon_tinted("phosphor/check.svg", palette.primary));
        }
        if enabled {
            let hover_bg = if selected {
                palette.primary.opacity(0.14)
            } else {
                palette.muted.opacity(0.08)
            };
            row = row
                .cursor_pointer()
                .hover(move |style| style.bg(hover_bg))
                .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)));
        } else {
            row = row.opacity(0.45);
        }
        row
    }

    /// Full-width confirmation row: the whole row toggles, not just the box.
    /// The Checkbox keeps its own click handler (it prevent_defaults, so a
    /// box click never double-fires the row listener); clicking anywhere
    /// else on the row toggles via the row handler.
    fn check_row(
        id: &'static str,
        label: &'static str,
        checked: bool,
        enabled: bool,
        palette: &Palette,
        on_toggle: impl Fn(&mut Self, bool, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let on_toggle = std::rc::Rc::new(on_toggle);
        let mut row = card_row()
            .id(id)
            .gap_2()
            .child(
                Checkbox::new(format!("{id}-box"))
                    .checked(checked)
                    .disabled(!enabled)
                    .on_click(cx.listener({
                        let on_toggle = on_toggle.clone();
                        move |this, checked: &bool, _, cx| on_toggle(this, *checked, cx)
                    })),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.foreground)
                    .child(label),
            );
        if enabled {
            row = row
                .cursor_pointer()
                .hover(|style| style.bg(palette.muted.opacity(0.08)))
                .on_click(cx.listener(move |this, _, _, cx| on_toggle(this, !checked, cx)));
        } else {
            row = row.opacity(0.45);
        }
        row
    }
}

impl Render for HandoffDestinationPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Palette::of(cx);
        // A previous job never hides the form or blocks another send.
        let has_footer = true;
        let mut body = div()
            // Attach before .id(): the listener lives on Div, which .id()
            // wraps into Stateful<Div>.
            .on_children_prepainted(move |children, window, cx| {
                fit::fit_window_to_content(children, has_footer, window, cx);
            })
            .id("handoff-destination-body")
            .flex()
            .flex_col()
            .gap(px(16.0))
            .px(px(20.0))
            .pt(px(10.0))
            .pb(px(16.0))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();

        if let Some(job) = self.active_job.clone() {
            body = body.child(self.render_job(&job, &palette, cx));
        }
        {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(group_label("SOURCE", &palette))
                    .child(self.render_source_card(&palette, cx)),
            );
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(group_label("CONTINUE IN", &palette))
                    .child(self.render_destination_card(&palette, cx)),
            );
        }

        if let Some(error) = &self.error {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(palette.danger)
                    .child(error.clone()),
            );
        }
        if self.busy {
            body = body.child(
                div()
                    .text_sm()
                    .text_color(palette.muted_foreground)
                    // One click covers prepare plus dispatch, so the status
                    // reads as a send from the user's point of view.
                    .child("Sending handoff…"),
            );
        }

        let mut root = div()
            .size_full()
            .overflow_hidden()
            .flex()
            .flex_col()
            .occlude()
            .font_family(palette.font_family.clone())
            .text_color(palette.foreground)
            .bg(palette.title_bar)
            .child(header("Agent Handoff", &palette))
            .child(body);

        {
            let confirm_ready = self.ready()
                && self.selected_destination.is_some()
                && self.model_error(cx).is_none();
            root = root.child(
                div()
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .px(px(20.0))
                    .py(px(12.0))
                    .flex_shrink_0()
                    .child(
                        Button::new("handoff-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, _| {
                                log::info!("handoff: cancel clicked");
                                window.remove_window();
                            }),
                    )
                    .child(
                        // One click prepares and dispatches: there is no
                        // separate review page to pass through first.
                        Button::new("handoff-send")
                            .primary()
                            .label("Send handoff")
                            .disabled(!confirm_ready)
                            .on_click(cx.listener(|this, _, window, cx| this.confirm(window, cx))),
                    ),
            );
        }

        root
    }
}
