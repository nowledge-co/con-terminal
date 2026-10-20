#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>
#include <math.h>
#include <objc/message.h>
#include <objc/runtime.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>

extern void ghostty_surface_set_content_scale(void *surface, double x, double y);
extern void ghostty_surface_set_display_id(void *surface, uint32_t display_id);
// Ghostty's API name is historical; the boolean is true when the surface is visible.
extern void ghostty_surface_set_occlusion(void *surface, bool visible);
extern void ghostty_surface_set_size(void *surface, uint32_t width, uint32_t height);

static char kConGhosttySurfaceBackingObserverKey;
static const int64_t kConDisplayWakeQuietPeriodMilliseconds = 250;

static char kConTerminalCanvasKey;
static char kConTerminalCanvasMaskKey;

typedef struct {
    CGFloat topLeft, topRight, bottomRight, bottomLeft;
} ConTerminalCornerRadii;

static bool con_terminal_radii_equal(ConTerminalCornerRadii a, ConTerminalCornerRadii b) {
    return a.topLeft == b.topLeft && a.topRight == b.topRight &&
        a.bottomRight == b.bottomRight && a.bottomLeft == b.bottomLeft;
}

static CGFloat con_terminal_max_radius(ConTerminalCornerRadii radii) {
    return fmax(fmax(radii.topLeft, radii.topRight), fmax(radii.bottomRight, radii.bottomLeft));
}

// The same circular Bezier as GPUI's exterior cutouts, in AppKit's y-up space.
static CGPathRef con_terminal_corner_path(CGRect rect, ConTerminalCornerRadii radii) {
    CGFloat x = CGRectGetMinX(rect), y = CGRectGetMinY(rect);
    CGFloat right = CGRectGetMaxX(rect), top = CGRectGetMaxY(rect);
    CGFloat tl = radii.topLeft, tr = radii.topRight;
    CGFloat br = radii.bottomRight, bl = radii.bottomLeft;
    const CGFloat c = 0.5522848;
    CGMutablePathRef path = CGPathCreateMutable();
    CGPathMoveToPoint(path, NULL, x + bl, y);
    CGPathAddLineToPoint(path, NULL, right - br, y);
    CGPathAddCurveToPoint(path, NULL, right - br + c * br, y, right, y + br - c * br, right, y + br);
    CGPathAddLineToPoint(path, NULL, right, top - tr);
    CGPathAddCurveToPoint(path, NULL, right, top - tr + c * tr, right - tr + c * tr, top, right - tr, top);
    CGPathAddLineToPoint(path, NULL, x + tl, top);
    CGPathAddCurveToPoint(path, NULL, x + tl - c * tl, top, x, top - tl + c * tl, x, top - tl);
    CGPathAddLineToPoint(path, NULL, x, y + bl);
    CGPathAddCurveToPoint(path, NULL, x, y + bl - c * bl, x + bl - c * bl, y, x + bl, y);
    CGPathCloseSubpath(path);
    return path;
}

@interface ConTerminalCanvasClip : NSObject
@property(nonatomic) NSRect rect;
@property(nonatomic) CGFloat radius;
@property(nonatomic) uint64_t generation;
@property(nonatomic) CGFloat scale;
@property(nonatomic) ConTerminalCornerRadii radii;
@property(nonatomic, strong) NSArray<NSData *> *coverageBytes;
@property(nonatomic, strong) NSArray *coverageImages;
@property(nonatomic, strong) NSArray<NSValue *> *coverageRects;
@end
@implementation ConTerminalCanvasClip
@end

@interface ConTerminalCanvasMask : NSObject
@property(nonatomic, strong) CAShapeLayer *layer;
@property(nonatomic, strong) CAShapeLayer *paneLayer;
@property(nonatomic, weak) CALayer *targetLayer;
@property(nonatomic) NSRect rect;
@property(nonatomic) NSRect bounds;
@property(nonatomic) NSRect paneRect;
@property(nonatomic) ConTerminalCornerRadii paneRadii;
@property(nonatomic) CGFloat radius;
@property(nonatomic) CGFloat scale;
@property(nonatomic) uint64_t paneGeneration;
@end
@implementation ConTerminalCanvasMask
@end

// Material only: this never receives input or becomes a first responder.
@interface ConTerminalCanvasSafeArea : NSView
@property(nonatomic, strong) NSColor *materialColor;
@property(nonatomic) uint64_t clipGeneration;
@end
@implementation ConTerminalCanvasSafeArea
- (NSView *)hitTest:(NSPoint)point { return nil; }
- (BOOL)acceptsFirstResponder { return NO; }
@end
static char kConTerminalCanvasSafeAreaKey;
static char kConTerminalPaneClipKey;
static char kConTerminalPaneHeaderKey;

// Header material lives beside the native terminal, not in GPUI's transparent
// overlay. Text and controls remain GPUI elements above this noninteractive view.
@interface ConTerminalPaneHeader : NSView
@property(nonatomic) BOOL materialVisible;
@property(nonatomic) CGFloat red;
@property(nonatomic) CGFloat green;
@property(nonatomic) CGFloat blue;
@property(nonatomic) CGFloat alpha;
@property(nonatomic) ConTerminalCornerRadii radii;
@end

