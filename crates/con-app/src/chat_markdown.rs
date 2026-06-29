use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use gpui::{
    AbsoluteLength, AnyElement, AppContext, DefiniteLength, FontStyle, FontWeight, Hsla,
    ImageSource, InteractiveElement, IntoElement, ParentElement, Render, RenderImage, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, StyledText, Task, TextRun, TextStyle,
    UnderlineStyle, WhiteSpace, Window, div, img, px,
};
use gpui_component::ActiveTheme as _;
use gpui_component::clipboard::Clipboard;
use gpui_component::highlighter::SyntaxHighlighter;
use gpui_component::scroll::ScrollableElement;
use gpui_component::{Colorize, Theme};
use markdown::{ParseOptions, mdast};
use ropey::Rope;

use crate::ui_scale::{mono_px, ui_px};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatMarkdownTone {
    Message,
    Thinking,
}

#[derive(Debug, Clone)]
enum MarkdownBlock {
    Paragraph {
        inlines: Vec<MarkdownInline>,
        inline_cache: RefCell<Option<CachedInlineRender>>,
    },
    Heading {
        level: u8,
        inlines: Vec<MarkdownInline>,
        inline_cache: RefCell<Option<CachedInlineRender>>,
    },
    CodeBlock {
        language: Option<String>,
        code: String,
        highlight_cache: RefCell<Option<CachedCodeHighlightRuns>>,
    },
    Mermaid {
        code: SharedString,
        scale: u32,
    },
    MathBlock {
        math: SharedString,
    },
    BlockQuote(Vec<MarkdownBlock>),
    List {
        ordered: bool,
        start: usize,
        items: Vec<Vec<MarkdownBlock>>,
    },
    Table {
        aligns: Vec<MarkdownTableAlign>,
        rows: Vec<Vec<MarkdownTableCell>>,
        text_cache: RefCell<Option<CachedTableRender>>,
    },
    Rule,
}

impl PartialEq for MarkdownBlock {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Paragraph { inlines: a, .. }, Self::Paragraph { inlines: b, .. }) => a == b,
            (
                Self::Heading {
                    level: level_a,
                    inlines: inlines_a,
                    ..
                },
                Self::Heading {
                    level: level_b,
                    inlines: inlines_b,
                    ..
                },
            ) => level_a == level_b && inlines_a == inlines_b,
            (
                Self::CodeBlock {
                    language: language_a,
                    code: code_a,
                    ..
                },
                Self::CodeBlock {
                    language: language_b,
                    code: code_b,
                    ..
                },
            ) => language_a == language_b && code_a == code_b,
            (
                Self::Mermaid {
                    code: code_a,
                    scale: scale_a,
                },
                Self::Mermaid {
                    code: code_b,
                    scale: scale_b,
                },
            ) => code_a == code_b && scale_a == scale_b,
            (Self::MathBlock { math: math_a, .. }, Self::MathBlock { math: math_b, .. }) => {
                math_a == math_b
            }
            (Self::BlockQuote(a), Self::BlockQuote(b)) => a == b,
            (
                Self::List {
                    ordered: ordered_a,
                    start: start_a,
                    items: items_a,
                },
                Self::List {
                    ordered: ordered_b,
                    start: start_b,
                    items: items_b,
                },
            ) => ordered_a == ordered_b && start_a == start_b && items_a == items_b,
            (
                Self::Table {
                    aligns: aligns_a,
                    rows: rows_a,
                    ..
                },
                Self::Table {
                    aligns: aligns_b,
                    rows: rows_b,
                    ..
                },
            ) => aligns_a == aligns_b && rows_a == rows_b,
            (Self::Rule, Self::Rule) => true,
            _ => false,
        }
    }
}

