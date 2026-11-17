// ObjC exception trampoline for Sparkle initialization.
//
// Rust's catch_unwind cannot catch ObjC exceptions (__rust_foreign_exception).
// This file provides @try/@catch wrappers so the Rust side never sees an
// uncatchable exception from Sparkle.
//
// Sparkle classes are loaded dynamically at runtime, so we use the ObjC
// runtime API (objc_msgSend) rather than compiled method calls.

#import <Foundation/Foundation.h>
#import <objc/message.h>
#import <objc/runtime.h>

/// Initialize SPUStandardUpdaterController with @try/@catch.
///
/// Returns the initialized controller, or NULL if an ObjC exception was thrown.
/// Uses initWithStartingUpdater:YES so Sparkle begins polling immediately after
/// successful controller creation.
void *con_sparkle_init_controller(void) {
    @try {
        Class cls = NSClassFromString(@"SPUStandardUpdaterController");
        if (!cls) return NULL;

        id alloc = [cls alloc];
        if (!alloc) return NULL;

        // initWithStartingUpdater:(BOOL)startingUpdater
        //   updaterDelegate:(id)updaterDelegate
        //   userDriverDelegate:(id)userDriverDelegate
        SEL initSel = NSSelectorFromString(
            @"initWithStartingUpdater:updaterDelegate:userDriverDelegate:");

        // Use objc_msgSend typed cast for the 3-arg init
        typedef id (*InitIMP)(id, SEL, BOOL, id, id);
        InitIMP initFunc = (InitIMP)objc_msgSend;
        id controller = initFunc(alloc, initSel, YES, nil, nil);

        if (!controller) return NULL;
        return (__bridge_retained void *)controller;
    } @catch (NSException *exception) {
        NSLog(@"[con-updater] ObjC exception during Sparkle init: %@ — %@",
              exception.name, exception.reason);
        return NULL;
    }
}

/// Call -[SPUUpdater startUpdater:] with @try/@catch.
///
/// Returns 1 on success, 0 on failure or exception.
int con_sparkle_start_updater(void *controller) {
    @try {
        id ctrl = (__bridge id)controller;

        // Get the SPUUpdater from the controller
        SEL updaterSel = NSSelectorFromString(@"updater");
        id updater = ((id (*)(id, SEL))objc_msgSend)(ctrl, updaterSel);
        if (!updater) return 0;

        // -[SPUUpdater startUpdater:] returns BOOL, takes NSError**
        SEL startSel = NSSelectorFromString(@"startUpdater:");
        NSError *error = nil;
        typedef BOOL (*StartIMP)(id, SEL, NSError **);
        StartIMP startFunc = (StartIMP)objc_msgSend;
        BOOL ok = startFunc(updater, startSel, &error);
        if (!ok) {
            NSLog(@"[con-updater] startUpdater failed: %@", error);
            return 0;
        }
        return 1;
    } @catch (NSException *exception) {
        NSLog(@"[con-updater] ObjC exception in startUpdater: %@ — %@",
              exception.name, exception.reason);
        return 0;
    }
}

/// Call -[SPUUpdater checkForUpdates] with @try/@catch.
void con_sparkle_check_for_updates(void *controller) {
    @try {
        id ctrl = (__bridge id)controller;
        SEL checkSel = NSSelectorFromString(@"checkForUpdates:");
        ((void (*)(id, SEL, id))objc_msgSend)(ctrl, checkSel, nil);
    } @catch (NSException *exception) {
        NSLog(@"[con-updater] ObjC exception in checkForUpdates: %@ — %@",
              exception.name, exception.reason);
    }
}

// Return -1 when unavailable; zero is a valid persisted user choice.
int con_sparkle_automatic_checks(void *controller) {
    @try {
        if (!controller) return -1;
        id updater = ((id (*)(id, SEL))objc_msgSend)((__bridge id)controller, NSSelectorFromString(@"updater"));
        if (!updater) return -1;
        return ((BOOL (*)(id, SEL))objc_msgSend)(updater, NSSelectorFromString(@"automaticallyChecksForUpdates")) ? 1 : 0;
    } @catch (NSException *exception) {
        NSLog(@"[con-updater] Cannot read update checks preference: %@", exception);
        return -1;
    }
}

int con_sparkle_set_automatic_checks(void *controller, int enabled) {
    @try {
        if (!controller) return 0;
        id updater = ((id (*)(id, SEL))objc_msgSend)((__bridge id)controller, NSSelectorFromString(@"updater"));
        if (!updater) return 0;
        ((void (*)(id, SEL, BOOL))objc_msgSend)(updater, NSSelectorFromString(@"setAutomaticallyChecksForUpdates:"), enabled != 0);
        BOOL actual = ((BOOL (*)(id, SEL))objc_msgSend)(updater, NSSelectorFromString(@"automaticallyChecksForUpdates"));
        return actual == (enabled != 0) ? 1 : 0;
    } @catch (NSException *exception) {
        NSLog(@"[con-updater] Cannot change update checks preference: %@", exception);
        return 0;
    }
}
