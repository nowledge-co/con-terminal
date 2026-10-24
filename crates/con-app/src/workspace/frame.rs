use gpui::{Bounds, Hsla, InteractiveElement, IntoElement, Pixels, Styled, Window, point, px};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// A split includes its 1pt divider; exterior canvas + pane padding must match it.
pub(crate) const CARD_GAP: f32 = 5.0;
pub(crate) const PANE_INSET: f32 = (CARD_GAP - 1.0) * 0.5;
pub(crate) const CANVAS_INSET: f32 = CARD_GAP - PANE_INSET;
pub(crate) const AGENT_CONTENT_INSET: f32 = 2.0;
// Keep the resize target wider than its visible gutter without shifting its center.
pub(crate) const PANE_RESIZE_HIT_WIDTH: f32 = 7.0;
pub(crate) const PANE_RADIUS: f32 = 8.0;
pub(crate) const CARD_OUTER_INSET: f32 = CANVAS_INSET + PANE_INSET;
pub(super) const INPUT_BOTTOM_INSET: f32 = CARD_GAP - CANVAS_INSET;
pub(super) const INPUT_RADIUS: f32 = PANE_RADIUS;

/// The compact canvas uses a negative top margin to reclaim caption clearance.
/// Its translucent gutters must not repaint the caption underneath that margin.
pub(super) fn canvas_viewport() -> gpui::Div {
    gpui::div()
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_hidden()
}

pub(super) fn canvas_depth_for_layout(
    horizontal_tabs: bool,
    split_panes: bool,
    left_panel: f32,
    agent_panel: f32,
) -> f32 {
    if horizontal_tabs || split_panes {
        1.0
    } else {
        left_panel.max(agent_panel)
    }
}

/// `caption` is the global caption row's presence. Without tabs its controls
/// sit midway between the window top and the card top, so the row's lower half
/// is the card's top clearance. Tabs keep the lateral 5pt gap below their selected surface.
pub(super) fn canvas_top_inset(depth: f32, horizontal_tabs: f32, caption: f32) -> f32 {
    let tabs = horizontal_tabs.clamp(0.0, 1.0);
    let tab_bottom_clearance = (super::TOP_BAR_TABS_HEIGHT - super::TOP_BAR_TAB_HEIGHT) * 0.5
        + super::TOP_BAR_TAB_SELECTION_INSET;
    let caption_clearance =
        (CANVAS_INSET + PANE_INSET) * (1.0 - tabs) + tab_bottom_clearance * tabs;
    (CANVAS_INSET - caption_clearance * caption.clamp(0.0, 1.0)) * depth
}

/// Allow the pane's rounded top to enter the caption's unused bottom clearance.
/// Content retains its original layout origin; only the viewport clip expands.
pub(super) fn canvas_top_bleed(depth: f32, tabs: f32, caption: f32) -> f32 {
    -(canvas_top_inset(depth, tabs, caption) + PANE_INSET * depth).min(0.0)
}

pub(crate) fn pane_card_geometry(split: bool, depth: f32) -> (f32, f32) {
    let depth = if split { 1.0 } else { depth.clamp(0.0, 1.0) };
    (PANE_INSET * depth, PANE_RADIUS * depth)
}

pub(super) fn agent_vertical_insets(
    depth: f32,
    split: bool,
    input_reserved_height: f32,
    horizontal_tabs: f32,
    caption: f32,
) -> (f32, f32) {
    let pane_inset = pane_card_geometry(split, depth).0;
    let top = canvas_top_inset(depth, horizontal_tabs, caption) + pane_inset;
    (
        top,
        CANVAS_INSET * depth + pane_inset + input_reserved_height,
    )
}

pub(crate) fn card_edge(radius: f32, foreground: Hsla, dark: bool) -> gpui::Div {
    edge(radius, card_edge_color(foreground, dark), false, true)
}

fn card_edge_color(foreground: Hsla, dark: bool) -> Hsla {
    foreground.opacity(if dark { 0.12 } else { 0.075 })
}

/// GPUI cards do not need the native terminal's complementary corner cutouts.
/// Draw fill and edge in one quad so their antialiasing shares one contour.
pub(crate) fn card_material(
    radius: f32,
    color: Hsla,
    foreground: Hsla,
    dark: bool,
) -> impl IntoElement {
    gpui::canvas(
        |bounds, window, _| {
            crate::terminal_geometry::snap_terminal_bounds(bounds, window.scale_factor())
        },
        move |_, bounds, window, _| {
            let radii = window_concentric_corners(bounds, radius, window);
            window.paint_quad(card_material_quad(
                bounds,
                radii,
                color,
                card_edge_color(foreground, dark),
                window.scale_factor(),
            ));
        },
    )
    .absolute()
    .size_full()
    .top_0()
    .left_0()
}

fn card_material_quad(
    bounds: Bounds<Pixels>,
    radii: gpui::Corners<Pixels>,
    color: Hsla,
    edge: Hsla,
    scale: f32,
) -> gpui::PaintQuad {
    let mut quad = gpui::fill(bounds, color);
    quad.corner_radii = radii;
    if radii != gpui::Corners::default() {
        quad.border_widths = gpui::Edges::all(px(1.0 / scale));
        quad.border_color = edge;
    }
    quad
}

/// Workspace card fill whose corners follow [`window_concentric_corners`].
pub(crate) fn card_fill(radius: f32, color: Hsla) -> impl IntoElement {
    gpui::canvas(
        |bounds, window, _| {
            crate::terminal_geometry::snap_terminal_bounds(bounds, window.scale_factor())
        },
        move |_, bounds, window, _| {
            let mut quad = gpui::fill(bounds, color);
            quad.corner_radii = window_concentric_corners(bounds, radius, window);
            window.paint_quad(quad);
        },
    )
    .absolute()
    .size_full()
    .top_0()
    .left_0()
}

/// A card corner that meets the window's rounded corner shares its centre:
/// its radius is the window radius less the card's clearance from both edges.
pub(crate) fn window_concentric_corners(
    card: Bounds<Pixels>,
    radius: f32,
    window: &Window,
) -> gpui::Corners<Pixels> {
    concentric_corners(
        card,
        window.viewport_size(),
        window_corner_radius(window),
        radius,
    )
}

fn concentric_corners(
    card: Bounds<Pixels>,
    viewport: gpui::Size<Pixels>,
    window_radius: f32,
    radius: f32,
) -> gpui::Corners<Pixels> {
    let limit = card.size.width.min(card.size.height).as_f32().max(0.0) * 0.5;
    let radius = radius.max(0.0).min(limit);
    if radius <= 0.0 || window_radius <= 0.0 {
        return gpui::Corners::all(px(radius));
    }
    let left = card.left().as_f32();
    let top = card.top().as_f32();
    let right = (viewport.width - card.right()).as_f32();
    let bottom = (viewport.height - card.bottom()).as_f32();
    let corner = |x: f32, y: f32| px(radius.max(window_radius - x.max(y).max(0.0)).min(limit));
    gpui::Corners {
        top_left: corner(left, top),
        top_right: corner(right, top),
        bottom_right: corner(right, bottom),
        bottom_left: corner(left, bottom),
    }
}

fn window_corner_radius(window: &Window) -> f32 {
    use raw_window_handle::HasWindowHandle;
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return 0.0;
    };
    let raw_window_handle::RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return 0.0;
    };
    unsafe { con_window_corner_radius(handle.ns_view.as_ptr()) as f32 }
}

/// Shared fine edge with a short, diagonal contact shadow. Never shade native content.
pub(crate) fn surface_edge(radius: f32, color: Hsla, raised: bool) -> gpui::Div {
    edge(radius, color, raised, false)
}

fn edge(radius: f32, color: Hsla, raised: bool, concentric: bool) -> gpui::Div {
    use gpui::ParentElement;
    gpui::div().absolute().child(
        gpui::canvas(
            |bounds, window, _| {
                crate::terminal_geometry::snap_terminal_bounds(bounds, window.scale_factor())
            },
            move |_, bounds, window, _| {
                let width = px(1.0 / window.scale_factor());
                let radii = if concentric {
                    window_concentric_corners(bounds, radius, window)
                } else {
                    gpui::Corners::all(px(radius))
                };
                if raised {
                    // Nested exterior contours share the surface's exact inner
                    // curve. Rectangular shadow cutoffs leave discontinuities
                    // where a rounded corner meets a straight edge.
                    for (step, falloff) in [(1.0, 0.34), (2.0, 0.14), (3.0, 0.04)] {
                        let outer = bounds.dilate(width * step);
                        let outer_radii = radii.map(|radius| *radius + width * step);
                        for (strip, direction) in contact_regions(outer, outer_radii, width) {
                            window.with_content_mask(
                                Some(gpui::ContentMask { bounds: strip }),
                                |window| {
                                    let mut shade = gpui::fill(outer, gpui::transparent_black());
                                    shade.corner_radii = outer_radii;
                                    shade.border_widths = gpui::Edges::all(width);
                                    shade.border_color =
                                        gpui::black().opacity(color.a * falloff * direction);
                                    window.paint_quad(shade);
                                },
                            );
                        }
                    }
                }
                // Clip before rasterization: a transparent full-window quad
                // would still shade its interior on every frame.
                for strip in outline_strips(bounds, radii, width) {
                    window.with_content_mask(Some(gpui::ContentMask { bounds: strip }), |window| {
                        let mut edge = gpui::fill(bounds, color.opacity(0.0));
                        edge.corner_radii = radii;
                        edge.border_widths = gpui::Edges::all(width);
                        edge.border_color = color;
                        window.paint_quad(edge);
                    });
                }
            },
        )
        .absolute()
        .size_full(),
    )
}

