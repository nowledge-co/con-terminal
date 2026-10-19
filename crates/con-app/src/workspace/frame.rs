use gpui::{Bounds, Hsla, IntoElement, PathBuilder, Pixels, Styled, Window, point, px};
use std::cell::RefCell;
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
/// is the card's top clearance. Tabs keep the lateral 5pt gap below their fill.
pub(super) fn canvas_top_inset(depth: f32, horizontal_tabs: f32, caption: f32) -> f32 {
    let tabs = horizontal_tabs.clamp(0.0, 1.0);
    let tab_bottom_clearance = (super::TOP_BAR_TABS_HEIGHT - super::TOP_BAR_TAB_HEIGHT) * 0.5;
    let caption_clearance =
        (CANVAS_INSET + PANE_INSET) * (1.0 - tabs) + tab_bottom_clearance * tabs;
    (CANVAS_INSET - caption_clearance * caption.clamp(0.0, 1.0)) * depth
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
    edge(
        radius,
        foreground.opacity(if dark { 0.12 } else { 0.075 }),
        false,
        true,
    )
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
    if radius <= 0.0 || window_radius <= 0.0 {
        return gpui::Corners::all(px(radius.max(0.0)));
    }
    let left = card.left().as_f32();
    let top = card.top().as_f32();
    let right = (viewport.width - card.right()).as_f32();
    let bottom = (viewport.height - card.bottom()).as_f32();
    let corner = |x: f32, y: f32| px(radius.max(window_radius - x.max(y).max(0.0)));
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

pub(super) fn input_reserved_height(height: f32, visible: bool, progress: f32, depth: f32) -> f32 {
    // Reserve once when opening and retain the viewport through closing. A
    // second resize at the reveal endpoint makes one interaction look like two.
    if visible || progress > 0.0 {
        input_safe_height(height, 1.0, depth)
    } else {
        0.0
    }
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
        .bottom(px(INPUT_BOTTOM_INSET * depth))
        .left_0()
        .w_full()
        .h(px(height))
        .opacity(progress)
        .overflow_hidden()
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
            move |_, (strips, corners), window, _| {
                for strip in strips {
                    window.paint_quad(gpui::fill(strip, color));
                }
                for corner in corners {
                    window.paint_path(corner, color);
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
    corners: Vec<gpui::Path<Pixels>>,
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
            move |bounds, (strips, corners), window, _| {
                log::trace!(target: "con::canvas", "paint bounds={bounds:?} depth={depth} safe_height={safe_height}");
                for strip in strips {
                    if strip.size.width > px(0.0) && strip.size.height > px(0.0) {
                        window.paint_quad(gpui::fill(strip, color));
                    }
                }
                for corner in corners {
                    window.paint_path(corner, color);
                }
            },
        )
        .absolute()
        .size_full()
    }
}

fn corner_cutouts(bounds: Bounds<Pixels>, radii: gpui::Corners<Pixels>) -> Vec<gpui::Path<Pixels>> {
    let limit = bounds.size.width.min(bounds.size.height) * 0.5;
    let mut corners = Vec::with_capacity(4);
    for (origin, radius, x_sign, y_sign) in [
        (bounds.origin, radii.top_left, 1.0, 1.0),
        (bounds.top_right(), radii.top_right, -1.0, 1.0),
        (bounds.bottom_right(), radii.bottom_right, -1.0, -1.0),
        (bounds.bottom_left(), radii.bottom_left, 1.0, -1.0),
    ] {
        let r = radius.min(limit);
        if r <= px(0.0) {
            continue;
        }
        let c = r * 0.552_284_8;
        let at = |x: Pixels, y: Pixels| point(origin.x + x * x_sign, origin.y + y * y_sign);
        let mut path = PathBuilder::fill();
        path.move_to(origin);
        path.line_to(at(r, px(0.0)));
        path.cubic_bezier_to(at(px(0.0), r), at(r - c, px(0.0)), at(px(0.0), r - c));
        path.close();
        if let Ok(path) = path.build() {
            corners.push(path);
        }
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
    fn con_terminal_canvas_radius(view: *mut std::ffi::c_void) -> f64;
    fn con_window_corner_radius(view: *mut std::ffi::c_void) -> f64;
    fn con_terminal_canvas_set_clip(
        view: *mut std::ffi::c_void,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        radius: f64,
    ) -> bool;
    fn con_terminal_canvas_header_corners(
        view: *mut std::ffi::c_void,
        x: f64,
        y: f64,
        width: f64,
    ) -> u8;
}

pub(crate) fn header_background(color: Hsla) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            use raw_window_handle::HasWindowHandle;
            let (corners, radius) = HasWindowHandle::window_handle(window)
                .ok()
                .and_then(|handle| {
                    if let raw_window_handle::RawWindowHandle::AppKit(handle) = handle.as_raw() {
                        Some(unsafe {
                            (
                                con_terminal_canvas_header_corners(
                                    handle.ns_view.as_ptr(),
                                    f64::from(bounds.left()),
                                    f64::from(bounds.top()),
                                    f64::from(bounds.size.width),
                                ),
                                con_terminal_canvas_radius(handle.ns_view.as_ptr()) as f32,
                            )
                        })
                    } else {
                        None
                    }
                })
                .unwrap_or((0, 0.0));
            let mut quad = gpui::fill(bounds, color);
            quad.corner_radii.top_left = px(if corners & 1 != 0 { radius } else { 0.0 });
            quad.corner_radii.top_right = px(if corners & 2 != 0 { radius } else { 0.0 });
            window.paint_quad(quad);
        },
    )
    .absolute()
    .size_full()
    .top_0()
    .left_0()
}

