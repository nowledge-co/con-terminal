// A gesture-scoped observer, not a global event tap. AppKit keeps mouse tracking
// outside the source view; pressedMouseButtons also covers a missed final event.
#import <AppKit/AppKit.h>
#import <QuartzCore/QuartzCore.h>

@interface ConSessionDragCapture : NSObject
@property(nonatomic, weak) NSView *source;
@property(nonatomic, strong) id monitor;
@property(nonatomic, strong) NSPanel *preview;
@property(nonatomic) NSPoint releasePoint;
@property(nonatomic) BOOL released;
@property(nonatomic) BOOL cancelled;
@end
@implementation ConSessionDragCapture
@end

// All entry points run on GPUI's main thread. No callback re-enters Rust/GPUI.
void *con_session_drag_begin(void *view_ptr, const char *title,
                            double br, double bg, double bb,
                            double fr, double fg, double fb) {
    NSCAssert(NSThread.isMainThread, @"session drag must be main-thread owned");
    NSView *view = (__bridge NSView *)view_ptr;
    if (view.window == nil || (NSEvent.pressedMouseButtons & 1) == 0) return NULL;
    ConSessionDragCapture *capture = [ConSessionDragCapture new];
    capture.source = view;
    NSPanel *panel = [[NSPanel alloc] initWithContentRect:NSMakeRect(0, 0, 220, 58)
                                              styleMask:NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel
                                                backing:NSBackingStoreBuffered defer:NO];
    panel.opaque = NO;
    panel.backgroundColor = NSColor.clearColor;
    panel.hasShadow = NO;
    panel.releasedWhenClosed = NO;
    panel.ignoresMouseEvents = YES;
    panel.hidesOnDeactivate = YES;
    panel.level = NSFloatingWindowLevel;
    panel.collectionBehavior = NSWindowCollectionBehaviorCanJoinAllSpaces |
                               NSWindowCollectionBehaviorFullScreenAuxiliary |
                               NSWindowCollectionBehaviorIgnoresCycle;
    NSView *content = panel.contentView;
    content.wantsLayer = YES;
    content.layer.cornerRadius = 10;
    content.layer.masksToBounds = YES;
    content.layer.backgroundColor = [NSColor colorWithSRGBRed:br green:bg blue:bb alpha:1].CGColor;
    NSColor *foreground = [NSColor colorWithSRGBRed:fr green:fg blue:fb alpha:1];
    NSTextField *name = [NSTextField labelWithString:[NSString stringWithUTF8String:title] ?: @"Terminal"];
    name.frame = NSMakeRect(12, 30, 196, 17);
    name.font = [NSFont systemFontOfSize:12 weight:NSFontWeightMedium];
    name.textColor = foreground;
    name.lineBreakMode = NSLineBreakByTruncatingMiddle;
    [content addSubview:name];
    NSTextField *hint = [NSTextField labelWithString:@"New window · Esc to cancel"];
    hint.frame = NSMakeRect(12, 11, 196, 15);
    hint.font = [NSFont systemFontOfSize:11];
    hint.textColor = [foreground colorWithAlphaComponent:0.72];
    [content addSubview:hint];
    capture.preview = panel;
    __weak ConSessionDragCapture *weak = capture;
    capture.monitor = [NSEvent addLocalMonitorForEventsMatchingMask:NSEventMaskLeftMouseUp | NSEventMaskKeyDown
                                                         handler:^NSEvent *(NSEvent *event) {
        ConSessionDragCapture *state = weak;
        if (state == nil) return event;
        if (event.type == NSEventTypeLeftMouseUp && !state.released) {
            state.releasePoint = event.window != nil
                ? [event.window convertPointToScreen:event.locationInWindow] : event.locationInWindow;
            state.released = YES;
        } else if (event.type == NSEventTypeKeyDown && event.keyCode == 53) {
            state.cancelled = YES;
            return nil; // Escape belongs to the drag, never the agent terminal.
        }
        return event;
    }];
    return (__bridge_retained void *)capture;
}

// 0 tracking, 1 released, 2 cancelled. Coordinates are in the source content
// view's top-left coordinate system, independent of screen origin and scale.
int con_session_drag_sample(void *ptr, double *x, double *y, bool *outside) {
    NSCAssert(NSThread.isMainThread, @"session drag must be main-thread owned");
    ConSessionDragCapture *capture = (__bridge ConSessionDragCapture *)ptr;
    NSView *source = capture.source;
    if (source.window == nil || !source.window.visible || !NSApp.active) capture.cancelled = YES;
    if (capture.cancelled) {
        [capture.preview orderOut:nil];
        return 2;
    }
    NSPoint screen = capture.released ? capture.releasePoint : NSEvent.mouseLocation;
    NSPoint local = [source convertPoint:[source.window convertPointFromScreen:screen] fromView:nil];
    *x = local.x - NSMinX(source.bounds);
    *y = source.isFlipped ? local.y - NSMinY(source.bounds) : NSMaxY(source.bounds) - local.y;
    // The window frame (rather than a terminal or root hitbox) is the ownership
    // boundary. A small hysteresis avoids accidental tear-off at its edge.
    *outside = !NSPointInRect(screen, NSInsetRect(source.window.frame, -12, -12));
    if (!capture.released && (NSEvent.pressedMouseButtons & 1) == 0) {
        capture.releasePoint = screen;
        capture.released = YES;
    }
    if (*outside && !capture.released) {
        NSRect frame = capture.preview.frame;
        frame.origin = NSMakePoint(screen.x + 16, screen.y - frame.size.height - 16);
        for (NSScreen *display in NSScreen.screens) {
            if (!NSPointInRect(screen, display.frame)) continue;
            NSRect visible = display.visibleFrame;
            frame.origin.x = MAX(NSMinX(visible), MIN(frame.origin.x, NSMaxX(visible) - frame.size.width));
            frame.origin.y = MAX(NSMinY(visible), MIN(frame.origin.y, NSMaxY(visible) - frame.size.height));
            break;
        }
        [capture.preview setFrame:frame display:NO];
        [capture.preview orderFront:nil];
    } else {
        [capture.preview orderOut:nil];
    }
    return capture.released ? 1 : 0;
}

void con_session_drag_end(void *ptr) {
    NSCAssert(NSThread.isMainThread, @"session drag must be main-thread owned");
    ConSessionDragCapture *capture = (__bridge_transfer ConSessionDragCapture *)ptr;
    if (capture.monitor != nil) [NSEvent removeMonitor:capture.monitor];
    capture.monitor = nil;
    [capture.preview orderOut:nil];
    [capture.preview close];
}