fn contact_regions(
    bounds: Bounds<Pixels>,
    radii: gpui::Corners<Pixels>,
    width: Pixels,
) -> [(Bounds<Pixels>, f32); 8] {
    let radii = radii.map(|radius| {
        (*radius)
            .max(width)
            .min(bounds.size.width * 0.5)
            .min(bounds.size.height * 0.5)
    });
    let gpui::Corners {
        top_left: tl,
        top_right: tr,
        bottom_right: br,
        bottom_left: bl,
    } = radii;
    let rect = |x, y, w: Pixels, h: Pixels| {
        Bounds::new(point(x, y), gpui::size(w.max(px(0.0)), h.max(px(0.0))))
    };
    [
        (rect(bounds.left(), bounds.top(), tl, tl), 0.12),
        (rect(bounds.right() - tr, bounds.top(), tr, tr), 0.55),
        (rect(bounds.left(), bounds.bottom() - bl, bl, bl), 0.55),
        (rect(bounds.right() - br, bounds.bottom() - br, br, br), 1.0),
        (
            rect(
                bounds.left() + tl,
                bounds.top(),
                bounds.size.width - tl - tr,
                width,
            ),
            0.12,
        ),
        (
            rect(
                bounds.left() + bl,
                bounds.bottom() - width,
                bounds.size.width - bl - br,
                width,
            ),
            0.8,
        ),
        (
            rect(
                bounds.left(),
                bounds.top() + tl,
                width,
                bounds.size.height - tl - bl,
            ),
            0.12,
        ),
        (
            rect(
                bounds.right() - width,
                bounds.top() + tr,
                width,
                bounds.size.height - tr - br,
            ),
            0.8,
        ),
    ]
}

fn outline_strips(
    bounds: Bounds<Pixels>,
    radii: gpui::Corners<Pixels>,
    width: Pixels,
) -> [Bounds<Pixels>; 4] {
    let cap = |a: Pixels, b: Pixels| a.max(b).max(width).min(bounds.size.height * 0.5);
    let top = cap(radii.top_left, radii.top_right);
    let bottom = cap(radii.bottom_left, radii.bottom_right);
    let side = width.min(bounds.size.width * 0.5);
    let middle = (bounds.size.height - top - bottom).max(px(0.0));
    [
        Bounds::new(bounds.origin, gpui::size(bounds.size.width, top)),
        Bounds::new(
            point(bounds.left(), bounds.bottom() - bottom),
            gpui::size(bounds.size.width, bottom),
        ),
        Bounds::new(
            point(bounds.left(), bounds.top() + top),
            gpui::size(side, middle),
        ),
        Bounds::new(
            point(bounds.right() - side, bounds.top() + top),
            gpui::size(side, middle),
        ),
    ]
}

/// Align the input tray and its popovers with the pane cards, not the window
/// centre. Include both the outer canvas and inner pane inset.
pub(super) fn input_island_geometry(width: f32, depth: f32) -> (f32, f32) {
    let inset = (CARD_OUTER_INSET * depth).min(width * 0.5);
    (inset, (width - 2.0 * inset).max(0.0))
}

pub(super) fn input_safe_height(height: f32, progress: f32, depth: f32) -> f32 {
    (height + (CARD_GAP - PANE_INSET + INPUT_BOTTOM_INSET) * depth) * progress.clamp(0.0, 1.0)
}

pub(super) fn input_reserved_height(height: f32, _visible: bool, progress: f32, depth: f32) -> f32 {
    // Geometry follows the same sample as the reveal. Holding the final height
    // until fade-out completes makes the native terminal expand one beat late.
    input_safe_height(height, progress, depth)
}

pub(super) fn caption_width(window: f32, sidebar: f32, dock: f32, leading: f32) -> f32 {
    if dock > 0.0 && sidebar >= caption_minimum_width(leading) {
        sidebar.min(window)
    } else {
        window
    }
}

pub(super) fn caption_minimum_width(leading: f32) -> f32 {
    leading + 3.0 * 22.0 + 24.0
}

pub(super) fn input_reveal_clip(height: f32, progress: f32, depth: f32) -> gpui::Div {
    let progress = progress.clamp(0.0, 1.0);
    gpui::div()
        .absolute()
        .bottom(px(INPUT_BOTTOM_INSET * depth * progress))
        .left_0()
        .w_full()
        .h(px(height * progress))
        .overflow_hidden()
}

#[derive(Clone, Copy, Default)]
struct InputTrayBounds {
    card: Option<Bounds<Pixels>>,
    reveal: Option<Bounds<Pixels>>,
}

impl InputTrayBounds {
    fn surround_regions(self, bounds: Bounds<Pixels>) -> [Bounds<Pixels>; 4] {
        let visible = self
            .card
            .zip(self.reveal)
            .map(|(card, reveal)| card.intersect(&reveal).intersect(&bounds))
            .filter(|visible| visible.size.width > px(0.0) && visible.size.height > px(0.0))
            .unwrap_or_else(|| Bounds::new(bounds.origin, gpui::size(px(0.0), px(0.0))));
        surround_strips(bounds, visible)
    }
}

/// Frame-local geometry shared between the tray and its exterior chrome.
/// All prepaint completes before paint: sample actual snapped layout bounds,
/// then subtract only the *visible* card, not the full card sliding below it.
#[derive(Default)]
pub(super) struct InputTray {
    bounds: Rc<Cell<InputTrayBounds>>,
}

impl InputTray {
    pub fn track_card(&self, card: gpui::Div) -> gpui::Div {
        self.track(card, true)
    }

    pub fn track_reveal(&self, reveal: gpui::Div) -> gpui::Div {
        self.track(reveal, false)
    }

    fn track(&self, element: gpui::Div, card: bool) -> gpui::Div {
        use gpui::ParentElement;
        let geometry = self.bounds.clone();
        element.child(
            gpui::canvas(
                move |bounds, window, _| {
                    let bounds = crate::terminal_geometry::snap_terminal_bounds(
                        bounds,
                        window.scale_factor(),
                    );
                    let mut next = geometry.get();
                    if card {
                        next.card = Some(bounds);
                    } else {
                        next.reveal = Some(bounds);
                    }
                    geometry.set(next);
                },
                |_, _, _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    }

    pub fn surround(&self, color: Hsla) -> impl IntoElement + use<> {
        let geometry = self.bounds.clone();
        gpui::canvas(
            |bounds, window, _| {
                crate::terminal_geometry::snap_terminal_bounds(bounds, window.scale_factor())
            },
            move |_, bounds, window, _| {
                window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                    for strip in geometry.get().surround_regions(bounds) {
                        if strip.size.width > px(0.0) && strip.size.height > px(0.0) {
                            window.paint_quad(gpui::fill(strip, color));
                        }
                    }
                });
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// Use the native terminal's exact coverage field rather than a second rounded
/// quad raster. The material stays at its configured alpha throughout reveal.
pub(super) fn input_card(
    width: f32,
    height: f32,
    depth: f32,
    color: Hsla,
    foreground: Hsla,
    dark: bool,
    surround: Hsla,
) -> gpui::Div {
    use gpui::ParentElement;
    let (left, width) = input_island_geometry(width, depth);
    gpui::div()
        .absolute()
        .top_0()
        .left(px((left - CANVAS_INSET * depth).max(0.0)))
        .w(px(width))
        .h(px(height))
        .overflow_hidden()
        .child(input_material(INPUT_RADIUS * depth, color, surround))
        .child(
            card_edge(INPUT_RADIUS * depth, foreground, dark)
                .inset_0()
                .debug_selector(|| "input-card-edge".into()),
        )
}

fn material_body_strips(bounds: Bounds<Pixels>, corners: &[Bounds<Pixels>]) -> Vec<Bounds<Pixels>> {
    let mut strips = vec![bounds];
    for corner in corners {
        strips = strips
            .into_iter()
            .flat_map(|strip| {
                if strip.intersects(corner) {
                    surround_strips(strip, strip.intersect(corner))
                        .into_iter()
                        .filter(|strip| strip.size.width > px(0.0) && strip.size.height > px(0.0))
                        .collect::<Vec<_>>()
                } else {
                    vec![strip]
                }
            })
            .collect();
    }
    strips
}

fn input_corner_image(
    tile: &NativeCornerCoverage,
    color: Hsla,
    chrome_alpha: f32,
) -> std::sync::Arc<gpui::RenderImage> {
    type Key = (usize, usize, u32, [u32; 4]);
    type Cache = Vec<(Key, std::sync::Arc<gpui::RenderImage>)>;
    thread_local! {
        static CACHE: RefCell<Cache> = const { RefCell::new(Vec::new()) };
    }
    let color: gpui::Rgba = color.into();
    let key = (
        tile.side,
        tile.corner,
        chrome_alpha.to_bits(),
        [
            color.r.to_bits(),
            color.g.to_bits(),
            color.b.to_bits(),
            color.a.to_bits(),
        ],
    );
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, image)) = cache.iter().find(|(old, _)| *old == key) {
            return image.clone();
        }
        // RenderImage consumes straight-alpha BGRA. Match the native mask's
        // physical pixels, including its compensation for translucent chrome.
        let channel = |value: f32| (value * 255.0).round().clamp(0.0, 255.0) as u8;
        let mut bytes = vec![0; tile.side * tile.side * 4];
        for (pixel, coverage) in bytes.chunks_exact_mut(4).zip(tile.rgba.chunks_exact(4)) {
            pixel.copy_from_slice(&[
                channel(color.b),
                channel(color.g),
                channel(color.r),
                (f32::from(coverage[3]) * color.a).round().clamp(0.0, 255.0) as u8,
            ]);
        }
        let pixels = image::RgbaImage::from_raw(tile.side as u32, tile.side as u32, bytes).unwrap();
        let image = std::sync::Arc::new(gpui::RenderImage::new(vec![image::Frame::new(pixels)]));
        if cache.len() >= 32 {
            cache.remove(0);
        }
        cache.push((key, image.clone()));
        image
    })
}

fn input_material(radius: f32, color: Hsla, surround: Hsla) -> impl IntoElement {
    gpui::canvas(
        move |bounds, window, _| {
            let bounds =
                crate::terminal_geometry::snap_terminal_bounds(bounds, window.scale_factor());
            let radii = window_concentric_corners(bounds, radius, window);
            let coverage = native_corner_coverage(bounds, radii, window.scale_factor(), surround.a)
                .unwrap_or_else(|error| {
                    log::error!("failed to prepare input corner coverage: {error}");
                    Vec::new()
                });
            let corners = coverage.iter().map(|tile| tile.bounds).collect::<Vec<_>>();
            let images = coverage
                .iter()
                .map(|tile| (tile.bounds, input_corner_image(tile, color, surround.a)))
                .collect::<Vec<_>>();
            (
                material_body_strips(bounds, &corners),
                images,
                corner_cutouts(bounds, radii),
            )
        },
        move |_, (strips, images, corners), window, cx| {
            for strip in strips {
                window.paint_quad(gpui::fill(strip, color));
            }
            for (bounds, image) in images {
                if let Err(error) =
                    window.paint_image(bounds, bounds, Default::default(), image, 0, false)
                {
                    log::error!("failed to paint input corner: {error}");
                }
            }
            for corner in corners {
                corner.paint(surround, window, cx);
            }
        },
    )
    .absolute()
    .size_full()
}