pub(crate) fn pane_header_background(color: Hsla) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, (), window, _| {
            // The pane's exterior cutout owns rounding for header and terminal.
            window.paint_quad(gpui::fill(bounds, color));
        },
    )
    .absolute()
    .size_full()
    .top_0()
    .left_0()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn horizontal_tabs_do_not_double_the_top_clearance() {
        let bounds = Bounds::new(
            point(px(0.0), px(super::super::TOP_BAR_TABS_HEIGHT)),
            gpui::size(px(600.0), px(400.0)),
        );
        let tab_bottom =
            (super::super::TOP_BAR_TABS_HEIGHT + super::super::TOP_BAR_TAB_HEIGHT) * 0.5;
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
    fn input_fades_without_wiping_or_moving_the_composer(cx: &mut gpui::TestAppContext) {
        use gpui::{
            Context, InteractiveElement, IntoElement, ParentElement, Render, Window, div, size,
        };
        struct Reveal {
            progress: f32,
        }
        impl Render for Reveal {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().relative().w_full().h(px(64.0)).child(
                    input_reveal_clip(44.0, self.progress, 1.0)
                        .debug_selector(|| "input-reveal-clip".into())
                        .child(
                            div()
                                .absolute()
                                .bottom_0()
                                .left(px(10.0))
                                .w(px(300.0))
                                .h(px(44.0))
                                .debug_selector(|| "fixed-composer".into()),
                        ),
                )
            }
        }
        let (view, cx) = cx.add_window_view(|_, _| Reveal { progress: 1.0 });
        cx.simulate_resize(size(px(500.0), px(300.0)));
        let fixed = cx.debug_bounds("fixed-composer").unwrap();
        for progress in [0.25, 0.7, 0.4, 1.0] {
            view.update(cx, |view, cx| {
                view.progress = progress;
                cx.notify();
            });
            assert_eq!(cx.debug_bounds("fixed-composer").unwrap(), fixed);
            let clip = cx.debug_bounds("input-reveal-clip").unwrap();
            assert_eq!(clip.bottom(), px(62.0));
            assert_eq!(clip.size.height, px(44.0));
        }
    }

    #[gpui::test]
    fn input_viewport_stays_stable_through_reveal_reversal_and_completion(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{
            Context, InteractiveElement, IntoElement, ParentElement, Render, Window, div, size,
        };
        struct Reveal {
            progress: f32,
            visible: bool,
        }
        impl Render for Reveal {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
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
                            .h(px(input_reserved_height(
                                44.0,
                                self.visible,
                                self.progress,
                                1.0,
                            )))
                            .flex_shrink_0(),
                    )
                    .child(
                        input_reveal_clip(44.0, self.progress, 1.0).child(
                            div()
                                .absolute()
                                .bottom_0()
                                .h(px(44.0))
                                .w_full()
                                .debug_selector(|| "island-frame".into()),
                        ),
                    )
            }
        }
        let (view, cx) = cx.add_window_view(|_, _| Reveal {
            progress: 0.0,
            visible: false,
        });
        cx.simulate_resize(size(px(500.0), px(400.0)));
        let island = cx.debug_bounds("island-frame").unwrap();
        for (progress, visible, expected_output) in [
            (0.1, true, 251.0),
            (0.7, true, 251.0),
            (1.0, true, 251.0),
            (0.6, false, 251.0),
            (0.8, true, 251.0),
            (1.0, true, 251.0),
            (0.0, false, 300.0),
        ] {
            view.update(cx, |view, cx| {
                view.progress = progress;
                view.visible = visible;
                cx.notify();
            });
            assert_eq!(cx.debug_bounds("island-frame").unwrap(), island);
            assert_eq!(
                cx.debug_bounds("output-viewport").unwrap().size.height,
                px(expected_output)
            );
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
    fn corner_paths_stay_small_and_leave_the_card_interior_unpainted() {
        let bounds = Bounds::new(point(px(0.0), px(0.0)), gpui::size(px(600.0), px(400.0)));
        for scale in [1.0, 2.0] {
            let paths = corner_cutouts(bounds, gpui::Corners::all(px(PANE_RADIUS)));
            assert_eq!(paths.len(), 4);
            for path in &paths {
                assert!(path.bounds.size.width <= px(PANE_RADIUS));
                assert!(path.bounds.size.height <= px(PANE_RADIUS));
            }
            let contains = |x, y| {
                let p = point(px(x), px(y));
                paths[0].vertices.chunks_exact(3).any(|triangle| {
                    let cross = |a: gpui::Point<Pixels>, b: gpui::Point<Pixels>| {
                        (b.x - a.x).as_f32() * (p.y - a.y).as_f32()
                            - (b.y - a.y).as_f32() * (p.x - a.x).as_f32()
                    };
                    let [a, b, c] = [
                        triangle[0].xy_position,
                        triangle[1].xy_position,
                        triangle[2].xy_position,
                    ];
                    let sides = [cross(a, b), cross(b, c), cross(c, a)];
                    sides.iter().all(|s| *s >= 0.0) || sides.iter().all(|s| *s <= 0.0)
                })
            };
            assert!(contains(0.1, 0.1));
            let near = 0.25 / scale;
            assert!(
                !contains(PANE_RADIUS - near, near) && !contains(near, PANE_RADIUS - near),
                "corner cutout must meet the straight edges at {scale}x"
            );
            assert!(!contains(8.0, 8.0), "corner painted terminal interior");
        }
        assert!(corner_cutouts(bounds, gpui::Corners::default()).is_empty());
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
