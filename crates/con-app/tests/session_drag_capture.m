// Linked tests of the shipped AppKit capture. All input getters and local
// monitors are process-local fakes; nothing posts input or shows a window.
#import <objc/runtime.h>
#import "../src/objc/session_drag_trampoline.m"

static NSUInteger buttons;
static NSPoint pointer;
static BOOL active = YES;
static BOOL visible = YES;
static int ordered, removed;
static NSEvent *(^handler)(NSEvent *);
static NSUInteger fake_buttons(id self, SEL cmd) { return buttons; }
static NSPoint fake_pointer(id self, SEL cmd) { return pointer; }
static BOOL fake_active(id self, SEL cmd) { return active; }
static void fake_front(id self, SEL cmd, id sender) { ordered++; }
static void fake_out(id self, SEL cmd, id sender) {}
static id fake_monitor(id self, SEL cmd, NSEventMask mask, NSEvent *(^block)(NSEvent *)) {
    handler = [block copy]; return [NSObject new];
}
static void fake_remove(id self, SEL cmd, id monitor) { removed++; handler = nil; }
static void replace(Class cls, SEL selector, IMP implementation) {
    Method method = class_getInstanceMethod(cls, selector);
    NSCAssert(method != NULL, @"AppKit method must exist");
    class_replaceMethod(cls, selector, implementation, method_getTypeEncoding(method));
}
@interface CaptureTestWindow : NSWindow
@end
@implementation CaptureTestWindow
- (BOOL)isVisible { return visible; }
@end
static void check(BOOL condition, NSString *message) {
    if (!condition) { fprintf(stderr, "%s\n", message.UTF8String); abort(); }
}
static void *begin(NSView *view) {
    buttons = 1; active = YES; visible = YES; pointer = NSMakePoint(200, 200);
    void *capture = con_session_drag_begin((__bridge void *)view, "a/long/session/title", .94,.92,.86,.13,.12,.1);
    check(capture != NULL, @"capture starts on a real source view while pressed");
    return capture;
}
int main(void) { @autoreleasepool {
    [NSApplication sharedApplication];
    replace(object_getClass(NSEvent.class), @selector(pressedMouseButtons), (IMP)fake_buttons);
    replace(object_getClass(NSEvent.class), @selector(mouseLocation), (IMP)fake_pointer);
    replace(object_getClass(NSEvent.class), @selector(addLocalMonitorForEventsMatchingMask:handler:), (IMP)fake_monitor);
    replace(object_getClass(NSEvent.class), @selector(removeMonitor:), (IMP)fake_remove);
    replace(NSApplication.class, @selector(isActive), (IMP)fake_active);
    replace(NSPanel.class, @selector(orderFront:), (IMP)fake_front);
    replace(NSPanel.class, @selector(orderOut:), (IMP)fake_out);
    CaptureTestWindow *window = [[CaptureTestWindow alloc] initWithContentRect:NSMakeRect(100,100,400,300)
        styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
    window.releasedWhenClosed = NO;
    NSView *view = window.contentView;
    double x=0,y=0; bool outside=false;
    void *capture = begin(view);
    NSPanel *preview = ((__bridge ConSessionDragCapture *)capture).preview;
    check(preview.ignoresMouseEvents && !preview.hasShadow &&
        (preview.styleMask & NSWindowStyleMaskNonactivatingPanel) != 0,
        @"the preview cannot intercept input, activate, or add a shadow");
    check(con_session_drag_sample(capture,&x,&y,&outside)==0 && !outside && x==100 && y==200,
        @"source coordinates use points and a top-left origin");
    pointer=NSMakePoint(505,200);
    check(con_session_drag_sample(capture,&x,&y,&outside)==0 && !outside,
        @"edge jitter inside the outer margin does not detach");
    pointer=NSMakePoint(550,200);
    check(con_session_drag_sample(capture,&x,&y,&outside)==0 && outside && ordered==1,
        @"outside tracking reveals the non-interactive preview");
    pointer=NSMakePoint(200,200);
    check(con_session_drag_sample(capture,&x,&y,&outside)==0 && !outside,
        @"reentering returns to in-window destinations without committing");
    pointer=NSMakePoint(550,200); buttons=0;
    check(con_session_drag_sample(capture,&x,&y,&outside)==1 && outside,
        @"button release finishes even when no mouse-up event arrives");
    double releaseX=x,releaseY=y;
    pointer=NSMakePoint(200,200);
    check(con_session_drag_sample(capture,&x,&y,&outside)==1 && outside && x==releaseX && y==releaseY,
        @"later pointer motion cannot change the recorded release");
    con_session_drag_end(capture);
    check(removed==1 && handler==nil, @"completion removes the local monitor");

    capture=begin(view);
    NSEvent *up=[NSEvent mouseEventWithType:NSEventTypeLeftMouseUp location:NSMakePoint(560,180)
        modifierFlags:0 timestamp:0 windowNumber:0 context:nil eventNumber:0 clickCount:1 pressure:0];
    check(handler(up)==up,@"mouse-up is observed without swallowing normal GPUI dispatch");
    pointer=NSMakePoint(200,200);
    check(con_session_drag_sample(capture,&x,&y,&outside)==1 && outside && x==460 && y==220,
        @"the event's release point wins over a later live cursor position");
    NSEvent *escape=[NSEvent keyEventWithType:NSEventTypeKeyDown location:NSZeroPoint modifierFlags:0
        timestamp:0 windowNumber:0 context:nil characters:@"\033" charactersIgnoringModifiers:@"\033" isARepeat:NO keyCode:53];
    check(handler(escape)==nil,@"Escape is consumed by the gesture, never sent to a terminal");
    check(con_session_drag_sample(capture,&x,&y,&outside)==2,@"cancellation wins over a pending release");
    con_session_drag_end(capture);

    capture=begin(view); active=NO;
    check(con_session_drag_sample(capture,&x,&y,&outside)==2,@"deactivation cancels rather than detaching");
    con_session_drag_end(capture);
    capture=begin(view); visible=NO;
    check(con_session_drag_sample(capture,&x,&y,&outside)==2,@"a hidden/closed source cancels safely");
    con_session_drag_end(capture);
    check(removed==4 && handler==nil,@"every cancellation releases observation ownership");
    [window close];
    puts("session drag capture: release, reentry, cancellation and cleanup passed");
} return 0; }