#[derive(Default)]
pub(super) struct CanvasSurround {
    cache: Rc<RefCell<Option<SurroundGeometry>>>,
}

/// Small, cached cutouts only. A full-size fill would make terminal glass
/// opaque, and a viewport-sized path with a hole still pays a large Metal blit.
#[derive(Clone, Default)]
pub(crate) struct PaneSurround {
    cache: Rc<RefCell<Option<SurroundGeometry>>>,
}

impl PaneSurround {
    pub fn render(&self, color: Hsla, depth: f32) -> impl IntoElement + use<> {
        let cache = self.cache.clone();
        gpui::canvas(
            move |bounds, window, _| {
                let scale = window.scale_factor();
                let bounds = crate::terminal_geometry::snap_terminal_bounds(bounds, scale);
                let inset = px(PANE_INSET * depth)
                    .min(bounds.size.width * 0.5)
                    .min(bounds.size.height * 0.5);
                let mut cache = cache.borrow_mut();
                let card = bounds.inset(inset);
                let radii = window_concentric_corners(card, PANE_RADIUS * depth, window);
                if cache.as_ref().is_none_or(|old| {
                    old.bounds != bounds
                        || old.scale != scale
                        || old.depth != depth
                        || old.radii != radii
                }) {
                    *cache = Some(SurroundGeometry {
                        bounds,
                        depth,
                        scale,
                        radii,
                        corners: corner_cutouts(card, radii),
                    });
                }
                (
                    surround_strips(bounds, bounds.inset(inset)),
                    cache.as_ref().unwrap().corners.clone(),
                )
            },
            move |_, (strips, corners), window, cx| {
                for strip in strips {
                    window.paint_quad(gpui::fill(strip, color));
                }
                for corner in corners {
                    corner.paint(color, window, cx);
                }
            },
        )
        .absolute()
        .size_full()
        .top_0()
        .left_0()
    }
}

struct SurroundGeometry {
    bounds: Bounds<Pixels>,
    depth: f32,
    scale: f32,
    radii: gpui::Corners<Pixels>,
    corners: Vec<CornerCutout>,
}

impl CanvasSurround {
    pub fn render(
        &self,
        color: Hsla,
        depth: f32,
        horizontal_tabs: f32,
        caption: f32,
        safe_height: f32,
        glass: Hsla,
    ) -> impl IntoElement + use<> {
        let cache = self.cache.clone();
        gpui::canvas(
            move |bounds, window, _| {
                let scale = window.scale_factor();
                let bounds = crate::terminal_geometry::snap_terminal_bounds(bounds, scale);
                let inner = canvas_inner_bounds(bounds, depth, horizontal_tabs, caption);
                log::trace!(target: "con::canvas", "prepaint bounds={bounds:?} depth={depth} safe_height={safe_height}");
                // Leaves own rounded corners. The shared viewport also covers
                // the composer reservation and must not cut a second curve.
                prepare_native_canvas(inner, 0.0, window);
                prepare_native_safe_area(safe_height, glass, window);
                let mut cache = cache.borrow_mut();
                if cache.as_ref().is_none_or(|old| {
                    old.bounds != bounds || old.depth != depth || old.scale != scale
                }) {
                    *cache = Some(SurroundGeometry {
                        bounds,
                        depth,
                        scale,
                        radii: gpui::Corners::default(),
                        corners: Vec::new(),
                    });
                }
                (
                    surround_strips(bounds, inner),
                    cache.as_ref().unwrap().corners.clone(),
                )
            },
            move |bounds, (strips, corners), window, cx| {
                log::trace!(target: "con::canvas", "paint bounds={bounds:?} depth={depth} safe_height={safe_height}");
                for strip in strips {
                    if strip.size.width > px(0.0) && strip.size.height > px(0.0) {
                        window.paint_quad(gpui::fill(strip, color));
                    }
                }
                for corner in corners {
                    corner.paint(color, window, cx);
                }
            },
        )
        .absolute()
        .size_full()
    }
}

// Normalized circular cutout, not a UI icon. GPUI's SVG atlas rasterizes this
// above device resolution and reuses the alpha masks across positions/themes.
// Keep the cubic identical to the native pane clip; do not offset the curve.
const CORNER_MASK: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1" viewBox="0 0 1 1"><path fill="white" d="M0 0H1C0.4477152 0 0 0.4477152 0 1Z"/></svg>"#;

#[derive(Clone)]
struct CornerCutout {
    bounds: Bounds<Pixels>,
    mask: usize,
}

impl CornerCutout {
    fn presentation(&self, scale: f32) -> Option<(Bounds<Pixels>, gpui::TransformationMatrix)> {
        let bounds = crate::terminal_geometry::snap_terminal_bounds(self.bounds, scale);
        if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
            return None;
        }
        // Reflect one atlas entry about its snapped physical centre. Rasterizing
        // four flipped curves separately need not produce identical coverage.
        let x = if matches!(self.mask, 1 | 2) {
            -1.0
        } else {
            1.0
        };
        let y = if matches!(self.mask, 2 | 3) {
            -1.0
        } else {
            1.0
        };
        Some((
            bounds,
            gpui::TransformationMatrix {
                rotation_scale: [[x, 0.0], [0.0, y]],
                translation: [
                    bounds.center().x.as_f32() * scale * (1.0 - x),
                    bounds.center().y.as_f32() * scale * (1.0 - y),
                ],
            },
        ))
    }

    fn paint(&self, color: Hsla, window: &mut Window, cx: &gpui::App) {
        let Some((bounds, reflection)) = self.presentation(window.scale_factor()) else {
            return;
        };
        if let Err(error) = window.paint_svg(
            bounds,
            "con/circular-corner-mask-v1".into(),
            Some(CORNER_MASK),
            reflection,
            color,
            cx,
        ) {
            log::error!("failed to paint pane corner: {error}");
        }
    }
}

/// A physical-pixel corner tile. Native and overlay coverage must come from
/// the same raster, rather than independently antialiasing the same curve.
pub(crate) struct NativeCornerCoverage {
    pub bounds: Bounds<Pixels>,
    pub side: usize,
    corner: usize,
    pub rgba: std::sync::Arc<[u8]>,
}

pub(crate) fn native_corner_coverage(
    bounds: Bounds<Pixels>,
    radii: gpui::Corners<Pixels>,
    scale: f32,
    chrome_alpha: f32,
) -> anyhow::Result<Vec<NativeCornerCoverage>> {
    anyhow::ensure!(
        scale.is_finite() && scale > 0.0 && chrome_alpha.is_finite(),
        "invalid corner material scale or alpha"
    );
    type Cache = Vec<((usize, u32, usize), std::sync::Arc<[u8]>)>;
    thread_local! {
        static CACHE: RefCell<Cache> = const { RefCell::new(Vec::new()) };
    }
    let alpha = chrome_alpha.clamp(0.0, 1.0);
    corner_cutouts(bounds, radii)
        .into_iter()
        .map(|corner| {
            let (bounds, _) = corner
                .presentation(scale)
                .ok_or_else(|| anyhow::anyhow!("empty corner"))?;
            let side = (bounds.size.width.as_f32() * scale).round() as usize;
            anyhow::ensure!(side > 0 && side <= 256, "corner tile exceeds raster budget");
            let key = (side, alpha.to_bits(), corner.mask);
            let cached = CACHE.with(|cache| {
                cache
                    .borrow()
                    .iter()
                    .find(|(old, _)| *old == key)
                    .map(|(_, bytes)| bytes.clone())
            });
            let rgba = if let Some(bytes) = cached {
                bytes
            } else {
                let renderer =
                    gpui::SvgRenderer::new(std::sync::Arc::new(crate::assets::ConAssets));
                let image = renderer.render_single_frame(CORNER_MASK, side as f32)?;
                let pixels = image
                    .as_bytes(0)
                    .ok_or_else(|| anyhow::anyhow!("missing corner frame"))?;
                anyhow::ensure!(
                    pixels.len() == side * side * 16,
                    "unexpected SVG supersampling size"
                );
                let mut bytes = vec![0; side * side * 4];
                for y in 0..side {
                    for x in 0..side {
                        // GPUI rasterizes SVG masks at 2x, then linearly samples
                        // their four texels at the physical pixel centre.
                        let stride = side * 2;
                        let exterior = [(0, 0), (1, 0), (0, 1), (1, 1)]
                            .into_iter()
                            .map(|(dx, dy)| {
                                f32::from(pixels[((y * 2 + dy) * stride + x * 2 + dx) * 4 + 3])
                            })
                            .sum::<f32>()
                            / (4.0 * 255.0);
                        let inside = 1.0 - exterior;
                        let coverage = if inside == 0.0 {
                            0.0
                        } else {
                            inside / (1.0 - alpha * exterior)
                        };
                        let xx = if matches!(corner.mask, 1 | 2) {
                            side - 1 - x
                        } else {
                            x
                        };
                        let yy = if matches!(corner.mask, 2 | 3) {
                            side - 1 - y
                        } else {
                            y
                        };
                        let offset = (yy * side + xx) * 4;
                        bytes[offset + 3] = (coverage * 255.0).round().clamp(0.0, 255.0) as u8;
                    }
                }
                let bytes: std::sync::Arc<[u8]> = bytes.into();
                CACHE.with(|cache| {
                    let mut cache = cache.borrow_mut();
                    if cache.len() >= 16 {
                        cache.remove(0);
                    }
                    cache.push((key, bytes.clone()));
                });
                bytes
            };
            Ok(NativeCornerCoverage {
                bounds,
                side,
                corner: corner.mask,
                rgba,
            })
        })
        .collect()
}

fn corner_cutouts(bounds: Bounds<Pixels>, radii: gpui::Corners<Pixels>) -> Vec<CornerCutout> {
    let limit = bounds.size.width.min(bounds.size.height) * 0.5;
    let mut corners = Vec::with_capacity(4);
    for (mask, (origin, radius, x_sign, y_sign)) in [
        (bounds.origin, radii.top_left, 1.0_f32, 1.0_f32),
        (bounds.top_right(), radii.top_right, -1.0, 1.0),
        (bounds.bottom_right(), radii.bottom_right, -1.0, -1.0),
        (bounds.bottom_left(), radii.bottom_left, 1.0, -1.0),
    ]
    .into_iter()
    .enumerate()
    {
        let r = radius.min(limit);
        if r <= px(0.0) {
            continue;
        }
        corners.push(CornerCutout {
            bounds: Bounds::new(
                point(
                    origin.x + r * x_sign.min(0.0),
                    origin.y + r * y_sign.min(0.0),
                ),
                gpui::size(r, r),
            ),
            mask,
        });
    }
    corners
}