impl Eq for MarkdownBlock {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownTableAlign {
    Left,
    Center,
    Right,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MarkdownInline {
    Text(String),
    Code(String),
    Math(String),
    Emphasis(Vec<MarkdownInline>),
    Strong(Vec<MarkdownInline>),
    Strikethrough(Vec<MarkdownInline>),
    Link {
        label: Vec<MarkdownInline>,
        destination: String,
    },
    SoftBreak,
    LineBreak,
}

#[derive(Debug, Clone)]
struct MarkdownTableCell {
    inlines: Vec<MarkdownInline>,
    inline_cache: RefCell<Option<CachedInlineRender>>,
}

impl PartialEq for MarkdownTableCell {
    fn eq(&self, other: &Self) -> bool {
        self.inlines == other.inlines
    }
}

impl Eq for MarkdownTableCell {}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodeHighlightCacheKey {
    highlight_theme_ptr: usize,
    mono_font_family: SharedString,
    mono_font_size_bits: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedCodeHighlightRuns {
    key: CodeHighlightCacheKey,
    text: SharedString,
    runs: Vec<TextRun>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TableRenderCacheKey {
    mono_font_family: SharedString,
    font_size_bits: u32,
    line_height_bits: u32,
    text_color: Hsla,
    header_color: Hsla,
    separator_color: Hsla,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedTableRender {
    key: TableRenderCacheKey,
    column_widths: Vec<gpui::Pixels>,
    min_width: gpui::Pixels,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InlineRenderCacheKey {
    font_family: SharedString,
    font_size_bits: u32,
    line_height_bits: u32,
    color: Hsla,
    font_weight: FontWeight,
    font_style: FontStyle,
    underline: Option<UnderlineStyle>,
    strikethrough: bool,
    inline_code_background: Hsla,
    inline_code_text_color: Hsla,
    inline_math_background: Hsla,
    math_text_color: Hsla,
    link_color: Hsla,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CachedInlineRender {
    key: InlineRenderCacheKey,
    text: SharedString,
    runs: Vec<TextRun>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RichSvgRenderKind {
    Mermaid,
    Math,
    InlineMath,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RichSvgThemeMode {
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RichSvgRenderKey {
    kind: RichSvgRenderKind,
    source: SharedString,
    metric: u32,
    theme_mode: RichSvgThemeMode,
    color: Option<SharedString>,
}

struct RichSvgRenderEntry {
    image: Option<Result<Arc<RenderImage>, SharedString>>,
    pending: bool,
    task: Option<Task<()>>,
}

struct ChatMarkdownStyle<'a> {
    theme: &'a Theme,
    tone: ChatMarkdownTone,
    content_width: gpui::Pixels,
    base_font_size: gpui::Pixels,
    base_line_height: gpui::Pixels,
    code_font_size: gpui::Pixels,
    code_line_height: gpui::Pixels,
    text_color: Hsla,
    muted_text_color: Hsla,
    inline_code_background: Hsla,
    inline_code_text_color: Hsla,
    inline_math_background: Hsla,
    math_text_color: Hsla,
    math_block_background: Hsla,
    math_block_text_color: Hsla,
    code_block_background: Hsla,
    code_block_body_background: Hsla,
    code_block_language_background: Hsla,
    code_block_language_text_color: Hsla,
    quote_background: Hsla,
    quote_tint: Hsla,
    rule_color: Hsla,
    link_color: Hsla,
    table_border: Hsla,
    table_cell_background: Hsla,
    block_gap: gpui::Pixels,
    inner_gap: gpui::Pixels,
}

impl<'a> ChatMarkdownStyle<'a> {
    fn new(theme: &'a Theme, tone: ChatMarkdownTone) -> Self {
        match tone {
            ChatMarkdownTone::Message => Self {
                theme,
                tone,
                content_width: px(720.0),
                base_font_size: ui_px(theme, 15.0),
                base_line_height: ui_px(theme, 24.0),
                code_font_size: theme.mono_font_size,
                code_line_height: mono_px(theme, 21.0),
                text_color: theme.foreground.opacity(0.88),
                muted_text_color: theme.muted_foreground.opacity(0.74),
                inline_code_background: theme
                    .secondary_active
                    .mix_oklab(theme.background, 0.26)
                    .opacity(0.96),
                inline_code_text_color: theme.foreground.opacity(0.96),
                inline_math_background: theme.primary.opacity(0.08),
                math_text_color: theme.primary.mix_oklab(theme.foreground, 0.58),
                math_block_background: theme.secondary.mix_oklab(theme.background, 0.62),
                math_block_text_color: theme.foreground.opacity(0.92),
                code_block_background: theme.secondary.mix_oklab(theme.background, 0.56),
                code_block_body_background: theme.background.mix_oklab(theme.secondary, 0.90),
                code_block_language_background: theme
                    .secondary_active
                    .mix_oklab(theme.background, 0.24)
                    .opacity(0.92),
                code_block_language_text_color: theme.foreground.opacity(0.74),
                quote_background: theme.secondary.opacity(0.68),
                quote_tint: theme.primary.opacity(0.34),
                rule_color: theme.muted_foreground.opacity(0.16),
                link_color: theme.primary,
                table_border: theme.muted_foreground.opacity(0.10),
                table_cell_background: theme.background.opacity(0.96),
                block_gap: ui_px(theme, 13.0),
                inner_gap: ui_px(theme, 9.0),
            },
            ChatMarkdownTone::Thinking => Self {
                theme,
                tone,
                content_width: px(640.0),
                base_font_size: ui_px(theme, 12.75),
                base_line_height: ui_px(theme, 20.0),
                code_font_size: theme.mono_font_size,
                code_line_height: mono_px(theme, 19.0),
                text_color: theme.muted_foreground.opacity(0.66),
                muted_text_color: theme.muted_foreground.opacity(0.58),
                inline_code_background: theme
                    .secondary_active
                    .mix_oklab(theme.background, 0.20)
                    .opacity(0.90),
                inline_code_text_color: theme.foreground.opacity(0.84),
                inline_math_background: theme.primary.opacity(0.06),
                math_text_color: theme.primary.mix_oklab(theme.muted_foreground, 0.62),
                math_block_background: theme.secondary.mix_oklab(theme.background, 0.52),
                math_block_text_color: theme.foreground.opacity(0.78),
                code_block_background: theme.secondary.mix_oklab(theme.background, 0.48),
                code_block_body_background: theme.background.mix_oklab(theme.secondary, 0.84),
                code_block_language_background: theme
                    .secondary_active
                    .mix_oklab(theme.background, 0.18)
                    .opacity(0.82),
                code_block_language_text_color: theme.foreground.opacity(0.68),
                quote_background: theme.secondary.opacity(0.46),
                quote_tint: theme.primary.opacity(0.24),
                rule_color: theme.muted_foreground.opacity(0.12),
                link_color: theme.primary.opacity(0.82),
                table_border: theme.muted_foreground.opacity(0.08),
                table_cell_background: theme.background.opacity(0.82),
                block_gap: ui_px(theme, 10.0),
                inner_gap: ui_px(theme, 8.0),
            },
        }
    }

    fn base_text_style(&self) -> TextStyle {
        TextStyle {
            color: self.text_color,
            font_family: self.theme.font_family.clone(),
            font_size: self.base_font_size.into(),
            line_height: self.base_line_height.into(),
            font_weight: FontWeight::NORMAL,
            font_style: FontStyle::Normal,
            white_space: WhiteSpace::Normal,
            ..Default::default()
        }
    }

    fn heading_text_style(&self, level: u8) -> TextStyle {
        let (font_size, line_height, weight) = match (self.tone, level) {
            (ChatMarkdownTone::Message, 1) => (
                ui_px(self.theme, 19.0),
                ui_px(self.theme, 27.0),
                FontWeight::BOLD,
            ),
            (ChatMarkdownTone::Message, 2) => (
                ui_px(self.theme, 17.0),
                ui_px(self.theme, 25.0),
                FontWeight::SEMIBOLD,
            ),
            (ChatMarkdownTone::Message, 3) => (
                ui_px(self.theme, 15.5),
                ui_px(self.theme, 23.0),
                FontWeight::SEMIBOLD,
            ),
            (ChatMarkdownTone::Thinking, 1) => (
                ui_px(self.theme, 14.5),
                ui_px(self.theme, 21.0),
                FontWeight::SEMIBOLD,
            ),
            (ChatMarkdownTone::Thinking, 2) => (
                ui_px(self.theme, 13.5),
                ui_px(self.theme, 20.0),
                FontWeight::SEMIBOLD,
            ),
            (ChatMarkdownTone::Thinking, _) => (
                ui_px(self.theme, 12.75),
                ui_px(self.theme, 19.0),
                FontWeight::MEDIUM,
            ),
            (_, _) => (
                self.base_font_size,
                self.base_line_height,
                FontWeight::MEDIUM,
            ),
        };

        TextStyle {
            font_size: font_size.into(),
            line_height: line_height.into(),
            font_weight: weight,
            ..self.base_text_style()
        }
    }

    fn code_text_style(&self) -> TextStyle {
        TextStyle {
            color: self.text_color,
            font_family: self.theme.mono_font_family.clone(),
            font_size: self.code_font_size.into(),
            line_height: self.code_line_height.into(),
            white_space: WhiteSpace::Normal,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedChatMarkdown {
    blocks: Vec<MarkdownBlock>,
}

impl ParsedChatMarkdown {
    pub fn parse(source: &str) -> Self {
        Self {
            blocks: parse_markdown(source),
        }
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}

pub fn render_parsed_chat_markdown(
    document: &ParsedChatMarkdown,
    tone: ChatMarkdownTone,
    theme: &Theme,
) -> AnyElement {
    render_parsed_chat_markdown_prefix(document, tone, theme, document.blocks.len())
}

pub fn render_parsed_chat_markdown_prefix(
    document: &ParsedChatMarkdown,
    tone: ChatMarkdownTone,
    theme: &Theme,
    max_blocks: usize,
) -> AnyElement {
    let style = ChatMarkdownStyle::new(theme, tone);
    let block_count = document.blocks.len().min(max_blocks);
    let blocks = &document.blocks[..block_count];

    if blocks.is_empty() {
        return div().into_any_element();
    }

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(style.block_gap)
        .children(
            blocks
                .iter()
                .enumerate()
                .map(|(idx, block)| render_block_with_width(block, idx, &style, None)),
        )
        .into_any_element()
}

pub fn chat_markdown_block_gap(tone: ChatMarkdownTone, theme: &Theme) -> gpui::Pixels {
    ChatMarkdownStyle::new(theme, tone).block_gap
}

pub struct ChatMarkdownBlockView {
    document: Arc<ParsedChatMarkdown>,
    block_index: usize,
    tone: ChatMarkdownTone,
    table_scroll_handle: ScrollHandle,
    rich_svg_renders: HashMap<RichSvgRenderKey, RichSvgRenderEntry>,
}

impl ChatMarkdownBlockView {
    pub fn new(
        document: Arc<ParsedChatMarkdown>,
        block_index: usize,
        tone: ChatMarkdownTone,
    ) -> Self {
        Self {
            document,
            block_index,
            tone,
            table_scroll_handle: ScrollHandle::new(),
            rich_svg_renders: HashMap::new(),
        }
    }

    pub fn update(
        &mut self,
        document: Arc<ParsedChatMarkdown>,
        block_index: usize,
        tone: ChatMarkdownTone,
        cx: &mut gpui::Context<Self>,
    ) {
        let old_block = self.document.blocks.get(self.block_index);
        let new_block = document.blocks.get(block_index);
        let block_changed =
            self.block_index != block_index || self.tone != tone || old_block != new_block;

        if self.block_index != block_index
            || self.tone != tone
            || !Arc::ptr_eq(&self.document, &document)
        {
            self.document = document;
            self.block_index = block_index;
            self.tone = tone;
            if block_changed {
                self.table_scroll_handle = ScrollHandle::new();
                self.rich_svg_renders.clear();
            }
            cx.notify();
        }
    }

    fn ensure_rich_svg_render(
        &mut self,
        key: RichSvgRenderKey,
        cx: &mut gpui::Context<Self>,
    ) -> (bool, Option<Result<Arc<RenderImage>, SharedString>>) {
        if let Some(entry) = self.rich_svg_renders.get(&key) {
            if entry.image.is_some() || entry.pending {
                return (entry.pending, entry.image.clone());
            }
        }

        let render_key = key.clone();
        let background_key = key.clone();
        let svg_renderer = cx.svg_renderer();
        let task = cx.spawn(async move |this, cx| {
            let result: Result<Arc<RenderImage>, SharedString> = cx
                .background_spawn(async move {
                    let result: anyhow::Result<Arc<RenderImage>> = (|| {
                        let svg = match background_key.kind {
                            RichSvgRenderKind::Mermaid => {
                                let source = mermaid_source_for_theme(
                                    background_key.source.as_ref(),
                                    background_key.theme_mode,
                                );
                                mermaid_rs_renderer::render_with_options(
                                    source.as_ref(),
                                    mermaid_render_options(background_key.theme_mode),
                                )?
                            }
                            RichSvgRenderKind::Math | RichSvgRenderKind::InlineMath => {
                                let options = mathjax_svg_rs::Options {
                                    font_size: background_key.metric as f64 / 1000.0,
                                    horizontal_align: match background_key.kind {
                                        RichSvgRenderKind::InlineMath => {
                                            mathjax_svg_rs::HorizontalAlign::Left
                                        }
                                        _ => mathjax_svg_rs::HorizontalAlign::Center,
                                    },
                                };
                                let svg = mathjax_svg_rs::render_tex(
                                    background_key.source.as_ref(),
                                    &options,
                                )
                                .map_err(anyhow::Error::msg)?;
                                if let Some(color) = background_key.color.as_ref() {
                                    apply_svg_root_color(svg, color.as_ref())
                                } else {
                                    math_svg_for_theme(svg, background_key.theme_mode)
                                }
                            }
                        };
                        svg_renderer
                            .render_single_frame(
                                svg.as_bytes(),
                                rich_svg_render_scale(&background_key),
                            )
                            .map_err(|error| anyhow::anyhow!("{error}"))
                    })();
                    result
                })
                .await
                .map_err(|error| SharedString::from(error.to_string()));

            this.update(cx, |view, cx| {
                if let Some(entry) = view.rich_svg_renders.get_mut(&render_key) {
                    entry.image = Some(result);
                    entry.pending = false;
                    entry.task = None;
                    cx.notify();
                }
            })
            .ok();
        });

        self.rich_svg_renders.insert(
            key,
            RichSvgRenderEntry {
                image: None,
                pending: true,
                task: Some(task),
            },
        );

        (true, None)
    }

    fn render_rich_svg_block(
        &mut self,
        path: &[usize],
        block: &MarkdownBlock,
        style: &ChatMarkdownStyle<'_>,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        let key = rich_svg_key_for_block(block, style)?;
        let render_id = rich_svg_render_id(path, &key);
        let (pending, image) = self.ensure_rich_svg_render(key, cx);
        match block {
            MarkdownBlock::Mermaid { code, scale } => Some(render_mermaid_block(
                render_id,
                code.as_ref(),
                *scale,
                pending,
                image.as_ref(),
                style,
            )),
            MarkdownBlock::MathBlock { math, .. } => Some(render_math_svg_block(
                render_id,
                math.as_ref(),
                pending,
                image.as_ref(),
                style,
            )),
            _ => None,
        }
    }

    fn render_block(
        &mut self,
        block: &MarkdownBlock,
        index: usize,
        style: &ChatMarkdownStyle<'_>,
        table_scroll_handle: Option<&ScrollHandle>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let mut path = vec![index];
        self.render_block_at_path(block, &mut path, style, table_scroll_handle, cx)
    }

    fn render_block_at_path(
        &mut self,
        block: &MarkdownBlock,
        path: &mut Vec<usize>,
        style: &ChatMarkdownStyle<'_>,
        table_scroll_handle: Option<&ScrollHandle>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let index = path.last().copied().unwrap_or(0);
        match block {
            MarkdownBlock::Mermaid { .. } | MarkdownBlock::MathBlock { .. } => self
                .render_rich_svg_block(path, block, style, cx)
                .unwrap_or_else(|| render_block(block, index, style, table_scroll_handle)),
            MarkdownBlock::CodeBlock {
                language,
                code,
                highlight_cache,
            } => render_code_block(
                code_block_copy_id(path, code),
                language,
                code,
                highlight_cache,
                style,
            ),
            MarkdownBlock::Paragraph {
                inlines,
                inline_cache,
            } => div()
                .w_full()
                .child(self.render_inline_content_at_path(
                    path,
                    inlines,
                    &style.base_text_style(),
                    style,
                    inline_cache,
                    cx,
                ))
                .into_any_element(),
            MarkdownBlock::Heading {
                level,
                inlines,
                inline_cache,
            } => div()
                .w_full()
                .pt(px(if *level <= 2 { 3.0 } else { 1.0 }))
                .child(self.render_inline_content_at_path(
                    path,
                    inlines,
                    &style.heading_text_style(*level),
                    style,
                    inline_cache,
                    cx,
                ))
                .into_any_element(),
            MarkdownBlock::BlockQuote(blocks) => {
                let children = blocks
                    .iter()
                    .enumerate()
                    .map(|(idx, block)| {
                        path.push(idx);
                        let rendered = self.render_block_at_path(block, path, style, None, cx);
                        path.pop();
                        rendered
                    })
                    .collect::<Vec<_>>();

                render_blockquote_children(children, style)
            }
            MarkdownBlock::List {
                ordered,
                start,
                items,
            } => {
                let item_children = items
                    .iter()
                    .enumerate()
                    .map(|(item_idx, item_blocks)| {
                        item_blocks
                            .iter()
                            .enumerate()
                            .map(|(nested_idx, nested_block)| {
                                path.push(item_idx);
                                path.push(nested_idx);
                                let rendered =
                                    self.render_block_at_path(nested_block, path, style, None, cx);
                                path.pop();
                                path.pop();
                                rendered
                            })
                            .collect::<Vec<_>>()
                    })
                    .collect::<Vec<_>>();

                render_list_children(*ordered, *start, items.len(), item_children, style)
            }
            _ => render_block(block, index, style, table_scroll_handle),
        }
    }

    fn render_inline_content_at_path(
        &mut self,
        path: &[usize],
        inlines: &[MarkdownInline],
        base_style: &TextStyle,
        style: &ChatMarkdownStyle<'_>,
        inline_cache: &RefCell<Option<CachedInlineRender>>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        if !contains_inline_math(inlines) {
            return render_inline_content(inlines, base_style, style, inline_cache);
        }

        render_inline_flow_content(self, path, inlines, base_style, style, cx)
    }

    fn render_block_with_width(
        &mut self,
        block: &MarkdownBlock,
        index: usize,
        style: &ChatMarkdownStyle<'_>,
        table_scroll_handle: Option<&ScrollHandle>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let mut wrapper = div().w_full();
        if !matches!(
            block,
            MarkdownBlock::CodeBlock { .. }
                | MarkdownBlock::Mermaid { .. }
                | MarkdownBlock::Table { .. }
        ) {
            wrapper = wrapper.max_w(style.content_width);
        }

        wrapper
            .child(self.render_block(block, index, style, table_scroll_handle, cx))
            .into_any_element()
    }
}

impl Render for ChatMarkdownBlockView {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let style = ChatMarkdownStyle::new(&theme, self.tone);
        let document = self.document.clone();
        document
            .blocks
            .get(self.block_index)
            .map(|block| {
                let table_scroll_handle = matches!(block, MarkdownBlock::Table { .. })
                    .then_some(self.table_scroll_handle.clone());
                self.render_block_with_width(
                    block,
                    self.block_index,
                    &style,
                    table_scroll_handle.as_ref(),
                    cx,
                )
            })
            .unwrap_or_else(|| div().into_any_element())
    }
}

fn parse_markdown(source: &str) -> Vec<MarkdownBlock> {
    match markdown::to_mdast(source, &chat_parse_options()) {
        Ok(mdast::Node::Root(root)) => root.children.iter().filter_map(parse_block_node).collect(),
        Ok(node) => parse_block_node(&node).into_iter().collect(),
        Err(_) => vec![MarkdownBlock::Paragraph {
            inlines: vec![MarkdownInline::Text(source.to_string())],
            inline_cache: RefCell::new(None),
        }],
    }
}

fn chat_parse_options() -> ParseOptions {
    let mut options = ParseOptions::gfm();
    options.constructs.math_flow = true;
    options.constructs.math_text = true;
    options
}

fn parse_block_node(node: &mdast::Node) -> Option<MarkdownBlock> {
    match node {
        mdast::Node::Paragraph(val) => Some(MarkdownBlock::Paragraph {
            inlines: parse_inline_nodes(&val.children),
            inline_cache: RefCell::new(None),
        }),
        mdast::Node::Heading(val) => Some(MarkdownBlock::Heading {
            level: val.depth,
            inlines: parse_inline_nodes(&val.children),
            inline_cache: RefCell::new(None),
        }),
        mdast::Node::Code(raw) => parse_mermaid_scale(raw.lang.as_deref(), raw.meta.as_deref())
            .map(|scale| MarkdownBlock::Mermaid {
                code: SharedString::from(raw.value.clone()),
                scale,
            })
            .or_else(|| {
                Some(MarkdownBlock::CodeBlock {
                    language: raw.lang.clone().filter(|lang| !lang.trim().is_empty()),
                    code: raw.value.clone(),
                    highlight_cache: RefCell::new(None),
                })
            }),
        mdast::Node::Blockquote(val) => Some(MarkdownBlock::BlockQuote(
            val.children.iter().filter_map(parse_block_node).collect(),
        )),
        mdast::Node::List(list) => Some(MarkdownBlock::List {
            ordered: list.ordered,
            start: list.start.unwrap_or(1) as usize,
            items: list
                .children
                .iter()
                .filter_map(|item| match item {
                    mdast::Node::ListItem(list_item) => Some(
                        list_item
                            .children
                            .iter()
                            .filter_map(parse_block_node)
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                })
                .collect(),
        }),
        mdast::Node::ThematicBreak(_) => Some(MarkdownBlock::Rule),
        mdast::Node::Table(table) => {
            let rows = table
                .children
                .iter()
                .filter_map(|row| match row {
                    mdast::Node::TableRow(row) => Some(
                        row.children
                            .iter()
                            .filter_map(|cell| match cell {
                                mdast::Node::TableCell(cell) => Some(MarkdownTableCell {
                                    inlines: parse_inline_nodes(&cell.children),
                                    inline_cache: RefCell::new(None),
                                }),
                                _ => None,
                            })
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                })
                .collect::<Vec<_>>();
            Some(MarkdownBlock::Table {
                aligns: table.align.iter().map(parse_table_align).collect(),
                rows,
                text_cache: RefCell::new(None),
            })
        }
        mdast::Node::Html(raw) => {
            let trimmed = raw.value.trim();
            (!trimmed.is_empty()).then(|| MarkdownBlock::Paragraph {
                inlines: vec![MarkdownInline::Text(trimmed.to_string())],
                inline_cache: RefCell::new(None),
            })
        }
        mdast::Node::Yaml(val) => Some(MarkdownBlock::CodeBlock {
            language: Some("yml".to_string()),
            code: val.value.clone(),
            highlight_cache: RefCell::new(None),
        }),
        mdast::Node::Toml(val) => Some(MarkdownBlock::CodeBlock {
            language: Some("toml".to_string()),
            code: val.value.clone(),
            highlight_cache: RefCell::new(None),
        }),
        mdast::Node::Math(val) => Some(MarkdownBlock::MathBlock {
            math: SharedString::from(val.value.clone()),
        }),
        mdast::Node::FootnoteDefinition(def) => Some(MarkdownBlock::Paragraph {
            inlines: std::iter::once(MarkdownInline::Text(format!("[{}]: ", def.identifier)))
                .chain(parse_inline_nodes(&def.children))
                .collect(),
            inline_cache: RefCell::new(None),
        }),
        _ => None,
    }
}

fn parse_table_align(align: &markdown::mdast::AlignKind) -> MarkdownTableAlign {
    match align {
        markdown::mdast::AlignKind::Left => MarkdownTableAlign::Left,
        markdown::mdast::AlignKind::Right => MarkdownTableAlign::Right,
        markdown::mdast::AlignKind::Center => MarkdownTableAlign::Center,
        markdown::mdast::AlignKind::None => MarkdownTableAlign::None,
    }
}

fn parse_mermaid_scale(lang: Option<&str>, meta: Option<&str>) -> Option<u32> {
    let lang = lang?.trim();
    if !lang.eq_ignore_ascii_case("mermaid") {
        return None;
    }

    Some(
        meta.and_then(|meta| meta.split_whitespace().next())
            .and_then(|scale| scale.parse::<u32>().ok())
            .unwrap_or(100)
            .clamp(10, 500),
    )
}

fn parse_inline_nodes(nodes: &[mdast::Node]) -> Vec<MarkdownInline> {
    let mut inlines = Vec::new();
    for node in nodes {
        match node {
            mdast::Node::Text(val) => push_text_fragments(&mut inlines, &val.value),
            mdast::Node::InlineCode(val) => inlines.push(MarkdownInline::Code(val.value.clone())),
            mdast::Node::InlineMath(val) => {
                if looks_like_inline_math(&val.value) {
                    inlines.push(MarkdownInline::Math(val.value.clone()));
                } else {
                    push_text_fragments(&mut inlines, &format!("${}$", val.value));
                }
            }
            mdast::Node::Emphasis(val) => {
                inlines.push(MarkdownInline::Emphasis(parse_inline_nodes(&val.children)))
            }
            mdast::Node::Strong(val) => {
                inlines.push(MarkdownInline::Strong(parse_inline_nodes(&val.children)))
            }
            mdast::Node::Delete(val) => inlines.push(MarkdownInline::Strikethrough(
                parse_inline_nodes(&val.children),
            )),
            mdast::Node::Link(val) => inlines.push(MarkdownInline::Link {
                label: parse_inline_nodes(&val.children),
                destination: val.url.clone(),
            }),
            mdast::Node::LinkReference(val) => inlines.push(MarkdownInline::Link {
                label: parse_inline_nodes(&val.children),
                destination: val.identifier.clone(),
            }),
            mdast::Node::Image(val) => {
                let label = if val.alt.is_empty() {
                    val.url.clone()
                } else {
                    val.alt.clone()
                };
                push_text_fragments(&mut inlines, &label);
            }
            mdast::Node::ImageReference(val) => {
                let label = if val.alt.is_empty() {
                    val.identifier.clone()
                } else {
                    val.alt.clone()
                };
                push_text_fragments(&mut inlines, &label);
            }
            mdast::Node::Break(_) => inlines.push(MarkdownInline::LineBreak),
            mdast::Node::FootnoteReference(val) => {
                push_text_fragments(&mut inlines, &format!("[{}]", val.identifier));
            }
            mdast::Node::Html(val) => {
                push_text_fragments(&mut inlines, &val.value);
            }
            mdast::Node::MdxTextExpression(val) => {
                push_text_fragments(&mut inlines, &val.value);
            }
            mdast::Node::MdxJsxTextElement(val) => {
                inlines.extend(parse_inline_nodes(&val.children));
            }
            _ => {}
        }
    }

    coalesce_inlines(inlines)
}

fn push_text(inlines: &mut Vec<MarkdownInline>, text: &str) {
    if text.is_empty() {
        return;
    }

    if let Some(MarkdownInline::Text(existing)) = inlines.last_mut() {
        existing.push_str(text);
    } else {
        inlines.push(MarkdownInline::Text(text.to_string()));
    }
}

fn push_text_fragments(inlines: &mut Vec<MarkdownInline>, text: &str) {
    if text.is_empty() {
        return;
    }

    let mut parts = text.split('\n').peekable();
    while let Some(part) = parts.next() {
        if !part.is_empty() {
            push_text(inlines, part);
        }
        if parts.peek().is_some() {
            inlines.push(MarkdownInline::SoftBreak);
        }
    }
}

fn coalesce_inlines(inlines: Vec<MarkdownInline>) -> Vec<MarkdownInline> {
    let mut output = Vec::new();

    for inline in inlines {
        match inline {
            MarkdownInline::Text(text) if text.is_empty() => {}
            MarkdownInline::Text(text) => {
                if let Some(MarkdownInline::Text(existing)) = output.last_mut() {
                    existing.push_str(&text);
                } else {
                    output.push(MarkdownInline::Text(text));
                }
            }
            other => output.push(other),
        }
    }

    output
}

fn looks_like_inline_math(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }

    if value.chars().any(|ch| {
        matches!(
            ch,
            '\\' | '^'
                | '_'
                | '='
                | '<'
                | '>'
                | '+'
                | '*'
                | '/'
                | '|'
                | '('
                | ')'
                | '['
                | ']'
                | '{'
                | '}'
        )
    }) {
        return true;
    }

    if value.as_bytes().windows(3).any(|window| {
        window[1] == b'-' && window[0].is_ascii_whitespace() && window[2].is_ascii_whitespace()
    }) {
        return true;
    }

    if value.chars().all(|ch| ch.is_alphanumeric() || ch == '-') && value.contains('-') {
        return false;
    }

    let words = value
        .split_whitespace()
        .map(|word| word.trim_matches(|ch: char| !ch.is_alphanumeric()))
        .collect::<Vec<_>>();
    if words.iter().any(|word| {
        matches!(
            word.to_ascii_lowercase().as_str(),
            "and" | "or" | "the" | "for"
        )
    }) {
        return false;
    }

    words.len() == 1 && value.chars().any(char::is_alphabetic)
}

fn render_block(
    block: &MarkdownBlock,
    index: usize,
    style: &ChatMarkdownStyle<'_>,
    table_scroll_handle: Option<&ScrollHandle>,
) -> AnyElement {
    let mut path = vec![index];
    render_block_at_path(block, &mut path, style, table_scroll_handle)
}

fn render_block_at_path(
    block: &MarkdownBlock,
    path: &mut Vec<usize>,
    style: &ChatMarkdownStyle<'_>,
    table_scroll_handle: Option<&ScrollHandle>,
) -> AnyElement {
    let index = path.last().copied().unwrap_or(0);
    match block {
        MarkdownBlock::Paragraph {
            inlines,
            inline_cache,
        } => div()
            .w_full()
            .child(render_inline_content(
                inlines,
                &style.base_text_style(),
                style,
                inline_cache,
            ))
            .into_any_element(),
        MarkdownBlock::Heading {
            level,
            inlines,
            inline_cache,
        } => div()
            .w_full()
            .pt(px(if *level <= 2 { 3.0 } else { 1.0 }))
            .child(render_inline_content(
                inlines,
                &style.heading_text_style(*level),
                style,
                inline_cache,
            ))
            .into_any_element(),
        MarkdownBlock::CodeBlock {
            language,
            code,
            highlight_cache,
        } => render_code_block(
            code_block_copy_id(path, code),
            language,
            code,
            highlight_cache,
            style,
        ),
        MarkdownBlock::Mermaid { code, scale } => {
            render_mermaid_code_fallback(index, code, *scale, style)
        }
        MarkdownBlock::MathBlock { math, .. } => render_math_block(math, style),
        MarkdownBlock::BlockQuote(blocks) => {
            let children = blocks
                .iter()
                .enumerate()
                .map(|(idx, block)| {
                    path.push(idx);
                    let rendered = render_block_at_path(block, path, style, None);
                    path.pop();
                    rendered
                })
                .collect::<Vec<_>>();

            render_blockquote_children(children, style)
        }
        MarkdownBlock::List {
            ordered,
            start,
            items,
        } => {
            let item_children = items
                .iter()
                .enumerate()
                .map(|(item_idx, item_blocks)| {
                    item_blocks
                        .iter()
                        .enumerate()
                        .map(|(nested_idx, nested_block)| {
                            path.push(item_idx);
                            path.push(nested_idx);
                            let rendered = render_block_at_path(nested_block, path, style, None);
                            path.pop();
                            path.pop();
                            rendered
                        })
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();

            render_list_children(*ordered, *start, items.len(), item_children, style)
        }
        MarkdownBlock::Table {
            aligns,
            rows,
            text_cache,
        } => render_table_block(index, aligns, rows, text_cache, style, table_scroll_handle),
        MarkdownBlock::Rule => div()
            .w_full()
            .h(px(1.0))
            .bg(style.rule_color)
            .into_any_element(),
    }
}

fn render_block_with_width(
    block: &MarkdownBlock,
    index: usize,
    style: &ChatMarkdownStyle<'_>,
    table_scroll_handle: Option<&ScrollHandle>,
) -> AnyElement {
    let mut wrapper = div().w_full();
    if !matches!(
        block,
        MarkdownBlock::CodeBlock { .. }
            | MarkdownBlock::Mermaid { .. }
            | MarkdownBlock::Table { .. }
    ) {
        wrapper = wrapper.max_w(style.content_width);
    }

    wrapper
        .child(render_block(block, index, style, table_scroll_handle))
        .into_any_element()
}

fn ordered_list_marker_lane_width(max_marker: usize) -> gpui::Pixels {
    let digits = max_marker.max(1).to_string().len() as f32;
    px(14.0 + digits * 8.0)
}

fn render_blockquote_children(
    children: Vec<AnyElement>,
    style: &ChatMarkdownStyle<'_>,
) -> AnyElement {
    div()
        .w_full()
        .px(px(10.0))
        .py(px(10.0))
        .rounded(px(8.0))
        .bg(style.quote_background)
        .child(
            div()
                .flex()
                .items_start()
                .gap(px(9.0))
                .child(
                    div()
                        .w(px(3.0))
                        .h_full()
                        .min_h(px(18.0))
                        .bg(style.quote_tint),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(style.inner_gap)
                        .children(children),
                ),
        )
        .into_any_element()
}

fn render_list_children(
    ordered: bool,
    start: usize,
    item_count: usize,
    item_children: Vec<Vec<AnyElement>>,
    style: &ChatMarkdownStyle<'_>,
) -> AnyElement {
    let marker_lane_width = if ordered {
        ordered_list_marker_lane_width(start + item_count.saturating_sub(1))
    } else {
        px(14.0)
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(7.0))
        .children(
            item_children
                .into_iter()
                .enumerate()
                .map(|(item_idx, nested_children)| {
                    let marker = if ordered {
                        format!("{}.", start + item_idx)
                    } else {
                        "\u{2022}".to_string()
                    };

                    div()
                        .w_full()
                        .flex()
                        .items_start()
                        .gap(px(9.0))
                        .child(
                            div()
                                .flex_none()
                                .pt(px(1.0))
                                .w(marker_lane_width)
                                .text_right()
                                .font_family(style.theme.mono_font_family.clone())
                                .text_size(style.base_font_size)
                                .line_height(style.base_line_height)
                                .text_color(style.muted_text_color)
                                .child(marker),
                        )
                        .child(
                            div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(7.0))
                                .flex_1()
                                .children(nested_children),
                        )
                        .into_any_element()
                }),
        )
        .into_any_element()
}

fn render_table_block(
    index: usize,
    aligns: &[MarkdownTableAlign],
    rows: &[Vec<MarkdownTableCell>],
    text_cache: &RefCell<Option<CachedTableRender>>,
    style: &ChatMarkdownStyle<'_>,
    table_scroll_handle: Option<&ScrollHandle>,
) -> AnyElement {
    if rows.is_empty() {
        return div().into_any_element();
    }
    let (column_widths, table_min_width) = cached_table_layout(rows, text_cache, style);

    let mut table_scroll = div()
        .id(("chat-md-table-scroll", index))
        .w_full()
        .overflow_x_scroll();
    table_scroll.style().restrict_scroll_to_axis = Some(true);
    if let Some(handle) = table_scroll_handle {
        table_scroll = table_scroll.track_scroll(handle);
    }

    let mut table_body = div()
        .min_w(table_min_width)
        .overflow_hidden()
        .rounded(px(10.0))
        .bg(style.table_cell_background);

    for (row_idx, row) in rows.iter().enumerate() {
        if row_idx > 0 {
            table_body = table_body.child(
                div()
                    .h(px(1.0))
                    .w_full()
                    .bg(style.table_border.opacity(0.55)),
            );
        }

        let is_header = row_idx == 0;
        let mut row_el = div().flex().items_stretch().w_full().bg(if is_header {
            style.table_border.opacity(0.18)
        } else {
            style.table_cell_background
        });

        for (col_idx, width) in column_widths.iter().enumerate() {
            if col_idx > 0 {
                row_el = row_el.child(
                    div()
                        .w(px(1.0))
                        .flex_none()
                        .bg(style
                            .table_border
                            .opacity(if is_header { 0.52 } else { 0.34 })),
                );
            }

            let cell = row.get(col_idx);
            let mut cell_style = style.base_text_style();
            cell_style.font_size = style.base_font_size.into();
            cell_style.line_height = style.base_line_height.into();
            cell_style.color = if is_header {
                style.text_color.opacity(0.96)
            } else {
                style.text_color.opacity(0.84)
            };
            cell_style.font_weight = if is_header {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::NORMAL
            };

            let content = cell
                .map(|cell| {
                    render_inline_content(&cell.inlines, &cell_style, style, &cell.inline_cache)
                })
                .unwrap_or_else(|| div().into_any_element());

            let mut cell_el = div()
                .w(*width)
                .flex_none()
                .px(px(14.0))
                .py(px(if is_header { 12.0 } else { 11.0 }))
                .min_h(px(if is_header { 46.0 } else { 42.0 }))
                .child(content);

            match aligns
                .get(col_idx)
                .copied()
                .unwrap_or(MarkdownTableAlign::Left)
            {
                MarkdownTableAlign::Right => {
                    cell_el = cell_el.text_right();
                }
                MarkdownTableAlign::Center => {
                    cell_el = cell_el.text_center();
                }
                MarkdownTableAlign::Left | MarkdownTableAlign::None => {}
            }

            row_el = row_el.child(cell_el);
        }

        table_body = table_body.child(row_el);
    }

    let mut table_frame = div()
        .relative()
        .w_full()
        .pb(px(if table_scroll_handle.is_some() {
            8.0
        } else {
            0.0
        }))
        .child(table_scroll.child(table_body));
    if let Some(handle) = table_scroll_handle {
        table_frame = table_frame.horizontal_scrollbar(handle);
    }

    let container = div().w_full().flex().flex_col().child(table_frame);

    container.into_any_element()
}

fn render_code_block(
    copy_id: String,
    language: &Option<String>,
    code: &str,
    highlight_cache: &RefCell<Option<CachedCodeHighlightRuns>>,
    style: &ChatMarkdownStyle<'_>,
) -> AnyElement {
    let header_label = code_block_header_label(language, code);

    let header_row = div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .rounded(px(8.0))
                .bg(style.code_block_language_background)
                .font_family(style.theme.mono_font_family.clone())
                .font_weight(FontWeight::MEDIUM)
                .text_size(px(10.5))
                .line_height(px(11.0))
                .text_color(style.code_block_language_text_color)
                .child(header_label),
        )
        .child(div().h(px(1.0)).flex_1().bg(style.rule_color.opacity(0.36)))
        .child(Clipboard::new(copy_id).value(SharedString::from(code.to_string())));

    let block = div()
        .w_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(13.0))
        .bg(style.code_block_background.opacity(0.98))
        .p(px(1.0))
        .child(
            div()
                .overflow_hidden()
                .rounded(px(12.0))
                .bg(style
                    .code_block_background
                    .mix_oklab(style.code_block_body_background, 0.82))
                .child(
                    div()
                        .px(px(14.0))
                        .pt(px(10.0))
                        .pb(px(8.0))
                        .child(header_row),
                ),
        );

    let (code_text, code_runs) =
        cached_highlighted_code_runs(code, language, highlight_cache, style);
    let code_column = div()
        .w_full()
        .font_family(style.theme.mono_font_family.clone())
        .text_size(style.code_font_size)
        .line_height(style.code_line_height)
        .text_color(style.text_color.opacity(0.96))
        .child(StyledText::new(code_text).with_runs(code_runs));

    block
        .child(
            div().px(px(10.0)).pb(px(10.0)).child(
                div()
                    .rounded(px(10.0))
                    .bg(style.code_block_body_background.opacity(0.985))
                    .px(px(12.0))
                    .py(px(11.0))
                    .child(code_column),
            ),
        )
        .into_any_element()
}

fn render_mermaid_block(
    id: SharedString,
    code: &str,
    scale: u32,
    pending: bool,
    image: Option<&Result<Arc<RenderImage>, SharedString>>,
    style: &ChatMarkdownStyle<'_>,
) -> AnyElement {
    let header = render_mermaid_header(id.clone(), code, scale, style);
    let body = match image {
        Some(Ok(image)) => div()
            .id(id)
            .w_full()
            .overflow_x_scroll()
            .child(
                div()
                    .min_w(px(240.0))
                    .p(px(14.0))
                    .child(img(ImageSource::Render(image.clone())).flex_none()),
            )
            .into_any_element(),
        Some(Err(error)) => div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(14.0))
            .child(
                div()
                    .font_family(style.theme.font_family.clone())
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .text_color(style.muted_text_color)
                    .child(format!("Could not render Mermaid diagram: {error}")),
            )
            .child(render_mermaid_source_text(code, style))
            .into_any_element(),
        None => div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(14.0))
            .child(
                div()
                    .font_family(style.theme.font_family.clone())
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .text_color(style.muted_text_color)
                    .child(if pending {
                        "Rendering Mermaid diagram..."
                    } else {
                        "Mermaid diagram"
                    }),
            )
            .child(render_mermaid_source_text(code, style))
            .into_any_element(),
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(13.0))
        .bg(style.code_block_background.opacity(0.98))
        .p(px(1.0))
        .child(header)
        .child(
            div()
                .mx(px(10.0))
                .mb(px(10.0))
                .rounded(px(10.0))
                .bg(style.code_block_body_background.opacity(0.985))
                .child(body),
        )
        .into_any_element()
}

fn render_math_svg_block(
    id: SharedString,
    math: &str,
    pending: bool,
    image: Option<&Result<Arc<RenderImage>, SharedString>>,
    style: &ChatMarkdownStyle<'_>,
) -> AnyElement {
    let body = match image {
        Some(Ok(image)) => div()
            .id(id)
            .w_full()
            .overflow_x_scroll()
            .child(
                div()
                    .min_w(px(180.0))
                    .px(px(16.0))
                    .py(px(14.0))
                    .child(img(ImageSource::Render(image.clone())).flex_none()),
            )
            .into_any_element(),
        Some(Err(error)) => div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(16.0))
            .py(px(13.0))
            .child(
                div()
                    .font_family(style.theme.font_family.clone())
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .text_color(style.muted_text_color)
                    .child(format!("Could not render LaTeX: {error}")),
            )
            .child(render_math_source_text(math, style))
            .into_any_element(),
        None => div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .px(px(16.0))
            .py(px(13.0))
            .child(
                div()
                    .font_family(style.theme.font_family.clone())
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .text_color(style.muted_text_color)
                    .child(if pending {
                        "Rendering LaTeX..."
                    } else {
                        "LaTeX"
                    }),
            )
            .child(render_math_source_text(math, style))
            .into_any_element(),
    };

    div()
        .w_full()
        .max_w(style.content_width)
        .overflow_hidden()
        .rounded(px(12.0))
        .bg(style.math_block_background.opacity(0.92))
        .child(body)
        .into_any_element()
}

fn render_mermaid_code_fallback(
    index: usize,
    code: &str,
    scale: u32,
    style: &ChatMarkdownStyle<'_>,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(13.0))
        .bg(style.code_block_background.opacity(0.98))
        .p(px(1.0))
        .child(render_mermaid_header(
            SharedString::from(format!("chat-md-mermaid-fallback-{index}")),
            code,
            scale,
            style,
        ))
        .child(
            div()
                .mx(px(10.0))
                .mb(px(10.0))
                .rounded(px(10.0))
                .bg(style.code_block_body_background.opacity(0.985))
                .p(px(12.0))
                .child(render_mermaid_source_text(code, style)),
        )
        .into_any_element()
}

fn render_mermaid_header(
    id: SharedString,
    code: &str,
    scale: u32,
    style: &ChatMarkdownStyle<'_>,
) -> AnyElement {
    let label = if scale == 100 {
        "mermaid".to_string()
    } else {
        format!("mermaid {scale}%")
    };

    div()
        .px(px(14.0))
        .pt(px(10.0))
        .pb(px(8.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .px(px(8.0))
                        .py(px(4.0))
                        .rounded(px(8.0))
                        .bg(style.code_block_language_background)
                        .font_family(style.theme.mono_font_family.clone())
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(px(10.5))
                        .line_height(px(11.0))
                        .text_color(style.code_block_language_text_color)
                        .child(label),
                )
                .child(div().h(px(1.0)).flex_1().bg(style.rule_color.opacity(0.36)))
                .child(
                    Clipboard::new(format!("{}-copy", id.as_ref()))
                        .value(SharedString::from(code.to_string())),
                ),
        )
        .into_any_element()
}

fn render_mermaid_source_text(code: &str, style: &ChatMarkdownStyle<'_>) -> AnyElement {
    let text = code_display_text(code);
    div()
        .w_full()
        .font_family(style.theme.mono_font_family.clone())
        .text_size(style.code_font_size)
        .line_height(style.code_line_height)
        .text_color(style.text_color.opacity(0.82))
        .child(StyledText::new(SharedString::from(text)))
        .into_any_element()
}

fn render_math_block(math: &str, style: &ChatMarkdownStyle<'_>) -> AnyElement {
    div()
        .w_full()
        .max_w(style.content_width)
        .rounded(px(12.0))
        .bg(style.math_block_background.opacity(0.92))
        .px(px(16.0))
        .py(px(13.0))
        .child(render_math_source_text(math, style))
        .into_any_element()
}

fn render_math_source_text(math: &str, style: &ChatMarkdownStyle<'_>) -> AnyElement {
    div()
        .w_full()
        .font_family(style.theme.mono_font_family.clone())
        .text_size(style.code_font_size + px(1.0))
        .line_height(style.code_line_height + px(3.0))
        .text_color(style.math_block_text_color)
        .child(StyledText::new(SharedString::from(math.to_string())))
        .into_any_element()
}

fn math_font_size_metric(style: &ChatMarkdownStyle<'_>) -> u32 {
    let font_size: f32 = (style.code_font_size + px(3.0)).into();
    (font_size * 1000.0).round().max(1.0) as u32
}

fn inline_math_font_size_metric(base_style: &TextStyle) -> u32 {
    let font_size: f32 = text_style_font_size(base_style).into();
    (font_size * 1000.0).round().max(1.0) as u32
}

fn math_svg_for_theme(svg: String, theme_mode: RichSvgThemeMode) -> String {
    let color = match theme_mode {
        RichSvgThemeMode::Light => "#0F172A",
        RichSvgThemeMode::Dark => "#F8FAFC",
    };
    apply_svg_root_color(svg, color)
}

fn svg_color_for_hsla_over_background(color: Hsla, background: Hsla) -> String {
    let fg = color.to_rgb();
    let bg = background.to_rgb();
    let alpha = fg.a.clamp(0.0, 1.0);
    let blend = |fg: f32, bg: f32| ((fg * alpha + bg * (1.0 - alpha)) * 255.0).round() as u8;
    format!(
        "#{:02X}{:02X}{:02X}",
        blend(fg.r, bg.r),
        blend(fg.g, bg.g),
        blend(fg.b, bg.b)
    )
}

fn apply_svg_root_color(mut svg: String, color: &str) -> String {
    let Some(svg_start) = svg.find("<svg") else {
        return svg;
    };
    let Some(tag_end_offset) = svg[svg_start..].find('>') else {
        return svg;
    };
    let tag_end = svg_start + tag_end_offset;
    let root_tag = &svg[svg_start..tag_end];
    if root_tag.contains(" color=") || root_tag.contains(" fill=") {
        return svg;
    }

    svg.insert_str(tag_end, &format!(" color=\"{color}\" fill=\"{color}\""));
    svg
}

fn rich_svg_theme_mode(style: &ChatMarkdownStyle<'_>) -> RichSvgThemeMode {
    if style.theme.is_dark() {
        RichSvgThemeMode::Dark
    } else {
        RichSvgThemeMode::Light
    }
}

fn mermaid_render_options(theme_mode: RichSvgThemeMode) -> mermaid_rs_renderer::RenderOptions {
    let mut options = mermaid_rs_renderer::RenderOptions::default();
    if matches!(theme_mode, RichSvgThemeMode::Dark) {
        options.theme = mermaid_dark_theme();
    }
    options
}

fn mermaid_source_for_theme(source: &str, theme_mode: RichSvgThemeMode) -> Cow<'_, str> {
    if !matches!(theme_mode, RichSvgThemeMode::Dark) {
        return Cow::Borrowed(source);
    }

    let mut rewritten = None::<String>;
    let mut cursor = 0;
    for line in source.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let should_rewrite = (trimmed.starts_with("style ") || trimmed.starts_with("classDef "))
            && mermaid_style_has_key(trimmed, "fill")
            && !mermaid_style_has_key(trimmed, "color");
        if should_rewrite && let Some(fill) = mermaid_style_value(trimmed, "fill") {
            let text_color = if mermaid_fill_is_light(fill) {
                "#0F172A"
            } else {
                "#F8FAFC"
            };
            let out = rewritten.get_or_insert_with(|| source[..cursor].to_string());
            let line_without_newline = line.trim_end_matches(|ch| ch == '\r' || ch == '\n');
            out.push_str(line_without_newline);
            out.push_str(",color:");
            out.push_str(text_color);
            if line.ends_with('\n') {
                out.push('\n');
            }
        } else if let Some(out) = rewritten.as_mut() {
            out.push_str(line);
        }
        cursor += line.len();
    }

    if cursor < source.len() {
        if let Some(out) = rewritten.as_mut() {
            out.push_str(&source[cursor..]);
        }
    }

    rewritten.map_or(Cow::Borrowed(source), Cow::Owned)
}

fn mermaid_style_has_key(line: &str, key: &str) -> bool {
    mermaid_style_value(line, key).is_some()
}

fn mermaid_style_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let styles = if let Some(rest) = line.strip_prefix("style ") {
        rest.trim_start().split_once(char::is_whitespace)?.1
    } else if let Some(rest) = line.strip_prefix("classDef ") {
        rest.trim_start().split_once(char::is_whitespace)?.1
    } else {
        return None;
    };

    styles.trim_start().split(',').find_map(|part| {
        let (part_key, value) = part.split_once(':')?;
        (part_key.trim() == key).then(|| value.trim())
    })
}

fn mermaid_fill_is_light(fill: &str) -> bool {
    let Some((r, g, b)) = mermaid_hex_color(fill) else {
        return false;
    };
    let luminance = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
    luminance >= 150.0
}

fn mermaid_hex_color(fill: &str) -> Option<(u8, u8, u8)> {
    let fill = fill.trim();
    let hex = fill.strip_prefix('#')?;
    match hex.len() {
        3 => {
            let mut chars = hex.chars();
            let r = chars.next()?.to_digit(16)? as u8;
            let g = chars.next()?.to_digit(16)? as u8;
            let b = chars.next()?.to_digit(16)? as u8;
            Some((r * 17, g * 17, b * 17))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b))
        }
        _ => None,
    }
}

fn mermaid_dark_theme() -> mermaid_rs_renderer::Theme {
    let mut theme = mermaid_rs_renderer::Theme::modern();
    theme.primary_color = "#1E293B".to_string();
    theme.primary_text_color = "#F8FAFC".to_string();
    theme.primary_border_color = "#64748B".to_string();
    theme.line_color = "#94A3B8".to_string();
    theme.secondary_color = "#334155".to_string();
    theme.tertiary_color = "#0F172A".to_string();
    theme.edge_label_background = "#0F172A".to_string();
    theme.cluster_background = "#111827".to_string();
    theme.cluster_border = "#475569".to_string();
    theme.background = "#0B1120".to_string();
    theme.sequence_actor_fill = "#1E293B".to_string();
    theme.sequence_actor_border = "#64748B".to_string();
    theme.sequence_actor_line = "#64748B".to_string();
    theme.sequence_note_fill = "#422006".to_string();
    theme.sequence_note_border = "#B45309".to_string();
    theme.sequence_activation_fill = "#334155".to_string();
    theme.sequence_activation_border = "#94A3B8".to_string();
    theme.text_color = "#E2E8F0".to_string();
    theme.pie_title_text_color = "#F8FAFC".to_string();
    theme.pie_section_text_color = "#F8FAFC".to_string();
    theme.pie_legend_text_color = "#CBD5E1".to_string();
    theme.pie_stroke_color = "#0F172A".to_string();
    theme.pie_outer_stroke_color = "#475569".to_string();
    theme
}

fn rich_svg_key_for_block(
    block: &MarkdownBlock,
    style: &ChatMarkdownStyle<'_>,
) -> Option<RichSvgRenderKey> {
    match block {
        MarkdownBlock::Mermaid { code, scale } => Some(RichSvgRenderKey {
            kind: RichSvgRenderKind::Mermaid,
            source: code.clone(),
            metric: *scale,
            theme_mode: rich_svg_theme_mode(style),
            color: None,
        }),
        MarkdownBlock::MathBlock { math, .. } => Some(RichSvgRenderKey {
            kind: RichSvgRenderKind::Math,
            source: math.clone(),
            metric: math_font_size_metric(style),
            theme_mode: rich_svg_theme_mode(style),
            color: None,
        }),
        _ => None,
    }
}

fn rich_svg_render_id(path: &[usize], key: &RichSvgRenderKey) -> SharedString {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let kind = match key.kind {
        RichSvgRenderKind::Mermaid => "mermaid",
        RichSvgRenderKind::Math => "math",
        RichSvgRenderKind::InlineMath => "inline-math",
    };
    let path = markdown_path_id(path);
    SharedString::from(format!("chat-md-{kind}-{path}-{:x}", hasher.finish()))
}

fn code_block_copy_id(path: &[usize], code: &str) -> String {
    let mut hasher = DefaultHasher::new();
    code.hash(&mut hasher);
    format!(
        "copy-code-block-{}-{:x}",
        markdown_path_id(path),
        hasher.finish()
    )
}

fn markdown_path_id(path: &[usize]) -> String {
    if path.is_empty() {
        return "root".to_string();
    }

    path.iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

fn rich_svg_render_scale(key: &RichSvgRenderKey) -> f32 {
    match key.kind {
        RichSvgRenderKind::Mermaid => key.metric as f32 / 100.0,
        RichSvgRenderKind::Math | RichSvgRenderKind::InlineMath => 1.0,
    }
}

fn cached_table_layout(
    rows: &[Vec<MarkdownTableCell>],
    cache: &RefCell<Option<CachedTableRender>>,
    style: &ChatMarkdownStyle<'_>,
) -> (Vec<gpui::Pixels>, gpui::Pixels) {
    let key = TableRenderCacheKey {
        mono_font_family: style.theme.mono_font_family.clone(),
        font_size_bits: {
            let size: f32 = style.base_font_size.into();
            size.to_bits()
        },
        line_height_bits: {
            let height: f32 = style.base_line_height.into();
            height.to_bits()
        },
        text_color: style.text_color.opacity(0.9),
        header_color: style.text_color.opacity(0.97),
        separator_color: style.muted_text_color.opacity(0.78),
    };

    {
        let cached = cache.borrow();
        if let Some(cached) = cached.as_ref()
            && cached.key == key
        {
            return (cached.column_widths.clone(), cached.min_width);
        }
    }

    let (column_widths, min_width) = table_layout(rows);
    *cache.borrow_mut() = Some(CachedTableRender {
        key,
        column_widths: column_widths.clone(),
        min_width,
    });
    (column_widths, min_width)
}

fn table_layout(rows: &[Vec<MarkdownTableCell>]) -> (Vec<gpui::Pixels>, gpui::Pixels) {
    let column_count = rows.iter().map(|row| row.len()).max().unwrap_or(0).max(1);
    let mut char_widths = vec![3usize; column_count];
    for row in rows {
        for (idx, cell) in row.iter().enumerate() {
            char_widths[idx] = char_widths[idx].max(table_cell_measure_chars(&cell.inlines));
        }
    }

    let column_widths = char_widths
        .into_iter()
        .map(|chars| {
            let measured = chars as f32 * 8.2 + 36.0;
            px(measured.clamp(112.0, 320.0))
        })
        .collect::<Vec<_>>();
    let separators = column_widths.len().saturating_sub(1) as f32;
    let min_width = column_widths
        .iter()
        .copied()
        .fold(px(0.0), |total, width| total + width)
        + px(separators);

    (column_widths, min_width)
}

fn table_cell_measure_chars(inlines: &[MarkdownInline]) -> usize {
    inlines
        .iter()
        .map(|inline| match inline {
            MarkdownInline::Text(text)
            | MarkdownInline::Code(text)
            | MarkdownInline::Math(text) => text
                .split_whitespace()
                .map(|word| word.chars().count())
                .max()
                .unwrap_or(0)
                .max(text.chars().count().min(24)),
            MarkdownInline::Emphasis(children)
            | MarkdownInline::Strong(children)
            | MarkdownInline::Strikethrough(children) => table_cell_measure_chars(children),
            MarkdownInline::Link { label, .. } => table_cell_measure_chars(label),
            MarkdownInline::SoftBreak | MarkdownInline::LineBreak => 1,
        })
        .max()
        .unwrap_or(3)
}

fn cached_highlighted_code_runs(
    code: &str,
    language: &Option<String>,
    cache: &RefCell<Option<CachedCodeHighlightRuns>>,
    style: &ChatMarkdownStyle<'_>,
) -> (SharedString, Vec<TextRun>) {
    let key = CodeHighlightCacheKey {
        highlight_theme_ptr: Arc::as_ptr(&style.theme.highlight_theme) as usize,
        mono_font_family: style.theme.mono_font_family.clone(),
        mono_font_size_bits: {
            let size: f32 = style.code_font_size.into();
            size.to_bits()
        },
    };

    {
        let cached = cache.borrow();
        if let Some(cached) = cached.as_ref()
            && cached.key == key
        {
            return (cached.text.clone(), cached.runs.clone());
        }
    }

    let (text, runs) = highlighted_code_runs(code, language, style);
    *cache.borrow_mut() = Some(CachedCodeHighlightRuns {
        key,
        text: text.clone(),
        runs: runs.clone(),
    });
    (text, runs)
}

fn highlighted_code_runs(
    code: &str,
    language: &Option<String>,
    style: &ChatMarkdownStyle<'_>,
) -> (SharedString, Vec<gpui::TextRun>) {
    let display_text = code_display_text(code);
    let base_style = style.code_text_style();
    let display_len = display_text.len();
    let lang = effective_highlighter_language(language, code);

    let rope = Rope::from_str(code);
    let mut runs = if let Some(lang) = lang.as_deref() {
        let mut highlighter = SyntaxHighlighter::new(lang);
        highlighter.update(None, &rope, None);
        let highlights = highlighter.styles(&(0..code.len()), &style.theme.highlight_theme);
        let mut runs = Vec::new();
        let mut cursor = 0usize;

        for (range, highlight) in &highlights {
            let start = range.start.min(code.len());
            let end = range.end.min(code.len());
            if start >= end {
                continue;
            }

            if start > cursor {
                runs.push(base_style.to_run(start - cursor));
            }

            let mut highlighted_style = base_style.clone();
            apply_code_highlight_style(&mut highlighted_style, *highlight, style);
            runs.push(highlighted_style.to_run(end - start));
            cursor = end;
        }

        if cursor < code.len() {
            runs.push(base_style.to_run(code.len() - cursor));
        }

        runs
    } else {
        vec![base_style.to_run(code.len())]
    };

    if runs.is_empty() {
        runs.push(base_style.to_run(display_len.max(1)));
    } else if display_len > code.len() {
        runs.push(base_style.to_run(display_len - code.len()));
    }

    (display_text.into(), runs)
}

fn code_display_text(code: &str) -> String {
    if code.is_empty() {
        return "\u{200B}".to_string();
    }

    let mut text = code.to_string();
    if code.ends_with('\n') {
        text.push('\u{200B}');
    }
    text
}

fn canonical_highlighter_language(language: &str) -> &str {
    match language.trim().to_ascii_lowercase().as_str() {
        "sh" | "shell" | "zsh" | "console" | "terminal" => "bash",
        other => {
            if other.is_empty() {
                ""
            } else {
                language.trim()
            }
        }
    }
}

fn code_block_header_label(language: &Option<String>, code: &str) -> String {
    if let Some(lang) = effective_highlighter_language(language, code) {
        return lang;
    }

    language
        .as_deref()
        .map(str::trim)
        .filter(|lang| !lang.is_empty())
        .unwrap_or("code")
        .to_string()
}

fn effective_highlighter_language(language: &Option<String>, code: &str) -> Option<String> {
    let raw_language = language
        .as_deref()
        .map(str::trim)
        .filter(|lang| !lang.is_empty());

    // Some models emit terminal transcripts as ```code or unlabeled fences.
    // Keep this inference intentionally narrow: only generic fences can be
    // reclassified, explicit `text` remains plain, and the source must begin
    // with a recognizable shell prompt.
    if raw_language.map(is_generic_code_language).unwrap_or(true)
        && looks_like_shell_transcript(code)
    {
        return Some("bash".to_string());
    }

    raw_language
        .map(canonical_highlighter_language)
        .filter(|lang| {
            !lang.is_empty()
                && !is_generic_code_language(lang)
                && !suppress_syntax_highlighting(lang)
        })
        .map(ToOwned::to_owned)
}

fn is_generic_code_language(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "code" | "source"
    )
}

fn looks_like_shell_transcript(code: &str) -> bool {
    let mut meaningful_lines = 0usize;
    let mut prompt_lines = 0usize;
    let mut first_meaningful_line_is_prompt = false;

    for line in code.lines().take(24) {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        meaningful_lines += 1;
        if is_shell_prompt_line(trimmed) {
            prompt_lines += 1;
            if meaningful_lines == 1 {
                first_meaningful_line_is_prompt = true;
            }
        }
    }

    prompt_lines > 0 && (first_meaningful_line_is_prompt || prompt_lines * 2 >= meaningful_lines)
}

fn is_shell_prompt_line(trimmed: &str) -> bool {
    trimmed == "$"
        || trimmed.starts_with("$ ")
        || trimmed.starts_with("$\t")
        || looks_like_compact_dollar_prompt(trimmed)
        || trimmed.starts_with("% ")
        || trimmed.starts_with("# ")
        || trimmed.starts_with("\u{276f} ")
        || trimmed.starts_with("\u{279c} ")
        || trimmed.starts_with("\u{03bb} ")
}

fn looks_like_compact_dollar_prompt(trimmed: &str) -> bool {
    let Some(rest) = trimmed.strip_prefix('$') else {
        return false;
    };
    let Some(first) = rest.chars().next() else {
        return false;
    };
    first.is_ascii_alphanumeric() && rest.contains(char::is_whitespace)
}

fn suppress_syntax_highlighting(lang: &str) -> bool {
    matches!(lang.to_ascii_lowercase().as_str(), "text" | "txt" | "plain")
}

fn apply_code_highlight_style(
    text_style: &mut TextStyle,
    highlight: gpui::HighlightStyle,
    style: &ChatMarkdownStyle<'_>,
) {
    let base_color = if style.theme.is_dark() {
        style.text_color.opacity(0.96)
    } else {
        style.text_color.opacity(0.90)
    };
    text_style.font_family = style.theme.mono_font_family.clone();
    text_style.font_size = style.code_font_size.into();
    text_style.line_height = style.code_line_height.into();
    text_style.font_style = FontStyle::Normal;
    text_style.font_weight = FontWeight::NORMAL;
    text_style.background_color = None;
    text_style.underline = None;
    text_style.strikethrough = None;
    text_style.color = highlight
        .color
        .map(|color| color.mix_oklab(base_color, 0.76).opacity(0.99))
        .unwrap_or(base_color);
}

fn render_inline_text(
    inlines: &[MarkdownInline],
    base_style: &TextStyle,
    style: &ChatMarkdownStyle<'_>,
    inline_cache: &RefCell<Option<CachedInlineRender>>,
) -> AnyElement {
    let (text, runs) = cached_inline_runs(inlines, base_style, style, inline_cache);
    let font_size = text_style_font_size(base_style);
    let line_height = text_style_line_height(base_style, font_size);
    div()
        .w_full()
        .font_family(base_style.font_family.clone())
        .text_size(font_size)
        .line_height(line_height)
        .text_color(base_style.color)
        .child(StyledText::new(text).with_runs(runs))
        .into_any_element()
}

fn render_inline_content(
    inlines: &[MarkdownInline],
    base_style: &TextStyle,
    style: &ChatMarkdownStyle<'_>,
    inline_cache: &RefCell<Option<CachedInlineRender>>,
) -> AnyElement {
    render_inline_text(inlines, base_style, style, inline_cache)
}

#[derive(Debug)]
enum InlineFlowItem {
    Text {
        text: SharedString,
        runs: Vec<TextRun>,
    },
    Math {
        value: String,
        index: usize,
    },
    LineBreak,
}

fn render_inline_flow_content(
    view: &mut ChatMarkdownBlockView,
    path: &[usize],
    inlines: &[MarkdownInline],
    base_style: &TextStyle,
    style: &ChatMarkdownStyle<'_>,
    cx: &mut gpui::Context<ChatMarkdownBlockView>,
) -> AnyElement {
    let mut items = Vec::new();
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut math_index = 0;

    collect_inline_flow_items(
        inlines,
        base_style.clone(),
        style,
        &mut items,
        &mut text,
        &mut runs,
        &mut math_index,
    );
    flush_inline_flow_text(&mut items, &mut text, &mut runs);

    if items.is_empty() {
        return div().into_any_element();
    }

    let font_size = text_style_font_size(base_style);
    let line_height = text_style_line_height(base_style, font_size);
    let mut root = div()
        .w_full()
        .flex()
        .flex_wrap()
        .items_baseline()
        .font_family(base_style.font_family.clone())
        .text_size(font_size)
        .line_height(line_height)
        .text_color(base_style.color);

    for item in items {
        root = root.child(match item {
            InlineFlowItem::Text { text, runs } => {
                render_inline_flow_text_item(text, runs, base_style, style.content_width)
            }
            InlineFlowItem::Math { value, index } => {
                let key = RichSvgRenderKey {
                    kind: RichSvgRenderKind::InlineMath,
                    source: SharedString::from(value.clone()),
                    metric: inline_math_font_size_metric(base_style),
                    theme_mode: rich_svg_theme_mode(style),
                    color: Some(SharedString::from(svg_color_for_hsla_over_background(
                        style.text_color,
                        style.theme.background,
                    ))),
                };
                let mut math_path = path.to_vec();
                math_path.push(index);
                let id = rich_svg_render_id(&math_path, &key);
                let (pending, image) = view.ensure_rich_svg_render(key, cx);
                render_inline_math_item(id, &value, pending, image.as_ref(), base_style, style)
            }
            InlineFlowItem::LineBreak => div().w_full().h(px(0.0)).into_any_element(),
        });
    }

    root.into_any_element()
}

fn render_inline_flow_text_item(
    text: SharedString,
    runs: Vec<TextRun>,
    base_style: &TextStyle,
    max_width: gpui::Pixels,
) -> AnyElement {
    let font_size = text_style_font_size(base_style);
    let line_height = text_style_line_height(base_style, font_size);
    div()
        .min_w_0()
        .max_w(max_width)
        .font_family(base_style.font_family.clone())
        .text_size(font_size)
        .line_height(line_height)
        .text_color(base_style.color)
        .child(StyledText::new(text).with_runs(runs))
        .into_any_element()
}

fn render_inline_math_item(
    id: SharedString,
    math: &str,
    pending: bool,
    image: Option<&Result<Arc<RenderImage>, SharedString>>,
    base_style: &TextStyle,
    style: &ChatMarkdownStyle<'_>,
) -> AnyElement {
    let font_size = text_style_font_size(base_style);
    let line_height = text_style_line_height(base_style, font_size);
    match image {
        Some(Ok(image)) => div()
            .id(id)
            .h(line_height)
            .flex()
            .items_center()
            .flex_none()
            .child(img(ImageSource::Render(image.clone())).flex_none())
            .into_any_element(),
        Some(Err(_)) | None => div()
            .id(id)
            .h(line_height)
            .flex()
            .items_center()
            .px(px(3.0))
            .py(px(0.0))
            .rounded(px(4.0))
            .bg(style.inline_math_background)
            .font_family(style.theme.mono_font_family.clone())
            .text_size((font_size - px(1.0)).max(px(10.0)))
            .line_height(line_height)
            .text_color(
                style
                    .math_text_color
                    .opacity(if pending { 0.70 } else { 0.92 }),
            )
            .child(math.to_string())
            .into_any_element(),
    }
}

fn collect_inline_flow_items(
    inlines: &[MarkdownInline],
    current_style: TextStyle,
    style: &ChatMarkdownStyle<'_>,
    items: &mut Vec<InlineFlowItem>,
    text: &mut String,
    runs: &mut Vec<TextRun>,
    math_index: &mut usize,
) {
    for inline in inlines {
        match inline {
            MarkdownInline::Text(value) => push_run(text, runs, &current_style, value),
            MarkdownInline::Code(value) => {
                let mut code_style = current_style.clone();
                code_style.font_family = style.theme.mono_font_family.clone();
                code_style.background_color = Some(style.inline_code_background);
                code_style.font_weight = FontWeight::MEDIUM;
                code_style.color = style.inline_code_text_color;
                push_run(text, runs, &code_style, value);
            }
            MarkdownInline::Math(value) => {
                flush_inline_flow_text(items, text, runs);
                let index = *math_index;
                *math_index += 1;
                items.push(InlineFlowItem::Math {
                    value: value.clone(),
                    index,
                });
            }
            MarkdownInline::Emphasis(children) => {
                let mut emphasis = current_style.clone();
                emphasis.font_style = FontStyle::Italic;
                collect_inline_flow_items(children, emphasis, style, items, text, runs, math_index);
            }
            MarkdownInline::Strong(children) => {
                let mut strong = current_style.clone();
                strong.font_weight = FontWeight::SEMIBOLD;
                collect_inline_flow_items(children, strong, style, items, text, runs, math_index);
            }
            MarkdownInline::Strikethrough(children) => {
                let mut struck = current_style.clone();
                struck.strikethrough = Some(gpui::StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(current_style.color.opacity(0.55)),
                    ..Default::default()
                });
                collect_inline_flow_items(children, struck, style, items, text, runs, math_index);
            }
            MarkdownInline::Link { label, .. } => {
                let mut link_style = current_style.clone();
                link_style.color = style.link_color;
                link_style.underline = Some(UnderlineStyle {
                    color: Some(style.link_color.opacity(0.48)),
                    thickness: px(1.0),
                    wavy: false,
                });
                collect_inline_flow_items(label, link_style, style, items, text, runs, math_index);
            }
            MarkdownInline::SoftBreak => push_run(text, runs, &current_style, " "),
            MarkdownInline::LineBreak => {
                flush_inline_flow_text(items, text, runs);
                items.push(InlineFlowItem::LineBreak);
            }
        }
    }
}

fn flush_inline_flow_text(
    items: &mut Vec<InlineFlowItem>,
    text: &mut String,
    runs: &mut Vec<TextRun>,
) {
    if text.is_empty() {
        return;
    }

    items.push(InlineFlowItem::Text {
        text: SharedString::from(std::mem::take(text)),
        runs: std::mem::take(runs),
    });
}

fn contains_inline_math(inlines: &[MarkdownInline]) -> bool {
    inlines.iter().any(|inline| match inline {
        MarkdownInline::Math(_) => true,
        MarkdownInline::Emphasis(children)
        | MarkdownInline::Strong(children)
        | MarkdownInline::Strikethrough(children) => contains_inline_math(children),
        MarkdownInline::Link { label, .. } => contains_inline_math(label),
        _ => false,
    })
}

fn text_style_font_size(text_style: &TextStyle) -> gpui::Pixels {
    match text_style.font_size {
        AbsoluteLength::Pixels(size) => size,
        _ => px(14.0),
    }
}

fn text_style_line_height(text_style: &TextStyle, font_size: gpui::Pixels) -> gpui::Pixels {
    match text_style.line_height {
        DefiniteLength::Absolute(AbsoluteLength::Pixels(size)) => size,
        _ => font_size * 1.5,
    }
}

fn cached_inline_runs(
    inlines: &[MarkdownInline],
    base_style: &TextStyle,
    style: &ChatMarkdownStyle<'_>,
    cache: &RefCell<Option<CachedInlineRender>>,
) -> (SharedString, Vec<gpui::TextRun>) {
    let font_size = text_style_font_size(base_style);
    let line_height = text_style_line_height(base_style, font_size);
    let font_size_f32: f32 = font_size.into();
    let line_height_f32: f32 = line_height.into();

    let key = InlineRenderCacheKey {
        font_family: base_style.font_family.clone(),
        font_size_bits: font_size_f32.to_bits(),
        line_height_bits: line_height_f32.to_bits(),
        color: base_style.color,
        font_weight: base_style.font_weight,
        font_style: base_style.font_style,
        underline: base_style.underline.clone(),
        strikethrough: base_style.strikethrough.is_some(),
        inline_code_background: style.inline_code_background,
        inline_code_text_color: style.inline_code_text_color,
        inline_math_background: style.inline_math_background,
        math_text_color: style.math_text_color,
        link_color: style.link_color,
    };

    {
        let cached = cache.borrow();
        if let Some(cached) = cached.as_ref()
            && cached.key == key
        {
            return (cached.text.clone(), cached.runs.clone());
        }
    }

    let (text, runs) = inline_runs(inlines, base_style, style);
    *cache.borrow_mut() = Some(CachedInlineRender {
        key,
        text: text.clone(),
        runs: runs.clone(),
    });
    (text, runs)
}

fn inline_runs(
    inlines: &[MarkdownInline],
    base_style: &TextStyle,
    style: &ChatMarkdownStyle<'_>,
) -> (SharedString, Vec<gpui::TextRun>) {
    let mut text = String::new();
    let mut runs = Vec::new();
    append_inline_runs(inlines, base_style.clone(), style, &mut text, &mut runs);
    if text.is_empty() {
        text.push('\u{200B}');
        runs.push(base_style.to_run(text.len()));
    }
    (text.into(), runs)
}

fn append_inline_runs(
    inlines: &[MarkdownInline],
    current_style: TextStyle,
    style: &ChatMarkdownStyle<'_>,
    text: &mut String,
    runs: &mut Vec<gpui::TextRun>,
) {
    for inline in inlines {
        match inline {
            MarkdownInline::Text(value) => push_run(text, runs, &current_style, value),
            MarkdownInline::Code(value) => {
                let mut code_style = current_style.clone();
                code_style.font_family = style.theme.mono_font_family.clone();
                code_style.background_color = Some(style.inline_code_background);
                code_style.font_weight = FontWeight::MEDIUM;
                code_style.color = style.inline_code_text_color;
                push_run(text, runs, &code_style, value);
            }
            MarkdownInline::Math(value) => {
                let mut math_style = current_style.clone();
                math_style.font_family = style.theme.mono_font_family.clone();
                math_style.background_color = Some(style.inline_math_background);
                math_style.font_style = FontStyle::Italic;
                math_style.font_weight = FontWeight::MEDIUM;
                math_style.color = style.math_text_color;
                push_run(text, runs, &math_style, value);
            }
            MarkdownInline::Emphasis(children) => {
                let mut emphasis = current_style.clone();
                emphasis.font_style = FontStyle::Italic;
                append_inline_runs(children, emphasis, style, text, runs);
            }
            MarkdownInline::Strong(children) => {
                let mut strong = current_style.clone();
                strong.font_weight = FontWeight::SEMIBOLD;
                append_inline_runs(children, strong, style, text, runs);
            }
            MarkdownInline::Strikethrough(children) => {
                let mut struck = current_style.clone();
                struck.strikethrough = Some(gpui::StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(current_style.color.opacity(0.55)),
                    ..Default::default()
                });
                append_inline_runs(children, struck, style, text, runs);
            }
            MarkdownInline::Link { label, .. } => {
                let mut link_style = current_style.clone();
                link_style.color = style.link_color;
                link_style.underline = Some(UnderlineStyle {
                    color: Some(style.link_color.opacity(0.48)),
                    thickness: px(1.0),
                    wavy: false,
                });
                append_inline_runs(label, link_style, style, text, runs);
            }
            MarkdownInline::SoftBreak => push_run(text, runs, &current_style, " "),
            MarkdownInline::LineBreak => push_run(text, runs, &current_style, "\n"),
        }
    }
}

fn push_run(text: &mut String, runs: &mut Vec<gpui::TextRun>, style: &TextStyle, content: &str) {
    if content.is_empty() {
        return;
    }

    text.push_str(content);
    runs.push(style.to_run(content.len()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_inline_code_and_lists() {
        let blocks = parse_markdown("- one `code`\n- two");
        assert!(matches!(blocks.first(), Some(MarkdownBlock::List { .. })));
    }

    #[test]
    fn preserves_heading_and_quote_blocks() {
        let blocks = parse_markdown("# Title\n\n> quoted");
        assert!(matches!(
            blocks.first(),
            Some(MarkdownBlock::Heading { .. })
        ));
        assert!(matches!(blocks.get(1), Some(MarkdownBlock::BlockQuote(_))));
    }

    #[test]
    fn soft_breaks_do_not_become_hard_breaks() {
        let blocks =
            parse_markdown("Root filesystem\n`/`: `340G used / 559G avail` out of `937G` total");
        let MarkdownBlock::Paragraph { inlines, .. } = &blocks[0] else {
            panic!("expected paragraph");
        };

        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, MarkdownInline::SoftBreak))
        );
        assert!(
            !inlines
                .iter()
                .any(|inline| matches!(inline, MarkdownInline::LineBreak))
        );
    }