void con_terminal_canvas_apply_clip(void *host_ptr);
@implementation ConTerminalPaneHeader
- (CALayer *)makeBackingLayer { return [CAShapeLayer layer]; }
- (NSView *)hitTest:(NSPoint)point { return nil; }
- (BOOL)acceptsFirstResponder { return NO; }
@end

bool con_terminal_pane_set_header(void *host_ptr, double x, double y,
                                 double width, double height, double top_left, double top_right,
                                 double red, double green, double blue, double alpha) {
    NSView *host = (__bridge NSView *)host_ptr;
    if (host == nil || host.superview == nil ||
        !isfinite(x) || !isfinite(y) || !isfinite(width) || !isfinite(height) ||
        !isfinite(top_left) || !isfinite(top_right) || !isfinite(red) || !isfinite(green) ||
        !isfinite(blue) || !isfinite(alpha) || width < 0 || height < 0 || top_left < 0 || top_right < 0) return false;
    ConTerminalPaneHeader *header = objc_getAssociatedObject(host, &kConTerminalPaneHeaderKey);
    if (height == 0 || width == 0) {
        if (header == nil) return false;
        header.materialVisible = NO;
        if (header.hidden) return false;
        header.hidden = YES;
        return true;
    }
    NSRect rect = NSMakeRect(x, host.superview.bounds.size.height - y - height, width, height);
    CGFloat limit = fmin(width * 0.5, height);
    ConTerminalCornerRadii radii = {fmin(top_left, limit), fmin(top_right, limit), 0, 0};
    ConTerminalCanvasClip *paneClip = objc_getAssociatedObject(host, &kConTerminalPaneClipKey);
    if (paneClip.coverageImages.count > 0) radii = (ConTerminalCornerRadii){0};
    CGFloat scale = host.window != nil ? host.window.backingScaleFactor : 1.0;
    CAShapeLayer *material = (CAShapeLayer *)header.layer;
    if (header != nil && header.superview == host.superview &&
        NSEqualRects(header.frame, rect) && con_terminal_radii_equal(header.radii, radii) &&
        material.contentsScale == scale && material.path != NULL && material.fillColor != NULL &&
        header.red == red && header.green == green && header.blue == blue && header.alpha == alpha &&
        header.materialVisible && header.hidden == host.hidden) {
        objc_setAssociatedObject(header, &kConTerminalPaneClipKey, paneClip, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        con_terminal_canvas_apply_clip((__bridge void *)header);
        return false;
    }
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    if (header == nil) {
        header = [[ConTerminalPaneHeader alloc] initWithFrame:rect];
        header.wantsLayer = YES;
        header.autoresizingMask = NSViewNotSizable;
        objc_setAssociatedObject(host, &kConTerminalPaneHeaderKey, header, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    if (header.superview != host.superview) {
        [header removeFromSuperview];
        [host.superview addSubview:header positioned:NSWindowAbove relativeTo:host];
    }
    header.frame = rect;
    material = (CAShapeLayer *)header.layer;
    CGPathRef path = con_terminal_corner_path(NSRectToCGRect(header.bounds), radii);
    material.contentsScale = scale;
    material.path = path;
    material.fillColor = [NSColor colorWithSRGBRed:red green:green blue:blue alpha:alpha].CGColor;
    CGPathRelease(path);
    header.radii = radii;
    header.red = red;
    header.green = green;
    header.blue = blue;
    header.alpha = alpha;
    header.materialVisible = YES;
    header.hidden = host.hidden;
    objc_setAssociatedObject(header, &kConTerminalPaneClipKey, paneClip, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    [CATransaction commit];
    con_terminal_canvas_apply_clip((__bridge void *)header);
    return true;
}

void con_terminal_pane_header_set_visible(void *host_ptr, bool visible) {
    NSView *host = (__bridge NSView *)host_ptr;
    ConTerminalPaneHeader *header = objc_getAssociatedObject(host, &kConTerminalPaneHeaderKey);
    BOOL hidden = !visible || !header.materialVisible;
    if (header != nil && header.hidden != hidden) header.hidden = hidden;
}

void con_terminal_pane_remove_header(void *host_ptr) {
    NSView *host = (__bridge NSView *)host_ptr;
    ConTerminalPaneHeader *header = objc_getAssociatedObject(host, &kConTerminalPaneHeaderKey);
    [header removeFromSuperview];
    objc_setAssociatedObject(host, &kConTerminalPaneHeaderKey, nil, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
}

// Pane bounds use the same parent coordinate system as the workspace clip.
// Keeping them on each host leaves the window-wide transition underlay alone.
bool con_terminal_pane_set_clip_corners(void *host_ptr, double x, double y,
                                       double width, double height, double top_left,
                                       double top_right, double bottom_right, double bottom_left) {
    NSView *host = (__bridge NSView *)host_ptr;
    if (host == nil || host.superview == nil || !isfinite(x) || !isfinite(y) ||
        !isfinite(width) || !isfinite(height) || !isfinite(top_left) || !isfinite(top_right) ||
        !isfinite(bottom_right) || !isfinite(bottom_left) || width < 0 || height < 0 ||
        top_left < 0 || top_right < 0 || bottom_right < 0 || bottom_left < 0) return false;
    NSRect rect = NSMakeRect(x, host.superview.bounds.size.height - y - height, width, height);
    CGFloat limit = fmin(width, height) * 0.5;
    ConTerminalCornerRadii radii = {fmin(top_left, limit), fmin(top_right, limit),
        fmin(bottom_right, limit), fmin(bottom_left, limit)};
    ConTerminalCanvasClip *clip = objc_getAssociatedObject(host, &kConTerminalPaneClipKey);
    if (clip != nil && NSEqualRects(clip.rect, rect) && con_terminal_radii_equal(clip.radii, radii)) return false;
    if (clip == nil) {
        clip = [ConTerminalCanvasClip new];
        objc_setAssociatedObject(host, &kConTerminalPaneClipKey, clip, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    }
    clip.rect = rect;
    clip.radii = radii;
    return true;
}

// Uniform entry point retained for native geometry fixtures.
bool con_terminal_pane_set_clip(void *host_ptr, double x, double y,
                                double width, double height, double radius) {
    return con_terminal_pane_set_clip_corners(host_ptr, x, y, width, height,
        radius, radius, radius, radius);
}

typedef struct {
    double x, y, width, height;
    size_t side;
    const uint8_t *bytes;
} ConNativeCornerTile;

// Only radius/material changes upload tiny tiles. Their coordinates are in the
// shared parent, so host and header consume one coverage field.
bool con_terminal_pane_set_coverage(void *host_ptr, const ConNativeCornerTile *tiles, size_t count) {
    NSView *host = (__bridge NSView *)host_ptr;
    ConTerminalCanvasClip *clip = objc_getAssociatedObject(host, &kConTerminalPaneClipKey);
    if (clip == nil || count > 4 || (count > 0 && tiles == NULL)) return false;
    BOOL unchanged = clip.coverageBytes != nil && clip.coverageBytes.count == count;
    BOOL sameRaster = unchanged;
    for (size_t i = 0; i < count; ++i) {
        ConNativeCornerTile t = tiles[i];
        if (t.bytes == NULL || t.side == 0 || t.side > 256 || !isfinite(t.x) || !isfinite(t.y) ||
            !isfinite(t.width) || !isfinite(t.height) || t.width <= 0 || t.height <= 0) return false;
        NSRect rect = NSMakeRect(t.x, host.superview.bounds.size.height - t.y - t.height, t.width, t.height);
        if (sameRaster) {
            NSData *old = clip.coverageBytes[i];
            sameRaster = old.length == t.side * t.side * 4 && memcmp(old.bytes, t.bytes, old.length) == 0;
        }
        unchanged = unchanged && sameRaster && NSEqualRects([clip.coverageRects[i] rectValue], rect);
    }
    if (unchanged) return false;
    if (sameRaster) {
        NSMutableArray *rects = [NSMutableArray arrayWithCapacity:count];
        for (size_t i = 0; i < count; ++i) {
            ConNativeCornerTile t = tiles[i];
            [rects addObject:[NSValue valueWithRect:NSMakeRect(t.x,
                host.superview.bounds.size.height - t.y - t.height, t.width, t.height)]];
        }
        clip.coverageRects = rects;
        clip.generation += 1;
        return true;
    }
    NSMutableArray *bytes = [NSMutableArray arrayWithCapacity:count];
    NSMutableArray *rects = [NSMutableArray arrayWithCapacity:count];
    for (size_t i = 0; i < count; ++i) {
        ConNativeCornerTile t = tiles[i];
        if (t.bytes == NULL || t.side == 0 || t.side > 256 || !isfinite(t.x) || !isfinite(t.y) ||
            !isfinite(t.width) || !isfinite(t.height) || t.width <= 0 || t.height <= 0) return false;
        [bytes addObject:[NSData dataWithBytes:t.bytes length:t.side * t.side * 4]];
        NSRect r = NSMakeRect(t.x, host.superview.bounds.size.height - t.y - t.height, t.width, t.height);
        [rects addObject:[NSValue valueWithRect:r]];
    }
    NSMutableArray *images = [NSMutableArray arrayWithCapacity:count];
    CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
    for (size_t i = 0; i < count; ++i) {
        CGDataProviderRef provider = CGDataProviderCreateWithCFData((__bridge CFDataRef)bytes[i]);
        size_t side = tiles[i].side;
        CGImageRef image = CGImageCreate(side, side, 8, 32, side * 4, space,
            kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big, provider, NULL, false, kCGRenderingIntentDefault);
        CGDataProviderRelease(provider);
        if (image == NULL) { CGColorSpaceRelease(space); return false; }
        [images addObject:CFBridgingRelease(image)];
    }
    CGColorSpaceRelease(space);
    clip.coverageBytes = bytes;
    clip.coverageRects = rects;
    clip.coverageImages = images;
    clip.generation += 1;
    return true;
}

static NSView *con_terminal_parent(NSView *gpui) {
    return gpui.superview ?: gpui;
}

bool con_terminal_canvas_set_clip(void *gpui_ptr, double x, double y,
                                  double width, double height, double radius) {
    NSView *parent = con_terminal_parent((__bridge NSView *)gpui_ptr);
    if (parent == nil) return false;
    if (!isfinite(x) || !isfinite(y) || !isfinite(width) || !isfinite(height) ||
        !isfinite(radius) || width < 0 || height < 0) return false;
    radius = fmax(0.0, fmin(radius, fmin(width, height) * 0.5));
    NSView *gpui = (__bridge NSView *)gpui_ptr;
    CGFloat scale = gpui.window != nil ? gpui.window.backingScaleFactor : 1.0;
    if (!isfinite(scale) || scale <= 0.0) scale = 1.0;
    NSRect rect = NSMakeRect(x, parent.bounds.size.height - y - height, width, height);
    ConTerminalCanvasClip *clip = objc_getAssociatedObject(parent, &kConTerminalCanvasKey);
    if (clip == nil) {
        clip = [ConTerminalCanvasClip new];
        objc_setAssociatedObject(parent, &kConTerminalCanvasKey, clip, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    } else if (NSEqualRects(clip.rect, rect) && clip.radius == radius && clip.scale == scale) {
        return false;
    }
    clip.rect = rect;
    clip.radius = radius;
    clip.scale = scale;
    clip.generation += 1;
    return true;
}

double con_terminal_canvas_radius(void *gpui_ptr) {
    NSView *parent = con_terminal_parent((__bridge NSView *)gpui_ptr);
    ConTerminalCanvasClip *clip = objc_getAssociatedObject(parent, &kConTerminalCanvasKey);
    return clip.radius;
}

uint64_t con_terminal_canvas_generation(void *host_ptr) {
    NSView *host = (__bridge NSView *)host_ptr;
    ConTerminalCanvasClip *clip = objc_getAssociatedObject(host.superview, &kConTerminalCanvasKey);
    return clip.generation;
}

void con_terminal_canvas_apply_clip(void *host_ptr) {
    NSView *host = (__bridge NSView *)host_ptr;
    ConTerminalCanvasClip *clip = objc_getAssociatedObject(host.superview, &kConTerminalCanvasKey);
    if (clip == nil) return;
    ConTerminalCanvasClip *pane = objc_getAssociatedObject(host, &kConTerminalPaneClipKey);
    NSRect rect = [host convertRect:clip.rect fromView:host.superview];
    ConTerminalCornerRadii paneRadii = pane != nil ? pane.radii : (ConTerminalCornerRadii){0};
    NSRect paneRect = pane != nil ? [host convertRect:pane.rect fromView:host.superview] : NSZeroRect;
    BOOL paneContainsHost = pane == nil || (con_terminal_max_radius(paneRadii) <= 0 && NSContainsRect(paneRect, host.bounds));
    BOOL clean = clip.radius <= 0 && NSContainsRect(rect, host.bounds) && paneContainsHost;
    BOOL paneMaskNeeded = !paneContainsHost;
    // A rectangular pane clip must still constrain reused/resizing hosts.
    // A Metal-backed descendant does not give NSScrollView a backing layer.
    if (host.layer == nil && !clean) host.wantsLayer = YES;
    if (host.layer == nil) return;
    CGFloat scale = host.window != nil ? host.window.backingScaleFactor : host.layer.contentsScale;
    if (!isfinite(scale) || scale <= 0.0) scale = 1.0;
    ConTerminalCanvasMask *mask = objc_getAssociatedObject(host, &kConTerminalCanvasMaskKey);
    if (mask == nil) {
        mask = [ConTerminalCanvasMask new];
        mask.layer = [CAShapeLayer layer];
        mask.layer.fillColor = NSColor.blackColor.CGColor;
        mask.paneLayer = [CAShapeLayer layer];
        mask.paneLayer.fillColor = NSColor.blackColor.CGColor;
        objc_setAssociatedObject(host, &kConTerminalCanvasMaskKey, mask, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    } else if (NSEqualRects(mask.rect, rect) && NSEqualRects(mask.bounds, host.bounds) &&
               mask.radius == clip.radius && mask.scale == scale && mask.paneGeneration == pane.generation &&
               con_terminal_radii_equal(mask.paneRadii, paneRadii) && NSEqualRects(mask.paneRect, paneRect)) {
        if (clean ? host.layer.mask != nil : (host.layer.mask != mask.layer ||
                mask.targetLayer != host.layer || mask.layer.mask != (paneMaskNeeded ? mask.paneLayer : nil))) {
            // AppKit may replace a backing layer without changing view bounds.
            // Rebind the cached paths instead of treating geometry as authority.
            [CATransaction begin];
            [CATransaction setDisableActions:YES];
            if (mask.targetLayer != nil && mask.targetLayer != host.layer && mask.targetLayer.mask == mask.layer) {
                mask.targetLayer.mask = nil;
            }
            mask.targetLayer = host.layer;
            mask.layer.mask = paneMaskNeeded ? mask.paneLayer : nil;
            host.layer.mask = clean ? nil : mask.layer;
            [CATransaction commit];
        }
        return;
    }
    mask.rect = rect;
    mask.bounds = host.bounds;
    mask.radius = clip.radius;
    mask.scale = scale;
    mask.paneRadii = paneRadii;
    mask.paneRect = paneRect;
    mask.paneGeneration = pane.generation;
    if (clean) {
        // Clean mode needs no extra offscreen mask for an already-contained host.
        [CATransaction begin];
        [CATransaction setDisableActions:YES];
        host.layer.mask = nil;
        [CATransaction commit];
        return;
    }
    CGPathRef path = CGPathCreateWithRoundedRect(NSRectToCGRect(rect), clip.radius, clip.radius, NULL);
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    if (mask.targetLayer != nil && mask.targetLayer != host.layer && mask.targetLayer.mask == mask.layer) {
        mask.targetLayer.mask = nil;
    }
    mask.targetLayer = host.layer;
    mask.layer.frame = host.bounds;
    mask.layer.contentsScale = scale;
    mask.layer.path = path;
    if (paneMaskNeeded) {
        CGPathRef panePath = con_terminal_corner_path(NSRectToCGRect(paneRect), paneRadii);
        mask.paneLayer.sublayers = nil;
        mask.paneLayer.fillRule = kCAFillRuleNonZero;
        if (pane.coverageImages.count > 0) {
            CGPathRelease(panePath);
            CGMutablePathRef sharedPath = CGPathCreateMutable();
            CGPathAddRect(sharedPath, NULL, NSRectToCGRect(paneRect));
            for (NSUInteger i = 0; i < pane.coverageImages.count; ++i) {
                NSRect tileRect = [host convertRect:[pane.coverageRects[i] rectValue] fromView:host.superview];
                CGPathAddRect(sharedPath, NULL, NSRectToCGRect(tileRect));
                CALayer *tile = [CALayer layer];
                tile.frame = tileRect;
                tile.contents = pane.coverageImages[i];
                tile.contentsScale = scale;
                tile.minificationFilter = kCAFilterNearest;
                tile.magnificationFilter = kCAFilterNearest;
                tile.actions = @{ @"contents": [NSNull null], @"bounds": [NSNull null], @"position": [NSNull null] };
                [mask.paneLayer addSublayer:tile];
            }
            panePath = sharedPath;
            mask.paneLayer.fillRule = kCAFillRuleEvenOdd;
        }
        mask.paneLayer.frame = host.bounds;
        mask.paneLayer.contentsScale = scale;
        mask.paneLayer.path = panePath;
        mask.layer.mask = mask.paneLayer;
        CGPathRelease(panePath);
    } else {
        mask.layer.mask = nil;
    }
    host.layer.mask = mask.layer;
    [CATransaction commit];
    CGPathRelease(path);
}

bool con_terminal_canvas_clip_is_attached(void *host_ptr) {
    NSView *host = (__bridge NSView *)host_ptr;
    ConTerminalCanvasClip *clip = objc_getAssociatedObject(host.superview, &kConTerminalCanvasKey);
    ConTerminalCanvasClip *pane = objc_getAssociatedObject(host, &kConTerminalPaneClipKey);
    if (clip == nil || host.layer == nil) return false;
    NSRect rect = [host convertRect:clip.rect fromView:host.superview];
    NSRect paneRect = pane != nil ? [host convertRect:pane.rect fromView:host.superview] : NSZeroRect;
    BOOL paneContainsHost = pane == nil || (con_terminal_max_radius(pane.radii) <= 0 && NSContainsRect(paneRect, host.bounds));
    if (clip.radius <= 0.0 && NSContainsRect(rect, host.bounds) && paneContainsHost) {
        return host.layer.mask == nil;
    }
    ConTerminalCanvasMask *mask = objc_getAssociatedObject(host, &kConTerminalCanvasMaskKey);
    return mask != nil && mask.targetLayer == host.layer && host.layer.mask == mask.layer &&
        mask.layer.mask == (!paneContainsHost ? mask.paneLayer : nil);
}

bool con_terminal_canvas_set_safe_area(void *gpui_ptr, double height,
                                      double red, double green, double blue, double alpha) {
    NSView *gpui = (__bridge NSView *)gpui_ptr;
    NSView *parent = con_terminal_parent(gpui);
    ConTerminalCanvasClip *clip = objc_getAssociatedObject(parent, &kConTerminalCanvasKey);
    if (parent == nil || clip == nil || !isfinite(height) || !isfinite(red) ||
        !isfinite(green) || !isfinite(blue) || !isfinite(alpha)) return false;
    height = fmax(0, fmin(height, clip.rect.size.height));
    ConTerminalCanvasSafeArea *view = objc_getAssociatedObject(parent, &kConTerminalCanvasSafeAreaKey);
    if (height == 0) {
        if (view == nil || view.hidden) return false;
        view.hidden = YES;
        return true;
    }
    NSRect rect = NSMakeRect(clip.rect.origin.x, clip.rect.origin.y,
                            clip.rect.size.width, height);
    NSColor *color = [NSColor colorWithSRGBRed:red green:green blue:blue alpha:alpha];
    if (view == nil) {
        view = [[ConTerminalCanvasSafeArea alloc] initWithFrame:rect];
        view.wantsLayer = YES;
        [parent addSubview:view positioned:NSWindowBelow relativeTo:gpui];
        objc_setAssociatedObject(parent, &kConTerminalCanvasSafeAreaKey, view,
                                 OBJC_ASSOCIATION_RETAIN_NONATOMIC);
    } else if (!view.hidden && NSEqualRects(view.frame, rect) &&
               [view.materialColor isEqual:color] && view.clipGeneration == clip.generation) {
        return false;
    }
    [CATransaction begin];
    [CATransaction setDisableActions:YES];
    view.frame = rect;
    view.layer.backgroundColor = color.CGColor;
    view.hidden = NO;
    con_terminal_canvas_apply_clip((__bridge void *)view);
    [CATransaction commit];
    view.materialColor = color;
    view.clipGeneration = clip.generation;
    return true;
}

// AppKit's window corner radius changes across macOS releases; workspace cards
// beside a window corner derive concentric curves from it. Fullscreen and
// unknown releases report no window corner.
double con_window_corner_radius(void *gpui_ptr) {
    NSWindow *window = ((__bridge NSView *)gpui_ptr).window;
    if (window == nil || (window.styleMask & NSWindowStyleMaskFullScreen) != 0) return 0.0;
    SEL selector = NSSelectorFromString(@"_cornerRadius");
    if (![window respondsToSelector:selector]) return 0.0;
    CGFloat radius = ((CGFloat (*)(id, SEL))objc_msgSend)(window, selector);
    return isfinite(radius) && radius > 0.0 ? radius : 0.0;
}

uint8_t con_terminal_canvas_header_corners(void *gpui_ptr, double x, double y,
                                          double width) {
    NSView *parent = con_terminal_parent((__bridge NSView *)gpui_ptr);
    ConTerminalCanvasClip *clip = objc_getAssociatedObject(parent, &kConTerminalCanvasKey);
    if (clip == nil || clip.radius <= 0.0) return 0;
    CGFloat top = parent.bounds.size.height - y;
    if (fabs(top - NSMaxY(clip.rect)) > 0.25) return 0;
    uint8_t corners = 0;
    if (fabs(x - NSMinX(clip.rect)) < 0.25) corners |= 1;
    if (fabs(x + width - NSMaxX(clip.rect)) < 0.25) corners |= 2;
    return corners;
}

// Only read the native event while its terminal window is handling keyDown.
// Remote text injection can carry Unicode unrelated to the virtual keycode.
const char *con_ghostty_surface_current_key_text(void *view_ptr, size_t *text_length) {
    NSView *view = (__bridge NSView *)view_ptr;
    NSEvent *event = NSApp.currentEvent;
    if (view.window == nil || event.window != view.window || event.type != NSEventTypeKeyDown) {
        return NULL;
    }
    if ((event.modifierFlags & NSEventModifierFlagDeviceIndependentFlagsMask) != 0) {
        return NULL;
    }
    NSString *text = event.characters;
    *text_length = [text lengthOfBytesUsingEncoding:NSUTF8StringEncoding];
    return text.UTF8String;
}

static double con_valid_scale(double scale, double fallback) {
    if (!isfinite(scale) || scale <= 0.0) {
        return fallback > 0.0 ? fallback : 1.0;
    }
    return scale;
}

static uint32_t con_ghostty_display_id_for_window(NSWindow *window) {
    NSScreen *screen = window.screen;
    if (screen == nil) {
        return 0;
    }

    NSNumber *number = screen.deviceDescription[@"NSScreenNumber"];
    return number == nil ? 0 : number.unsignedIntValue;
}

bool con_ghostty_surface_sync_backing(
    void *view_ptr,
    void *surface_ptr,
    double logical_width,
    double logical_height,
    double fallback_scale,
    double *out_scale_x,
    double *out_scale_y,
    uint32_t *out_width,
    uint32_t *out_height
) {
    NSView *view = (__bridge NSView *)view_ptr;
    if (view == nil || surface_ptr == NULL) {
        return false;
    }

    fallback_scale = con_valid_scale(fallback_scale, 1.0);

    NSWindow *window = view.window;
    double window_scale = fallback_scale;
    uint32_t display_id = 0;
    if (window != nil) {
        window_scale = con_valid_scale(window.backingScaleFactor, fallback_scale);
        display_id = con_ghostty_display_id_for_window(window);
    }

    NSSize unit_backing = [view convertSizeToBacking:NSMakeSize(1.0, 1.0)];
    double scale_x = con_valid_scale(unit_backing.width, window_scale);
    double scale_y = con_valid_scale(unit_backing.height, window_scale);

    CALayer *layer = view.layer;
    if (layer != nil) {
        [CATransaction begin];
        [CATransaction setDisableActions:YES];
        layer.contentsScale = scale_y;
        [CATransaction commit];
    }

    NSSize logical_size = NSMakeSize(fmax(logical_width, 1.0), fmax(logical_height, 1.0));
    NSSize backing_size = [view convertSizeToBacking:logical_size];
    uint32_t width = (uint32_t)llround(fmax(backing_size.width, 1.0));
    uint32_t height = (uint32_t)llround(fmax(backing_size.height, 1.0));

    if (display_id != 0) {
        ghostty_surface_set_display_id(surface_ptr, display_id);
    }
    ghostty_surface_set_content_scale(surface_ptr, scale_x, scale_y);
    ghostty_surface_set_size(surface_ptr, width, height);

    if (out_scale_x != NULL) {
        *out_scale_x = scale_x;
    }
    if (out_scale_y != NULL) {
        *out_scale_y = scale_y;
    }
    if (out_width != NULL) {
        *out_width = width;
    }
    if (out_height != NULL) {
        *out_height = height;
    }

    return true;
}

@interface ConGhosttySurfaceBackingObserver : NSObject
@property(nonatomic, weak) NSView *view;
@property(nonatomic, assign) void *surface;
@property(nonatomic, weak) NSWindow *window;
@property(nonatomic, strong) id screenObserver;
@property(nonatomic, strong) id backingObserver;
@property(nonatomic, strong) id occlusionObserver;
@property(nonatomic, strong) id displayObserver;
@property(nonatomic, strong) id willSleepObserver;
@property(nonatomic, strong) id didWakeObserver;
@property(nonatomic, assign) BOOL suspendedForSleep;
@property(nonatomic, assign) NSUInteger wakeGeneration;
@property(nonatomic, assign) BOOL surfaceVisibilityKnown;
@property(nonatomic, assign) BOOL lastSurfaceVisible;
- (void)installForView:(NSView *)view surface:(void *)surface;
- (void)invalidate;
- (void)scheduleWakeSettle;
- (void)setSurfaceVisible:(BOOL)visible;
- (void)sync;
- (void)syncOcclusion;
@end

@implementation ConGhosttySurfaceBackingObserver

- (void)dealloc {
    [self invalidate];
}

- (void)installForView:(NSView *)view surface:(void *)surface {
    NSWindow *window = view.window;
    if (_window == window && _surface == surface && _screenObserver != nil &&
        _backingObserver != nil && _occlusionObserver != nil &&
        _displayObserver != nil && _willSleepObserver != nil &&
        _didWakeObserver != nil) {
        return;
    }

    [self invalidate];
    _view = view;
    _surface = surface;
    _window = window;
    _suspendedForSleep = NO;
    _wakeGeneration += 1;
    _surfaceVisibilityKnown = NO;

    if (window == nil || surface == NULL) {
        return;
    }

    __weak ConGhosttySurfaceBackingObserver *weakSelf = self;
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    _screenObserver = [center addObserverForName:NSWindowDidChangeScreenNotification
                                          object:window
                                           queue:NSOperationQueue.mainQueue
                                      usingBlock:^(__unused NSNotification *notification) {
        ConGhosttySurfaceBackingObserver *observer = weakSelf;
        if (observer.suspendedForSleep) {
            [observer scheduleWakeSettle];
            return;
        }
        [observer sync];
        dispatch_async(dispatch_get_main_queue(), ^{
            [observer sync];
        });
    }];
    _backingObserver = [center addObserverForName:NSWindowDidChangeBackingPropertiesNotification
                                           object:window
                                            queue:NSOperationQueue.mainQueue
                                       usingBlock:^(__unused NSNotification *notification) {
        ConGhosttySurfaceBackingObserver *observer = weakSelf;
        if (observer.suspendedForSleep) {
            [observer scheduleWakeSettle];
        } else {
            [observer sync];
        }
    }];
    _occlusionObserver = [center addObserverForName:NSWindowDidChangeOcclusionStateNotification
                                             object:window
                                              queue:NSOperationQueue.mainQueue
                                         usingBlock:^(__unused NSNotification *notification) {
        [weakSelf syncOcclusion];
    }];
    _displayObserver = [center addObserverForName:NSApplicationDidChangeScreenParametersNotification
                                           object:nil
                                            queue:NSOperationQueue.mainQueue
                                       usingBlock:^(__unused NSNotification *notification) {
        ConGhosttySurfaceBackingObserver *observer = weakSelf;
        if (observer.suspendedForSleep) {
            [observer scheduleWakeSettle];
        }
    }];

    NSNotificationCenter *workspaceCenter = NSWorkspace.sharedWorkspace.notificationCenter;
    _willSleepObserver = [workspaceCenter addObserverForName:NSWorkspaceWillSleepNotification
                                                       object:nil
                                                        queue:NSOperationQueue.mainQueue
                                                   usingBlock:^(__unused NSNotification *notification) {
        ConGhosttySurfaceBackingObserver *observer = weakSelf;
        if (observer == nil || observer->_surface == NULL) {
            return;
        }

        observer.suspendedForSleep = YES;
        observer.wakeGeneration += 1;
        [observer setSurfaceVisible:NO];
    }];
    _didWakeObserver = [workspaceCenter addObserverForName:NSWorkspaceDidWakeNotification
                                                     object:nil
                                                      queue:NSOperationQueue.mainQueue
                                                 usingBlock:^(__unused NSNotification *notification) {
        ConGhosttySurfaceBackingObserver *observer = weakSelf;
        if (observer == nil) {
            return;
        }

        [observer scheduleWakeSettle];
    }];

    [self syncOcclusion];
}

- (void)invalidate {
    NSNotificationCenter *center = NSNotificationCenter.defaultCenter;
    if (_screenObserver != nil) {
        [center removeObserver:_screenObserver];
        _screenObserver = nil;
    }
    if (_backingObserver != nil) {
        [center removeObserver:_backingObserver];
        _backingObserver = nil;
    }
    if (_occlusionObserver != nil) {
        [center removeObserver:_occlusionObserver];
        _occlusionObserver = nil;
    }
    if (_displayObserver != nil) {
        [center removeObserver:_displayObserver];
        _displayObserver = nil;
    }

    NSNotificationCenter *workspaceCenter = NSWorkspace.sharedWorkspace.notificationCenter;
    if (_willSleepObserver != nil) {
        [workspaceCenter removeObserver:_willSleepObserver];
        _willSleepObserver = nil;
    }
    if (_didWakeObserver != nil) {
        [workspaceCenter removeObserver:_didWakeObserver];
        _didWakeObserver = nil;
    }
    _wakeGeneration += 1;
    _suspendedForSleep = NO;
    _surfaceVisibilityKnown = NO;
    _window = nil;
    _surface = NULL;
}

- (void)scheduleWakeSettle {
    if (!_suspendedForSleep || _surface == NULL) {
        return;
    }

    NSUInteger generation = ++_wakeGeneration;
    __weak ConGhosttySurfaceBackingObserver *weakSelf = self;
    // Screen and backing notifications can arrive in bursts while WindowServer
    // rebuilds the topology. Resume only after a quiet interval; every newer
    // notification invalidates this block through wakeGeneration.
    dispatch_after(
        dispatch_time(
            DISPATCH_TIME_NOW,
            kConDisplayWakeQuietPeriodMilliseconds * NSEC_PER_MSEC
        ),
        dispatch_get_main_queue(),
        ^{
            ConGhosttySurfaceBackingObserver *observer = weakSelf;
            if (observer == nil || observer.wakeGeneration != generation ||
                observer->_surface == NULL) {
                return;
            }

            observer.suspendedForSleep = NO;
            [observer sync];
            [observer syncOcclusion];
        }
    );
}

- (void)sync {
    NSView *view = _view;
    if (view == nil || _surface == NULL || _suspendedForSleep) {
        return;
    }

    NSSize size = view.bounds.size;
    double fallback_scale = view.window == nil ? 1.0 : view.window.backingScaleFactor;
    con_ghostty_surface_sync_backing(
        (__bridge void *)view,
        _surface,
        size.width,
        size.height,
        fallback_scale,
        NULL,
        NULL,
        NULL,
        NULL
    );
}

- (void)setSurfaceVisible:(BOOL)visible {
    if (_surface == NULL || (_surfaceVisibilityKnown && _lastSurfaceVisible == visible)) {
        return;
    }

    _surfaceVisibilityKnown = YES;
    _lastSurfaceVisible = visible;
    ghostty_surface_set_occlusion(_surface, visible);
}

- (void)syncOcclusion {
    NSView *view = _view;
    NSWindow *window = _window;
    if (view == nil || window == nil || _surface == NULL) {
        return;
    }

    BOOL visible = !_suspendedForSleep && !view.isHiddenOrHasHiddenAncestor &&
        (window.occlusionState & NSWindowOcclusionStateVisible) != 0;
    [self setSurfaceVisible:visible];
}

@end

void con_ghostty_surface_install_backing_observer(void *view_ptr, void *surface_ptr) {
    NSView *view = (__bridge NSView *)view_ptr;
    if (view == nil || surface_ptr == NULL) {
        return;
    }

    ConGhosttySurfaceBackingObserver *observer =
        objc_getAssociatedObject(view, &kConGhosttySurfaceBackingObserverKey);
    if (observer == nil) {
        observer = [[ConGhosttySurfaceBackingObserver alloc] init];
        objc_setAssociatedObject(
            view,
            &kConGhosttySurfaceBackingObserverKey,
            observer,
            OBJC_ASSOCIATION_RETAIN_NONATOMIC
        );
    }

    [observer installForView:view surface:surface_ptr];
}

void con_ghostty_surface_sync_occlusion(void *view_ptr) {
    NSView *view = (__bridge NSView *)view_ptr;
    if (view == nil) {
        return;
    }

    ConGhosttySurfaceBackingObserver *observer =
        objc_getAssociatedObject(view, &kConGhosttySurfaceBackingObserverKey);
    [observer syncOcclusion];
}

void con_ghostty_surface_remove_backing_observer(void *view_ptr) {
    NSView *view = (__bridge NSView *)view_ptr;
    if (view == nil) {
        return;
    }

    ConGhosttySurfaceBackingObserver *observer =
        objc_getAssociatedObject(view, &kConGhosttySurfaceBackingObserverKey);
    [observer invalidate];
    objc_setAssociatedObject(
        view,
        &kConGhosttySurfaceBackingObserverKey,
        nil,
        OBJC_ASSOCIATION_RETAIN_NONATOMIC
    );
}