fn canvas_inset(bounds: Bounds<Pixels>, depth: f32) -> Pixels {
    px(CANVAS_INSET * depth)
        .min(bounds.size.width * 0.5)
        .min(bounds.size.height * 0.5)
        .max(px(0.0))
}

fn canvas_inner_bounds(
    bounds: Bounds<Pixels>,
    depth: f32,
    tabs: f32,
    caption: f32,
) -> Bounds<Pixels> {
    let inset = canvas_inset(bounds, depth);
    let top = px(canvas_top_inset(depth, tabs, caption)).min(inset);
    Bounds::new(
        point(bounds.left() + inset, bounds.top() + top),
        gpui::size(
            bounds.size.width - inset * 2.0,
            bounds.size.height - inset - top,
        ),
    )
}

fn surround_strips(bounds: Bounds<Pixels>, inner: Bounds<Pixels>) -> [Bounds<Pixels>; 4] {
    [
        Bounds::new(
            bounds.origin,
            gpui::size(bounds.size.width, inner.top() - bounds.top()),
        ),
        Bounds::new(
            point(bounds.left(), inner.bottom()),
            gpui::size(bounds.size.width, bounds.bottom() - inner.bottom()),
        ),
        Bounds::new(
            point(bounds.left(), inner.top()),
            gpui::size(inner.left() - bounds.left(), inner.size.height),
        ),
        Bounds::new(
            point(inner.right(), inner.top()),
            gpui::size(bounds.right() - inner.right(), inner.size.height),
        ),
    ]
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CaptionLayout {
    pub dock: f32,
    pub header: f32,
}

impl CaptionLayout {
    // The sidebar's existing spring owns time. Caption geometry must not
    // start another spring after that sidebar finally becomes wide enough.
    pub fn new(dock: f32) -> Self {
        let dock = dock.clamp(0.0, 1.0);
        Self {
            dock,
            header: 1.0 - dock,
        }
    }
}

pub(super) fn prepare_native_canvas(bounds: Bounds<Pixels>, radius: f32, window: &Window) {
    use raw_window_handle::HasWindowHandle;
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let raw_window_handle::RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    let bounds = crate::terminal_geometry::snap_terminal_bounds(bounds, window.scale_factor());
    let changed = unsafe {
        con_terminal_canvas_set_clip(
            handle.ns_view.as_ptr(),
            f64::from(bounds.left()),
            f64::from(bounds.top()),
            f64::from(bounds.size.width),
            f64::from(bounds.size.height),
            f64::from(radius),
        )
    };
    if changed {
        window.request_native_surface_presentation_sync();
    }
}

fn prepare_native_safe_area(height: f32, color: Hsla, window: &Window) {
    use raw_window_handle::HasWindowHandle;
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let raw_window_handle::RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    let color: gpui::Rgba = color.into();
    if unsafe {
        con_terminal_canvas_set_safe_area(
            handle.ns_view.as_ptr(),
            height as f64,
            color.r as f64,
            color.g as f64,
            color.b as f64,
            color.a as f64,
        )
    } {
        window.request_native_surface_presentation_sync();
    }
}

unsafe extern "C" {
    fn con_terminal_canvas_set_safe_area(
        view: *mut std::ffi::c_void,
        height: f64,
        red: f64,
        green: f64,
        blue: f64,
        alpha: f64,
    ) -> bool;
    fn con_window_corner_radius(view: *mut std::ffi::c_void) -> f64;
    fn con_terminal_canvas_set_clip(
        view: *mut std::ffi::c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        radius: f64,
    ) -> bool;
    #[cfg(test)]
    fn con_terminal_canvas_header_corners(
        view: *mut std::ffi::c_void,
        x: f64,
        y: f64,
        width: f64,
    ) -> u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_material_partitions_the_body_and_reuses_native_corner_coverage() {
        let bounds = Bounds::new(point(px(5.0), px(20.0)), gpui::size(px(120.0), px(44.0)));
        let radii = gpui::Corners {
            top_left: px(8.0),
            top_right: px(4.0),
            bottom_left: px(10.0),
            bottom_right: px(6.0),
        };
        let rgba = gpui::Rgba {
            r: 0.2,
            g: 0.4,
            b: 0.7,
            a: 0.63,
        };
        let color: Hsla = rgba.into();
        for scale in [1.0, 1.5, 2.0] {
            let tiles = native_corner_coverage(bounds, radii, scale, 0.8).unwrap();
            let corners = tiles.iter().map(|tile| tile.bounds).collect::<Vec<_>>();
            let strips = material_body_strips(bounds, &corners);
            for y in 0..(44.0 * scale) as usize {
                for x in 0..(120.0 * scale) as usize {
                    let p = point(
                        bounds.left() + px((x as f32 + 0.5) / scale),
                        bounds.top() + px((y as f32 + 0.5) / scale),
                    );
                    let count = strips
                        .iter()
                        .chain(corners.iter())
                        .filter(|rect| rect.contains(&p))
                        .count();
                    assert_eq!(count, 1, "overlap or hole in input material at {p:?}");
                }
            }
            for tile in tiles {
                let image = input_corner_image(&tile, color, 0.8);
                let again = input_corner_image(&tile, color, 0.8);
                assert!(
                    std::sync::Arc::ptr_eq(&image, &again),
                    "moving the card must not re-upload its corners"
                );
                let bytes = image.as_bytes(0).unwrap();
                for (pixel, coverage) in bytes.chunks_exact(4).zip(tile.rgba.chunks_exact(4)) {
                    assert_eq!(&pixel[..3], &[179, 102, 51], "straight-alpha BGRA");
                    assert!((f32::from(pixel[3]) - f32::from(coverage[3]) * rgba.a).abs() <= 0.51);
                }
                let transparent = input_corner_image(&tile, color.opacity(0.2), 0.8);
                assert!(
                    !std::sync::Arc::ptr_eq(&image, &transparent),
                    "opacity changes must invalidate tinted pixels"
                );
            }
        }
    }

    #[test]
    fn gpui_card_material_keeps_fill_alpha_and_a_single_physical_pixel_edge() {
        let bounds = Bounds::new(point(px(5.0), px(32.0)), gpui::size(px(320.0), px(240.0)));
        for dark in [false, true] {
            let foreground = if dark { gpui::white() } else { gpui::black() };
            let fill = if dark { gpui::black() } else { gpui::white() }.opacity(0.55);
            for scale in [1.0, 1.5, 2.0] {
                for radius in [0.0, 4.0, PANE_RADIUS] {
                    let radii = gpui::Corners::all(px(radius));
                    let quad = card_material_quad(
                        bounds,
                        radii,
                        fill,
                        card_edge_color(foreground, dark),
                        scale,
                    );
                    assert!(
                        quad.background == gpui::Background::from(fill),
                        "do not make chrome opaque"
                    );
                    assert_eq!(quad.corner_radii, radii);
                    if radius == 0.0 {
                        assert_eq!(
                            quad.border_widths,
                            gpui::Edges::default(),
                            "clean mode stays flush"
                        );
                    } else {
                        assert_eq!(quad.border_widths, gpui::Edges::all(px(1.0 / scale)));
                        assert_eq!(quad.border_color, card_edge_color(foreground, dark));
                    }
                }
            }
        }
    }

    #[gpui::test]
    fn compact_canvas_gutters_do_not_repaint_the_caption(cx: &mut gpui::TestAppContext) {
        use gpui::{ParentElement, Render, size};
        type Observation = Rc<RefCell<Option<(Bounds<Pixels>, Bounds<Pixels>)>>>;
        struct Fixture(Observation);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
                let observed = self.0.clone();
                gpui::div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(gpui::div().h(px(30.0)).flex_shrink_0())
                    .child(
                        canvas_viewport().child(
                            gpui::div().relative().flex_1().mt(px(-PANE_INSET)).child(
                                gpui::canvas(
                                    |bounds, _, _| bounds,
                                    move |_, bounds, window, _| {
                                        *observed.borrow_mut() =
                                            Some((bounds, window.content_mask().bounds));
                                    },
                                )
                                .absolute()
                                .size_full(),
                            ),
                        ),
                    )
            }
        }
        let observed = Rc::new(RefCell::new(None));
        let (_, window) = cx.add_window_view(|_, _| Fixture(observed.clone()));
        window.simulate_resize(size(px(600.0), px(400.0)));
        let (bounds, mask) = observed.borrow().expect("canvas must paint");
        assert_eq!(bounds.top(), px(28.0), "preserve compact layout geometry");
        assert_eq!(
            mask.top(),
            px(30.0),
            "gutters overlap the translucent caption"
        );
        assert!(
            mask.contains(&point(px(300.0), px(32.0))),
            "do not clip pane content"
        );
    }

    #[gpui::test]
    fn selected_tab_clearance_does_not_clip_the_pane_top_or_shift_the_bottom(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{ParentElement, Render, size};
        struct Fixture(f32);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
                let bleed = canvas_top_bleed(1.0, self.0, 1.0);
                let (agent_top, agent_bottom) = agent_vertical_insets(1.0, false, 0.0, self.0, 1.0);
                gpui::div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(
                        gpui::div()
                            .h(px(super::super::TOP_BAR_TABS_HEIGHT))
                            .flex_shrink_0(),
                    )
                    .child(
                        gpui::div()
                            .relative()
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .child(
                                canvas_viewport().mt(px(-bleed)).pt(px(bleed)).child(
                                    gpui::div()
                                        .relative()
                                        .flex()
                                        .flex_col()
                                        .flex_1()
                                        .min_h_0()
                                        .child(
                                            gpui::div()
                                                .relative()
                                                .flex_1()
                                                .m(px(CANVAS_INSET))
                                                .mt(px(canvas_top_inset(1.0, self.0, 1.0)))
                                                .child(
                                                    gpui::div()
                                                        .absolute()
                                                        .inset_0()
                                                        .m(px(PANE_INSET))
                                                        .debug_selector(|| "pane-clearance".into()),
                                                ),
                                        ),
                                ),
                            )
                            .child(
                                gpui::div()
                                    .w(px(100.0))
                                    .mt(px(agent_top.min(0.0)))
                                    .pt(px(agent_top.max(0.0)))
                                    .pb(px(agent_bottom))
                                    .child(
                                        gpui::div()
                                            .size_full()
                                            .debug_selector(|| "agent-clearance".into()),
                                    ),
                            ),
                    )
            }
        }
        let (view, cx) = cx.add_window_view(|_, _| Fixture(1.0));
        cx.simulate_resize(size(px(600.0), px(400.0)));
        for tabs in [0.0, 0.25, 0.5, 0.75, 1.0] {
            view.update(cx, |view, cx| {
                view.0 = tabs;
                cx.notify();
            });
            let pane = cx.debug_bounds("pane-clearance").unwrap();
            let agent = cx.debug_bounds("agent-clearance").unwrap();
            assert_eq!(pane.top(), agent.top());
            assert_eq!(pane.bottom(), agent.bottom());
            assert_eq!(pane.bottom(), px(400.0 - CARD_OUTER_INSET));
            let clip_top = super::super::TOP_BAR_TABS_HEIGHT - canvas_top_bleed(1.0, tabs, 1.0);
            assert!(
                pane.top() >= px(clip_top),
                "pane top must remain inside the viewport"
            );
        }
    }

    #[test]
    fn agent_edges_follow_terminal_cards_and_the_visible_input_tray() {
        for depth in [0.0, 0.5, 1.0] {
            for split in [false, true] {
                let (top, bottom) = agent_vertical_insets(depth, split, 0.0, 0.0, 0.0);
                assert_eq!(
                    top,
                    CANVAS_INSET * depth + pane_card_geometry(split, depth).0
                );
                assert_eq!(bottom, top);
                let reserved = input_reserved_height(44.0, true, 1.0, depth);
                let (with_input_top, with_input_bottom) =
                    agent_vertical_insets(depth, split, reserved, 0.0, 0.0);
                assert_eq!(with_input_top, top);
                assert_eq!(with_input_bottom, top + reserved);
            }
        }
    }

    #[test]
    fn contact_contours_remain_outside_the_surface_and_use_small_disjoint_masks() {
        for (width, height) in [(320.0, 44.0), (1200.0, 800.0), (2400.0, 1600.0)] {
            let bounds = Bounds::new(point(px(12.0), px(28.0)), gpui::size(px(width), px(height)));
            let mut area = 0.0;
            let outer = bounds.dilate(px(0.5));
            // The border's inner bounds/radius are exactly the original shape.
            assert_eq!(outer.inset(px(0.5)), bounds);
            let regions =
                contact_regions(outer, gpui::Corners::all(px(PANE_RADIUS + 0.5)), px(0.5));
            for (i, (strip, _)) in regions.iter().enumerate() {
                assert!(!strip.contains(&bounds.center()));
                for (other, _) in &regions[i + 1..] {
                    let overlap = strip.intersect(other);
                    assert!(overlap.size.width <= px(0.0) || overlap.size.height <= px(0.0));
                }
                area += strip.size.width.as_f32() * strip.size.height.as_f32();
            }
            assert!(area < width * 2.0 + height * 2.0 + 300.0);
        }
    }

    #[test]
    fn outline_masks_keep_raster_work_on_the_perimeter() {
        for (width, height) in [(320.0, 44.0), (1200.0, 800.0), (2400.0, 1600.0)] {
            let bounds = Bounds::new(point(px(12.0), px(28.0)), gpui::size(px(width), px(height)));
            let mut area = 0.0;
            for strip in outline_strips(bounds, gpui::Corners::all(px(8.0)), px(0.5)) {
                assert!(strip.left() >= bounds.left() && strip.right() <= bounds.right());
                assert!(strip.top() >= bounds.top() && strip.bottom() <= bounds.bottom());
                assert!(!strip.contains(&bounds.center()));
                area += strip.size.width.as_f32() * strip.size.height.as_f32();
            }
            assert!(area <= width * 16.0 + height);
        }
    }

    #[test]
    fn input_tray_and_popovers_align_with_pane_card_edges() {
        for width in [0.0, 20.0, 320.0, 900.0, 1600.0] {
            for depth in [0.0, 0.5, 1.0] {
                let (left, island) = input_island_geometry(width, depth);
                assert!(left >= 0.0 && island >= 0.0);
                assert!((left * 2.0 + island - width).abs() < 0.001);
                assert!(left + island <= width);
                if depth == 0.0 {
                    assert_eq!((left, island), (0.0, width));
                }
            }
        }
        assert_eq!(input_island_geometry(1600.0, 1.0), (5.0, 1590.0));
        assert_eq!(input_island_geometry(900.0, 1.0), (5.0, 890.0));
        assert_eq!(input_safe_height(44.0, 1.0, 1.0), 49.0);
        assert_eq!(input_safe_height(44.0, 0.0, 1.0), 0.0);
    }

    #[test]
    fn workspace_gutters_and_composer_baselines_agree() {
        for height in [44.0, 72.0, 104.0] {
            let window_bottom = 600.0;
            let reserved = input_reserved_height(height, true, 1.0, 1.0);
            let pane_bottom = window_bottom - CANVAS_INSET - reserved - PANE_INSET;
            let tray_bottom = window_bottom - CANVAS_INSET - INPUT_BOTTOM_INSET;
            let tray_top = tray_bottom - height;
            assert_eq!(tray_top - pane_bottom, 5.0);
            assert_eq!(window_bottom - tray_bottom, 5.0);
            assert_eq!(2.0 * PANE_INSET + 1.0, tray_top - pane_bottom);
            assert_eq!(CARD_OUTER_INSET, tray_top - pane_bottom);

            for split in [false, true] {
                for input_height in [0.0, reserved] {
                    let (_, padding_bottom) =
                        agent_vertical_insets(1.0, split, input_height, 0.0, 0.0);
                    let terminal_bottom = window_bottom - CANVAS_INSET - input_height - PANE_INSET;
                    assert_eq!(window_bottom - padding_bottom, terminal_bottom);
                }
            }
        }
    }

    #[test]
    fn split_panes_keep_outer_clearance_without_sidebars_or_extra_tabs() {
        for (tabs, split, left, right, expected) in [
            (false, false, 0.0, 0.0, 0.0),
            (false, false, 0.2, 0.7, 0.7),
            (true, false, 0.0, 0.0, 1.0),
            (false, true, 0.0, 0.0, 1.0),
            (false, true, 0.4, 0.2, 1.0),
        ] {
            let depth = canvas_depth_for_layout(tabs, split, left, right);
            assert_eq!(depth, expected);
            if split {
                assert_eq!(
                    CANVAS_INSET * depth + pane_card_geometry(split, depth).0,
                    5.0
                );
            }
        }
    }

    #[test]
    fn horizontal_tabs_match_selected_surface_clearance_to_pane_gutters() {
        let bounds = Bounds::new(
            point(px(0.0), px(super::super::TOP_BAR_TABS_HEIGHT)),
            gpui::size(px(600.0), px(400.0)),
        );
        let tab_bottom = (super::super::TOP_BAR_TABS_HEIGHT + super::super::TOP_BAR_TAB_HEIGHT)
            * 0.5
            - super::super::TOP_BAR_TAB_SELECTION_INSET;
        let inner = canvas_inner_bounds(bounds, 1.0, 1.0, 1.0);
        let pane_top = inner.top().as_f32() + PANE_INSET;
        assert_eq!(pane_top - tab_bottom, CANVAS_INSET + PANE_INSET);
        assert_eq!(inner.left().as_f32() + PANE_INSET, pane_top - tab_bottom);
        for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let inner = canvas_inner_bounds(bounds, 1.0, progress, 1.0);
            let (agent_top, _) = agent_vertical_insets(1.0, false, 0.0, progress, 1.0);
            assert_eq!(bounds.top() + px(agent_top), inner.top() + px(PANE_INSET));
            assert_eq!(inner.bottom(), bounds.bottom() - px(CANVAS_INSET));
            for strip in surround_strips(bounds, inner) {
                let overlap = strip.intersect(&inner);
                assert!(overlap.size.width <= px(0.0) || overlap.size.height <= px(0.0));
            }
        }
        assert_eq!(canvas_top_inset(0.0, 1.0, 1.0), 0.0);
    }

    #[test]
    fn cards_share_the_window_corner_centre_only_where_they_meet_it() {
        let viewport = gpui::size(px(1000.0), px(700.0));
        let inset = CARD_OUTER_INSET;
        let card = Bounds::new(
            point(px(240.0), px(40.0)),
            gpui::size(px(1000.0 - 240.0 - inset), px(700.0 - 40.0 - inset)),
        );
        let radii = concentric_corners(card, viewport, 16.0, PANE_RADIUS);
        assert_eq!(radii.bottom_right, px(16.0 - inset));
        assert_eq!(radii.bottom_left, px(PANE_RADIUS));
        assert_eq!(radii.top_left, px(PANE_RADIUS));
        assert_eq!(radii.top_right, px(PANE_RADIUS));
        // A card opening away from the corner settles back to its own radius.
        let away = Bounds::new(card.origin, card.size - gpui::size(px(3.0), px(0.0)));
        let radii = concentric_corners(away, viewport, 16.0, PANE_RADIUS);
        assert_eq!(radii.bottom_right, px(PANE_RADIUS));
        // Fullscreen windows and flat cards keep their own geometry.
        assert_eq!(
            concentric_corners(card, viewport, 0.0, PANE_RADIUS),
            gpui::Corners::all(px(PANE_RADIUS))
        );
        assert_eq!(
            concentric_corners(card, viewport, 16.0, 0.0),
            gpui::Corners::all(px(0.0))
        );
    }

    #[test]
    fn caption_controls_sit_midway_above_the_terminal_card() {
        let row = super::super::TOP_BAR_COMPACT_HEIGHT;
        let bounds = Bounds::new(point(px(0.0), px(row)), gpui::size(px(600.0), px(400.0)));
        let card_top = canvas_inner_bounds(bounds, 1.0, 0.0, 1.0).top().as_f32() + PANE_INSET;
        // Controls are centred in the caption row, like the traffic lights.
        assert_eq!(card_top - row * 0.5, row * 0.5);
        let (agent_top, _) = agent_vertical_insets(1.0, false, 0.0, 0.0, 1.0);
        assert_eq!(row + agent_top, card_top);
        // A docked caption gives its row back; the card keeps its full margin.
        let docked = Bounds::new(point(px(0.0), px(0.0)), bounds.size);
        assert_eq!(
            canvas_inner_bounds(docked, 1.0, 0.0, 0.0).top().as_f32() + PANE_INSET,
            CANVAS_INSET + PANE_INSET
        );
    }

    #[test]
    fn window_corners_stay_within_small_cards() {
        let card = Bounds::new(point(px(5.0), px(5.0)), gpui::size(px(10.0), px(6.0)));
        let radii = concentric_corners(card, gpui::size(px(20.0), px(16.0)), 16.0, PANE_RADIUS);
        assert_eq!(radii, gpui::Corners::all(px(3.0)));
        let flat = Bounds::new(card.origin, gpui::size(px(10.0), px(0.0)));
        assert_eq!(
            concentric_corners(flat, gpui::size(px(20.0), px(16.0)), 16.0, PANE_RADIUS),
            gpui::Corners::all(px(0.0))
        );
    }

    #[test]
    fn caption_controls_fit_while_navigation_width_changes() {
        for sidebar in [44.0, 90.0, 184.0, 230.0] {
            for dock in [0.0, 0.5, 1.0] {
                let width = caption_width(1000.0, sidebar, dock, 78.0);
                assert!(width >= 168.0);
                assert!(width <= 1000.0);
                if dock > 0.0 && sidebar >= caption_minimum_width(78.0) {
                    assert_eq!(width, sidebar, "toolbar crossed native terminal content");
                } else {
                    assert_eq!(width, 1000.0);
                }
            }
        }
    }

    #[gpui::test]
    fn input_reveal_preserves_content_size_while_clipping(cx: &mut gpui::TestAppContext) {
        use gpui::{
            Context, InteractiveElement, IntoElement, ParentElement, Render, Window, div, size,
        };
        struct Reveal {
            progress: f32,
            depth: f32,
        }
        impl Render for Reveal {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().relative().w_full().h(px(64.0)).child(
                    input_reveal_clip(44.0, self.progress, self.depth)
                        .debug_selector(|| "input-reveal-clip".into())
                        .child(
                            input_card(
                                320.0,
                                44.0,
                                self.depth,
                                gpui::black().opacity(0.6),
                                gpui::white(),
                                true,
                                gpui::white().opacity(0.8),
                            )
                            .debug_selector(|| "fixed-composer".into())
                            .child(gpui::div().size_full()),
                        ),
                )
            }
        }
        let (view, cx) = cx.add_window_view(|_, _| Reveal {
            progress: 1.0,
            depth: 1.0,
        });
        cx.simulate_resize(size(px(500.0), px(300.0)));
        for depth in [0.0, 0.5, 1.0] {
            view.update(cx, |view, cx| {
                view.depth = depth;
                cx.notify();
            });
            let fixed = cx.debug_bounds("fixed-composer").unwrap();
            assert_eq!(fixed.left(), px(PANE_INSET * depth));
            assert_eq!(fixed.size.width, px(320.0 - 2.0 * CARD_OUTER_INSET * depth));
            for progress in [0.25, 0.7, 0.4, 1.0] {
                view.update(cx, |view, cx| {
                    view.progress = progress;
                    cx.notify();
                });
                let content = cx.debug_bounds("fixed-composer").unwrap();
                assert_eq!(content.size, fixed.size);
                assert_eq!(content.left(), fixed.left());
                assert_eq!(
                    content,
                    cx.debug_bounds("input-card-edge").unwrap(),
                    "the fine contour must cover the entire moving card"
                );
                assert_eq!(
                    content.top(),
                    cx.debug_bounds("input-reveal-clip").unwrap().top()
                );
                let clip = cx.debug_bounds("input-reveal-clip").unwrap();
                assert!(
                    (clip.bottom().as_f32() - (64.0 - INPUT_BOTTOM_INSET * depth * progress)).abs()
                        <= 0.51
                );
                assert!((clip.size.height.as_f32() - 44.0 * progress).abs() <= 0.51);
            }
        }
    }

    #[gpui::test]
    fn input_viewport_tracks_reveal_reversal_without_an_endpoint_jump(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{
            Context, InteractiveElement, IntoElement, ParentElement, Render, Window, div, size,
        };
        struct Reveal {
            progress: f32,
            visible: bool,
            geometry: Rc<Cell<InputTrayBounds>>,
        }
        impl Render for Reveal {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let tray = InputTray::default();
                self.geometry = tray.bounds.clone();
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .w_full()
                    .h(px(300.0))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .debug_selector(|| "output-viewport".into()),
                    )
                    .child(
                        div()
                            .relative()
                            .debug_selector(|| "input-reservation".into())
                            .h(px(input_reserved_height(
                                44.0,
                                self.visible,
                                self.progress,
                                1.0,
                            )))
                            .flex_shrink_0()
                            .child(tray.surround(gpui::black().opacity(0.9))),
                    )
                    .child(
                        tray.track_reveal(
                            input_reveal_clip(44.0, self.progress, 1.0)
                                .debug_selector(|| "actual-input-reveal".into())
                                .child(
                                    tray.track_card(
                                        input_card(
                                            500.0,
                                            44.0,
                                            1.0,
                                            gpui::black().opacity(0.8),
                                            gpui::white(),
                                            true,
                                            gpui::black().opacity(0.9),
                                        )
                                        .debug_selector(|| "island-frame".into()),
                                    ),
                                ),
                        ),
                    )
            }
        }
        let (view, cx) = cx.add_window_view(|_, _| Reveal {
            progress: 0.0,
            visible: false,
            geometry: Rc::default(),
        });
        cx.simulate_resize(size(px(500.0), px(400.0)));
        let island = cx.debug_bounds("island-frame").unwrap();
        for (progress, visible, expected_output) in [
            (0.1, true, 295.1),
            (0.7, true, 265.7),
            (1.0, true, 251.0),
            (0.6, false, 270.6),
            (0.8, true, 260.8),
            (1.0, true, 251.0),
            (0.0, false, 300.0),
        ] {
            view.update(cx, |view, cx| {
                view.progress = progress;
                view.visible = visible;
                cx.notify();
            });
            assert_eq!(cx.debug_bounds("island-frame").unwrap().size, island.size);
            assert!(
                (cx.debug_bounds("output-viewport")
                    .unwrap()
                    .size
                    .height
                    .as_f32()
                    - expected_output)
                    .abs()
                    <= 0.51
            );
            let reservation = cx.debug_bounds("input-reservation").unwrap();
            let card = cx.debug_bounds("island-frame").unwrap();
            let reveal = cx.debug_bounds("actual-input-reveal").unwrap();
            let geometry = view.read_with(cx, |view, _| view.geometry.get());
            if progress > 0.0 {
                assert_eq!(geometry.card, Some(card));
                assert_eq!(geometry.reveal, Some(reveal));
            }
            // A full card extends below the viewport during closing. Chrome
            // must cover the gap below its reveal mask, even at fractional DPI.
            for scale in [1.0, 1.5, 2.0] {
                let snap = |bounds| crate::terminal_geometry::snap_terminal_bounds(bounds, scale);
                let reservation = snap(reservation);
                let geometry = InputTrayBounds {
                    card: geometry.card.map(snap),
                    reveal: geometry.reveal.map(snap),
                };
                let regions = geometry.surround_regions(reservation);
                for y in 0..(reservation.size.height.as_f32() * scale).round() as usize {
                    for x in [0.5, 250.5, 499.5] {
                        let p = point(px(x), reservation.top() + px((y as f32 + 0.5) / scale));
                        if !reservation.contains(&p) {
                            continue;
                        }
                        let visible = geometry
                            .card
                            .zip(geometry.reveal)
                            .is_some_and(|(card, reveal)| card.contains(&p) && reveal.contains(&p));
                        assert_eq!(
                            regions.iter().filter(|region| region.contains(&p)).count(),
                            usize::from(!visible),
                            "exposed or double-painted tray pixel at {p:?}, progress={progress}, scale={scale}"
                        );
                    }
                }
            }
        }
    }

    #[gpui::test]
    fn native_safe_area_is_cached_passive_and_removed_from_clean_layouts() {
        use cocoa::foundation::{NSPoint, NSRect, NSSize};
        use objc::{class, msg_send, sel, sel_impl};
        unsafe {
            let parent: *mut objc::runtime::Object = msg_send![class!(NSView), alloc];
            let parent: *mut objc::runtime::Object = msg_send![parent, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0))];
            let gpui: *mut objc::runtime::Object = msg_send![class!(NSView), alloc];
            let gpui: *mut objc::runtime::Object = msg_send![gpui, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0))];
            let _: () = msg_send![parent, addSubview: gpui];
            con_terminal_canvas_set_clip(gpui.cast(), 4.0, 4.0, 592.0, 392.0, 8.0);
            assert!(!con_terminal_canvas_set_safe_area(
                gpui.cast(),
                0.0,
                1.0,
                0.9,
                0.8,
                0.9
            ));
            assert!(con_terminal_canvas_set_safe_area(
                gpui.cast(),
                64.0,
                1.0,
                0.9,
                0.8,
                0.9
            ));
            assert!(!con_terminal_canvas_set_safe_area(
                gpui.cast(),
                64.0,
                1.0,
                0.9,
                0.8,
                0.9
            ));
            let children: *mut objc::runtime::Object = msg_send![parent, subviews];
            let count: usize = msg_send![children, count];
            assert_eq!(count, 2);
            let material: *mut objc::runtime::Object = msg_send![children, objectAtIndex: 0usize];
            let rect: NSRect = msg_send![material, frame];
            assert_eq!(
                (
                    rect.origin.x,
                    rect.origin.y,
                    rect.size.width,
                    rect.size.height
                ),
                (4.0, 4.0, 592.0, 64.0)
            );
            let hit: *mut objc::runtime::Object =
                msg_send![material, hitTest: NSPoint::new(20.0, 20.0)];
            assert!(hit.is_null());
            let accepts: bool = msg_send![material, acceptsFirstResponder];
            assert!(!accepts);
            assert!(con_terminal_canvas_set_safe_area(
                gpui.cast(),
                0.0,
                1.0,
                0.9,
                0.8,
                0.9
            ));
            let hidden: bool = msg_send![material, isHidden];
            assert!(hidden);
            assert!(con_terminal_canvas_set_safe_area(
                gpui.cast(),
                64.0,
                0.2,
                0.2,
                0.2,
                0.8
            ));
            let layer: *mut objc::runtime::Object = msg_send![material, layer];
            let mask: *mut objc::runtime::Object = msg_send![layer, mask];
            assert!(!mask.is_null());
            let count: usize = msg_send![children, count];
            assert_eq!(count, 2, "retargeting allocated another native material");
            let _: () = msg_send![gpui, release];
            let _: () = msg_send![parent, release];
        }
    }

    #[test]
    fn chrome_surround_never_paints_the_terminal_interior() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), gpui::size(px(600.0), px(400.0)));
        let strips = surround_strips(bounds, bounds.inset(canvas_inset(bounds, 1.0)));
        let contains = |x: f32, y: f32| {
            let p = point(px(x), px(y));
            strips.iter().any(|strip| strip.contains(&p))
        };
        for (x, y) in [(11.0, 220.0), (609.0, 220.0), (310.0, 21.0), (310.0, 419.0)] {
            assert!(contains(x, y), "missing chrome at {x},{y}");
        }
        for (x, y) in [(310.0, 220.0), (26.0, 28.0), (310.0, 28.0), (602.0, 220.0)] {
            assert!(!contains(x, y), "chrome painted terminal at {x},{y}");
        }
        assert_eq!(canvas_inset(bounds, 0.0), px(0.0));
        assert!(
            surround_strips(bounds, bounds)
                .iter()
                .all(|strip| strip.size.width == px(0.0) || strip.size.height == px(0.0))
        );
    }

    #[test]
    fn corner_masks_keep_smooth_coverage_and_leave_the_card_interior_unpainted() {
        let renderer = gpui::SvgRenderer::new(std::sync::Arc::new(crate::assets::ConAssets));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(600.0), px(400.0)));
        for scale in [1.0, 1.5, 2.0] {
            let masks = corner_cutouts(bounds, gpui::Corners::all(px(PANE_RADIUS)));
            assert_eq!(masks.len(), 4);
            for mask in masks {
                assert!(mask.bounds.size.width <= px(PANE_RADIUS));
                assert!(mask.bounds.size.height <= px(PANE_RADIUS));
            }
            let image = renderer
                .render_single_frame(CORNER_MASK, PANE_RADIUS * scale)
                .unwrap();
            let alphas = image
                .as_bytes(0)
                .unwrap()
                .chunks_exact(4)
                .map(|pixel| pixel[3])
                .collect::<Vec<_>>();
            let levels = alphas
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            assert!(
                levels.len() > 8,
                "coverage collapsed to coarse multisample steps at {scale}x"
            );
            let side = (PANE_RADIUS * scale * gpui::SMOOTH_SVG_SCALE_FACTOR) as usize;
            assert_eq!(alphas[0], 255);
            assert_eq!(alphas[side * side - 1], 0);
            assert!(alphas[side - 1] <= 8, "cutout extends beyond the tangent");
            let area = alphas
                .iter()
                .map(|alpha| f32::from(*alpha) / 255.0)
                .sum::<f32>()
                / (side * side) as f32;
            assert!(
                (area - (1.0 - std::f32::consts::PI / 4.0)).abs() < 0.015,
                "mask does not follow the circular clip"
            );
        }
        assert!(corner_cutouts(bounds, gpui::Corners::default()).is_empty());
        let masks = corner_cutouts(
            bounds,
            gpui::Corners {
                top_left: px(0.0),
                top_right: px(11.0),
                bottom_left: px(0.0),
                bottom_right: px(8.0),
            },
        );
        assert_eq!(
            masks.iter().map(|mask| mask.mask).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(masks[0].bounds.origin, point(px(589.0), px(0.0)));
        assert_eq!(masks[1].bounds.origin, point(px(592.0), px(392.0)));
    }

    #[test]
    fn shared_corner_coverage_preserves_material_weights_and_reuses_tiles() {
        let bounds = Bounds::new(point(px(5.0), px(7.0)), gpui::size(px(200.0), px(120.0)));
        let renderer = gpui::SvgRenderer::new(std::sync::Arc::new(crate::assets::ConAssets));
        for scale in [1.0, 1.5, 2.0] {
            for alpha in [0.0, 0.25, 0.82, 1.0] {
                let tiles =
                    native_corner_coverage(bounds, gpui::Corners::all(px(8.0)), scale, alpha)
                        .unwrap();
                let again =
                    native_corner_coverage(bounds, gpui::Corners::all(px(8.0)), scale, alpha)
                        .unwrap();
                assert_eq!(tiles.len(), 4);
                for (tile, cached) in tiles.iter().zip(&again) {
                    assert!(std::sync::Arc::ptr_eq(&tile.rgba, &cached.rgba));
                }
                let side = tiles[0].side;
                let exterior = renderer
                    .render_single_frame(CORNER_MASK, side as f32)
                    .unwrap();
                let pixels = exterior.as_bytes(0).unwrap();
                for y in 0..side {
                    for x in 0..side {
                        let m = [(0, 0), (1, 0), (0, 1), (1, 1)]
                            .into_iter()
                            .map(|(dx, dy)| {
                                f32::from(pixels[((2 * y + dy) * side * 2 + 2 * x + dx) * 4 + 3])
                            })
                            .sum::<f32>()
                            / 1020.0;
                        let n = f32::from(tiles[0].rgba[(y * side + x) * 4 + 3]) / 255.0;
                        // Native terminal survives the translucent overlay with
                        // the intended inside weight, without leaking backdrop.
                        assert!(((1.0 - alpha * m) * n - (1.0 - m)).abs() <= 0.5 / 255.0 + 0.00001);
                        for (corner, tile) in tiles.iter().enumerate() {
                            let xx = if matches!(corner, 1 | 2) {
                                side - 1 - x
                            } else {
                                x
                            };
                            let yy = if matches!(corner, 2 | 3) {
                                side - 1 - y
                            } else {
                                y
                            };
                            assert_eq!(
                                tile.rgba[(yy * side + xx) * 4 + 3],
                                tiles[0].rgba[(y * side + x) * 4 + 3]
                            );
                        }
                    }
                }
            }
        }
        assert!(
            native_corner_coverage(bounds, gpui::Corners::default(), 2.0, 0.82)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn corner_reflections_stay_in_their_device_pixel_tiles() {
        let bounds = Bounds::new(point(px(5.0), px(7.0)), gpui::size(px(200.0), px(120.0)));
        let masks = corner_cutouts(bounds, gpui::Corners::all(px(8.0)));
        for scale in [1.0, 1.5, 2.0] {
            for (corner, mask) in masks.iter().enumerate() {
                let (tile, transform) = mask.presentation(scale).unwrap();
                let physical = |x: Pixels, y: Pixels| point(x * scale, y * scale);
                let min = physical(tile.left(), tile.top());
                let max = physical(tile.right(), tile.bottom());
                let size = tile.size.map(|value| value * scale);
                let exterior = transform.apply(min + point(size.width * 0.1, size.height * 0.1));
                let interior = transform.apply(min + point(size.width * 0.9, size.height * 0.9));
                assert_eq!(exterior.x > interior.x, matches!(corner, 1 | 2));
                assert_eq!(exterior.y > interior.y, matches!(corner, 2 | 3));
                for source in [min, max, point(min.x, max.y), point(max.x, min.y)] {
                    let target = transform.apply(source);
                    assert!(target.x >= min.x - px(0.0001) && target.x <= max.x + px(0.0001));
                    assert!(target.y >= min.y - px(0.0001) && target.y <= max.y + px(0.0001));
                }
                let restored = transform.apply(transform.apply(min));
                assert!((restored.x - min.x).abs() <= px(0.0001));
                assert!((restored.y - min.y).abs() <= px(0.0001));
            }
            let tiny = CornerCutout {
                bounds: Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(0.01), px(0.01))),
                mask: 0,
            };
            assert!(
                tiny.presentation(scale).is_none(),
                "subpixel mask must not allocate a zero-sized SVG"
            );
        }
    }

    #[gpui::test]
    fn rounded_canvas_creates_a_mask_for_a_view_backed_scroll_host() {
        use cocoa::foundation::{NSPoint, NSRect, NSSize};
        use objc::{class, msg_send, sel, sel_impl};
        use std::ffi::c_void;
        unsafe extern "C" {
            fn con_terminal_canvas_apply_clip(view: *mut c_void);
            fn con_terminal_canvas_clip_is_attached(view: *mut c_void) -> bool;
        }
        unsafe {
            let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, 300.0));
            let parent: *mut objc::runtime::Object = msg_send![class!(NSView), alloc];
            let parent: *mut objc::runtime::Object = msg_send![parent, initWithFrame: rect];
            let host: *mut objc::runtime::Object = msg_send![class!(NSScrollView), alloc];
            let host: *mut objc::runtime::Object = msg_send![host, initWithFrame: rect];
            let _: () = msg_send![parent, addSubview: host];
            let _: () = msg_send![host, setWantsLayer: false];
            let layer: *mut objc::runtime::Object = msg_send![host, layer];
            assert!(
                layer.is_null(),
                "fixture must exercise the unbacked host path"
            );
            con_terminal_canvas_set_clip(parent.cast(), 0.0, 0.0, 400.0, 300.0, 8.0);
            con_terminal_canvas_apply_clip(host.cast());
            assert!(con_terminal_canvas_clip_is_attached(host.cast()));
            let parent_layer: *mut objc::runtime::Object = msg_send![parent, layer];
            assert!(
                parent_layer.is_null(),
                "mask must not change window-container backing"
            );
            let _: () = msg_send![host, removeFromSuperview];
            let _: () = msg_send![host, release];
            let _: () = msg_send![parent, release];
        }
    }

    #[gpui::test]
    fn native_canvas_clip_tracks_parent_geometry_and_reuses_unchanged_masks() {
        use cocoa::foundation::{NSPoint, NSRect, NSSize};
        use objc::{class, msg_send, sel, sel_impl};
        use std::ffi::c_void;
        unsafe extern "C" {
            fn con_terminal_canvas_generation(view: *mut c_void) -> u64;
            fn con_terminal_canvas_apply_clip(view: *mut c_void);
            fn con_terminal_canvas_clip_is_attached(view: *mut c_void) -> bool;
            fn con_terminal_pane_set_clip(
                view: *mut c_void,
                x: f64,
                y: f64,
                width: f64,
                height: f64,
                radius: f64,
            ) -> bool;
            fn CGPathContainsPoint(
                path: *const c_void,
                transform: *const c_void,
                point: NSPoint,
                even_odd: bool,
            ) -> bool;
        }
        unsafe {
            let parent: *mut objc::runtime::Object = msg_send![class!(NSView), alloc];
            let parent: *mut objc::runtime::Object = msg_send![parent, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0))];
            let host: *mut objc::runtime::Object = msg_send![class!(NSView), alloc];
            let host: *mut objc::runtime::Object = msg_send![host, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0))];
            let _: () = msg_send![parent, addSubview: host];
            let _: () = msg_send![host, setWantsLayer: true];
            let parent_ptr = parent.cast();
            assert!(con_terminal_canvas_set_clip(
                parent_ptr, 6.0, 6.0, 588.0, 388.0, 9.0
            ));
            assert_eq!(con_terminal_canvas_generation(host.cast()), 1);
            let subviews: *mut objc::runtime::Object = msg_send![parent, subviews];
            let view_count: usize = msg_send![subviews, count];
            assert_eq!(view_count, 1, "edge material added an input-receiving view");
            con_terminal_canvas_apply_clip(host.cast());
            let layer: *mut objc::runtime::Object = msg_send![host, layer];
            let mask: *mut objc::runtime::Object = msg_send![layer, mask];
            assert!(!mask.is_null());
            let path: *const c_void = msg_send![mask, path];
            let contains =
                |x, y| CGPathContainsPoint(path, std::ptr::null(), NSPoint::new(x, y), false);
            assert!(contains(300.0, 200.0));
            assert!(contains(15.0, 7.0));
            assert!(!contains(6.5, 6.5));
            assert!(!contains(2.0, 200.0));
            assert!(!con_terminal_canvas_set_clip(
                parent_ptr, 6.0, 6.0, 588.0, 388.0, 9.0
            ));
            con_terminal_canvas_apply_clip(host.cast());
            let same_path: *const c_void = msg_send![mask, path];
            assert_eq!(path, same_path, "idle layout rebuilt the mask");
            assert!(con_terminal_canvas_clip_is_attached(host.cast()));
            let _: () = msg_send![layer, setMask: std::ptr::null_mut::<objc::runtime::Object>()];
            assert!(!con_terminal_canvas_clip_is_attached(host.cast()));
            con_terminal_canvas_apply_clip(host.cast());
            assert!(
                con_terminal_canvas_clip_is_attached(host.cast()),
                "same-size layout failed to reattach its mask"
            );
            let replacement: *mut objc::runtime::Object = msg_send![class!(CALayer), layer];
            let _: () = msg_send![host, setLayer: replacement];
            assert!(!con_terminal_canvas_clip_is_attached(host.cast()));
            con_terminal_canvas_apply_clip(host.cast());
            assert!(
                con_terminal_canvas_clip_is_attached(host.cast()),
                "same-size replacement layer bypassed the mask cache"
            );
            let layer = replacement;
            assert_eq!(
                con_terminal_canvas_header_corners(parent_ptr, 6.0, 6.0, 588.0),
                3
            );
            assert_eq!(
                con_terminal_canvas_header_corners(parent_ptr, 300.0, 6.0, 294.0),
                2
            );
            assert_eq!(
                con_terminal_canvas_header_corners(parent_ptr, 6.0, 100.0, 588.0),
                0
            );
            assert!(con_terminal_canvas_set_clip(
                parent_ptr, 12.0, 6.0, 580.0, 300.0, 9.0
            ));
            assert_eq!(con_terminal_canvas_generation(host.cast()), 2);
            con_terminal_canvas_apply_clip(host.cast());
            let changed_path: *const c_void = msg_send![mask, path];
            assert_ne!(changed_path, same_path);
            assert!(!CGPathContainsPoint(
                changed_path,
                std::ptr::null(),
                NSPoint::new(15.0, 50.0),
                false
            ));
            assert!(con_terminal_canvas_set_clip(
                parent_ptr, 6.0, 6.0, 588.0, 388.0, 9.0
            ));
            let _: () = msg_send![host, setFrame: NSRect::new(NSPoint::new(298.0, 4.0), NSSize::new(298.0, 392.0))];
            con_terminal_canvas_apply_clip(host.cast());
            let split_path: *const c_void = msg_send![mask, path];
            assert!(CGPathContainsPoint(
                split_path,
                std::ptr::null(),
                NSPoint::new(100.0, 200.0),
                false
            ));
            assert!(!CGPathContainsPoint(
                split_path,
                std::ptr::null(),
                NSPoint::new(295.5, 389.5),
                false
            ));
            let generation = con_terminal_canvas_generation(host.cast());
            assert!(!con_terminal_canvas_set_clip(
                parent_ptr,
                f64::NAN,
                0.0,
                10.0,
                10.0,
                9.0
            ));
            assert_eq!(con_terminal_canvas_generation(host.cast()), generation);
            assert!(con_terminal_canvas_set_clip(
                parent_ptr, 0.0, 0.0, 600.0, 400.0, 0.0
            ));
            con_terminal_canvas_apply_clip(host.cast());
            let clean_mask: *mut objc::runtime::Object = msg_send![layer, mask];
            assert!(
                clean_mask.is_null(),
                "clean terminal retained an offscreen mask"
            );
            // A split pane intersects the workspace mask; it must not replace
            // it, rebuild idle paths, or leave rounded clipping after unzoom.
            let _: () = msg_send![host, setFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0))];
            assert!(con_terminal_pane_set_clip(
                host.cast(),
                10.0,
                10.0,
                280.0,
                380.0,
                7.0
            ));
            con_terminal_canvas_apply_clip(host.cast());
            let pane_mask: *mut objc::runtime::Object = msg_send![layer, mask];
            let inner_mask: *mut objc::runtime::Object = msg_send![pane_mask, mask];
            assert!(!inner_mask.is_null());
            let pane_path: *const c_void = msg_send![inner_mask, path];
            let sibling: *mut objc::runtime::Object = msg_send![class!(NSView), alloc];
            let sibling: *mut objc::runtime::Object = msg_send![sibling, initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0))];
            let _: () = msg_send![parent, addSubview: sibling];
            let _: () = msg_send![sibling, setWantsLayer: true];
            con_terminal_canvas_apply_clip(sibling.cast());
            let sibling_layer: *mut objc::runtime::Object = msg_send![sibling, layer];
            let sibling_mask: *mut objc::runtime::Object = msg_send![sibling_layer, mask];
            assert!(
                sibling_mask.is_null(),
                "a local pane mask escaped into its sibling or shared underlay"
            );
            let _: () = msg_send![sibling, removeFromSuperview];
            let _: () = msg_send![sibling, release];
            assert!(CGPathContainsPoint(
                pane_path,
                std::ptr::null(),
                NSPoint::new(150.0, 200.0),
                false
            ));
            assert!(!CGPathContainsPoint(
                pane_path,
                std::ptr::null(),
                NSPoint::new(10.5, 10.5),
                false
            ));
            assert!(!con_terminal_pane_set_clip(
                host.cast(),
                10.0,
                10.0,
                280.0,
                380.0,
                7.0
            ));
            con_terminal_canvas_apply_clip(host.cast());
            let unchanged: *const c_void = msg_send![inner_mask, path];
            assert_eq!(pane_path, unchanged);
            assert!(!con_terminal_pane_set_clip(
                host.cast(),
                f64::NAN,
                0.0,
                1.0,
                1.0,
                7.0
            ));
            assert!(con_terminal_canvas_set_clip(
                parent_ptr, 20.0, 0.0, 580.0, 400.0, 8.0
            ));
            con_terminal_canvas_apply_clip(host.cast());
            let outer_path: *const c_void = msg_send![pane_mask, path];
            let pane_path: *const c_void = msg_send![inner_mask, path];
            assert!(!CGPathContainsPoint(
                outer_path,
                std::ptr::null(),
                NSPoint::new(15.0, 200.0),
                false
            ));
            assert!(CGPathContainsPoint(
                pane_path,
                std::ptr::null(),
                NSPoint::new(15.0, 200.0),
                false
            ));
            assert!(con_terminal_canvas_set_clip(
                parent_ptr, 0.0, 0.0, 600.0, 400.0, 0.0
            ));
            assert!(con_terminal_pane_set_clip(
                host.cast(),
                10.0,
                10.0,
                280.0,
                380.0,
                0.0
            ));
            con_terminal_canvas_apply_clip(host.cast());
            let rectangular_mask: *mut objc::runtime::Object = msg_send![layer, mask];
            assert!(
                !rectangular_mask.is_null(),
                "zero radius lost pane clipping"
            );
            let inner_mask: *mut objc::runtime::Object = msg_send![rectangular_mask, mask];
            assert!(!inner_mask.is_null());
            let rectangular_path: *const c_void = msg_send![inner_mask, path];
            assert!(CGPathContainsPoint(
                rectangular_path,
                std::ptr::null(),
                NSPoint::new(10.1, 10.1),
                false
            ));
            assert!(!CGPathContainsPoint(
                rectangular_path,
                std::ptr::null(),
                NSPoint::new(5.0, 200.0),
                false
            ));
            assert!(con_terminal_pane_set_clip(
                host.cast(),
                0.0,
                0.0,
                600.0,
                400.0,
                0.0
            ));
            assert!(!con_terminal_canvas_set_clip(
                parent_ptr, 0.0, 0.0, 600.0, 400.0, 0.0
            ));
            con_terminal_canvas_apply_clip(host.cast());
            let reset_mask: *mut objc::runtime::Object = msg_send![layer, mask];
            assert!(reset_mask.is_null(), "zoom retained the outgoing pane mask");
            assert!(con_terminal_canvas_set_clip(
                parent_ptr, 0.0, 0.0, 1.0, 1.0, 9.0
            ));
            con_terminal_canvas_apply_clip(host.cast());
            assert!(con_terminal_canvas_set_clip(
                parent_ptr, 0.0, 0.0, 0.0, 0.0, 9.0
            ));
            con_terminal_canvas_apply_clip(host.cast());
            assert_eq!(
                con_terminal_canvas_header_corners(parent_ptr, 0.0, 0.0, 0.0),
                0
            );
            let _: () = msg_send![host, release];
            let _: () = msg_send![parent, release];
        }
    }
}