    #[test]
    fn parses_tables_from_mdast() {
        let blocks = parse_markdown("| Name | Value |\n| --- | ---: |\n| foo | `bar` |");
        let MarkdownBlock::Table { aligns, rows, .. } = &blocks[0] else {
            panic!("expected table");
        };

        assert_eq!(aligns.len(), 2);
        assert_eq!(rows.len(), 2);
        assert!(matches!(aligns[1], MarkdownTableAlign::Right));
    }

    #[test]
    fn parses_mermaid_fences_as_diagram_blocks() {
        let blocks = parse_markdown("```mermaid 140\ngraph TD\n  A-->B\n```");
        let Some(MarkdownBlock::Mermaid { code, scale }) = blocks.first() else {
            panic!("expected mermaid block");
        };

        assert_eq!(*scale, 140);
        assert!(code.contains("A-->B"));
    }

    #[test]
    fn parses_markdown_math_as_first_class_content() {
        let blocks = parse_markdown("Inline $a^2 + b^2 = c^2$ math.\n\n$$\ne^{i\\pi}+1=0\n$$");
        let MarkdownBlock::Paragraph { inlines, .. } = &blocks[0] else {
            panic!("expected paragraph");
        };

        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, MarkdownInline::Math(math) if math.contains("a^2")))
        );
        assert!(matches!(
            blocks.get(1),
            Some(MarkdownBlock::MathBlock { math, .. }) if math.contains("e^{i\\pi}")
        ));
    }

    #[test]
    fn parses_euler_formula_as_inline_math() {
        let blocks = parse_markdown("行内公式示例: $e^{i\\pi}+1=0$");
        let MarkdownBlock::Paragraph { inlines, .. } = &blocks[0] else {
            panic!("expected paragraph");
        };

        assert!(contains_inline_math(inlines));
        assert!(
            inlines.iter().any(
                |inline| matches!(inline, MarkdownInline::Math(math) if math == "e^{i\\pi}+1=0")
            )
        );
    }

    #[test]
    fn parses_nested_mermaid_and_math_blocks() {
        let blocks =
            parse_markdown("> ```mermaid\n> flowchart TD\n>   A-->B\n> ```\n\n- $$\n  x^2\n  $$");
        let Some(MarkdownBlock::BlockQuote(quote_blocks)) = blocks.first() else {
            panic!("expected blockquote");
        };
        assert!(matches!(
            quote_blocks.first(),
            Some(MarkdownBlock::Mermaid { code, .. }) if code.contains("A-->B")
        ));

        let Some(MarkdownBlock::List { items, .. }) = blocks.get(1) else {
            panic!("expected list");
        };
        assert!(matches!(
            items.first().and_then(|item| item.first()),
            Some(MarkdownBlock::MathBlock { math, .. }) if math.contains("x^2")
        ));
    }

    #[test]
    fn dollar_amounts_do_not_become_inline_math() {
        let blocks = parse_markdown("Costs are $5 and $6 today, but `$x$` stays code.");
        let MarkdownBlock::Paragraph { inlines, .. } = &blocks[0] else {
            panic!("expected paragraph");
        };

        assert!(
            !inlines
                .iter()
                .any(|inline| matches!(inline, MarkdownInline::Math(_)))
        );
        assert!(inlines.iter().any(|inline| {
            matches!(inline, MarkdownInline::Text(text) if text.contains("$5 and $6"))
        }));
    }

    #[test]
    fn hyphenated_dollar_text_does_not_become_inline_math() {
        for source in [
            "This is $end-to-end$ tested.",
            "The $X-ray$ scan passed.",
            "Costs are $3-5$ today.",
        ] {
            let blocks = parse_markdown(source);
            let MarkdownBlock::Paragraph { inlines, .. } = &blocks[0] else {
                panic!("expected paragraph");
            };

            assert!(
                !inlines
                    .iter()
                    .any(|inline| matches!(inline, MarkdownInline::Math(_))),
                "{source}"
            );
        }
    }

    #[test]
    fn spaced_minus_inline_math_is_supported() {
        let blocks = parse_markdown("Use $x - y$ for the delta.");
        let MarkdownBlock::Paragraph { inlines, .. } = &blocks[0] else {
            panic!("expected paragraph");
        };

        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, MarkdownInline::Math(math) if math == "x - y"))
        );
    }

    #[test]
    fn single_letter_inline_math_is_supported() {
        let blocks = parse_markdown("Let $x$ be the selected pane.");
        let MarkdownBlock::Paragraph { inlines, .. } = &blocks[0] else {
            panic!("expected paragraph");
        };

        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, MarkdownInline::Math(math) if math == "x"))
        );
    }

    #[test]
    fn identifier_inline_math_is_supported() {
        let blocks = parse_markdown("Use $theta$ and $velocity$ in the formula.");
        let MarkdownBlock::Paragraph { inlines, .. } = &blocks[0] else {
            panic!("expected paragraph");
        };

        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, MarkdownInline::Math(math) if math == "theta"))
        );
        assert!(
            inlines
                .iter()
                .any(|inline| matches!(inline, MarkdownInline::Math(math) if math == "velocity"))
        );
    }

    #[test]
    fn rich_svg_renderers_produce_svg() {
        let mermaid = mermaid_rs_renderer::render("flowchart TD\n  A-->B").unwrap();
        assert!(mermaid.contains("<svg"));

        let styled_dark_source = mermaid_source_for_theme(
            "flowchart TD\n  A{ i < n? }\n  style A fill:#e1f5fe,stroke:#03a9f4\n",
            RichSvgThemeMode::Dark,
        );
        assert!(styled_dark_source.contains("color:#0F172A"));
        let crlf_styled_dark_source = mermaid_source_for_theme(
            "flowchart TD\r\n  A{ i < n? }\r\n  style A fill:#e1f5fe,stroke:#03a9f4\r\n",
            RichSvgThemeMode::Dark,
        );
        assert!(
            crlf_styled_dark_source.contains("style A fill:#e1f5fe,stroke:#03a9f4,color:#0F172A\n")
        );
        assert!(!crlf_styled_dark_source.contains("\r,color:"));

        let dark_mermaid = mermaid_rs_renderer::render_with_options(
            styled_dark_source.as_ref(),
            mermaid_render_options(RichSvgThemeMode::Dark),
        )
        .unwrap();
        assert!(dark_mermaid.contains("<svg"));
        assert!(dark_mermaid.contains("#0B1120") || dark_mermaid.contains("#0b1120"));
        assert!(dark_mermaid.contains("#0F172A") || dark_mermaid.contains("#0f172a"));

        let math = mathjax_svg_rs::render_tex(
            r"e^{i\pi}+1=0",
            &mathjax_svg_rs::Options {
                font_size: 18.0,
                horizontal_align: mathjax_svg_rs::HorizontalAlign::Center,
            },
        )
        .unwrap();
        assert!(math.contains("<svg"));

        let dark_math = math_svg_for_theme(math, RichSvgThemeMode::Dark);
        assert!(dark_math.contains("color=\"#F8FAFC\""));
        assert!(dark_math.contains("fill=\"#F8FAFC\""));
    }

    #[test]
    fn rich_svg_render_ids_include_nested_block_path() {
        let key = RichSvgRenderKey {
            kind: RichSvgRenderKind::Mermaid,
            source: "flowchart TD\n  A-->B".into(),
            metric: 100,
            theme_mode: RichSvgThemeMode::Light,
            color: None,
        };

        let first = rich_svg_render_id(&[0, 0, 0], &key);
        let second = rich_svg_render_id(&[1, 0, 0], &key);
        assert_ne!(first, second);
    }

    #[test]
    fn code_block_copy_ids_include_nested_block_path() {
        let code = "brew services start lizardbyte/homebrew/sunshine";

        let first = code_block_copy_id(&[0, 0, 1], code);
        let second = code_block_copy_id(&[0, 1, 1], code);

        assert_ne!(first, second);
        assert!(first.contains("0.0.1"));
        assert!(second.contains("0.1.1"));
    }

    #[test]
    fn highlighted_code_runs_cover_empty_lines() {
        let theme = Theme::default();
        let style = ChatMarkdownStyle::new(&theme, ChatMarkdownTone::Message);
        let code = "print(1)\n\nprint(2)";
        let (text, runs) = highlighted_code_runs(code, &Some("python".into()), &style);

        assert_eq!(text.as_ref(), code);
        assert!(!runs.is_empty());
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), code.len());
    }

    #[test]
    fn highlighted_code_runs_keep_mono_font() {
        let mut theme = Theme::default();
        theme.mono_font_family = "IoskeleyMono".into();
        let style = ChatMarkdownStyle::new(&theme, ChatMarkdownTone::Message);
        let (_, runs) = highlighted_code_runs("let value = 1;", &Some("rust".into()), &style);

        assert!(!runs.is_empty());
        assert!(
            runs.iter()
                .all(|run| run.font.family.as_ref() == "IoskeleyMono")
        );
    }

    #[test]
    fn generic_code_fence_infers_bash_for_shell_prompt() {
        let code = "$ amp --version\n0.0.1";
        let language = Some("code".to_string());

        assert_eq!(
            effective_highlighter_language(&language, code).as_deref(),
            Some("bash")
        );
        assert_eq!(code_block_header_label(&language, code), "bash");
    }

    #[test]
    fn generic_code_fence_infers_bash_for_compact_shell_prompt() {
        let code = "$amp --version\n0.0.1";
        let language = Some("code".to_string());

        assert_eq!(
            effective_highlighter_language(&language, code).as_deref(),
            Some("bash")
        );
        assert_eq!(code_block_header_label(&language, code), "bash");
    }

    #[test]
    fn plain_text_fence_does_not_infer_shell_prompt() {
        let code = "$ amp --version\n0.0.1";
        let language = Some("text".to_string());

        assert_eq!(effective_highlighter_language(&language, code), None);
        assert_eq!(code_block_header_label(&language, code), "text");
    }
}
