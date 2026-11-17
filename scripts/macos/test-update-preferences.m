#import <AppKit/AppKit.h>
#import <objc/message.h>
extern void *con_sparkle_init_controller(void);
extern int con_sparkle_automatic_checks(void *);
extern int con_sparkle_set_automatic_checks(void *, int);

int main(void) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
        NSString *domain = NSBundle.mainBundle.bundleIdentifier;
        NSCAssert([domain hasPrefix:@"co.nowledge.con.updater-test."], @"isolated bundle required");
        NSUserDefaults *defaults = NSUserDefaults.standardUserDefaults;
        @try {
            // Simulate a pre-existing opt-in. Bundle policy must still require
            // explicit installation without erasing the user's stored choice.
            [defaults setBool:YES forKey:@"SUAutomaticallyUpdate"];
            NSString *framework = [NSBundle.mainBundle.bundlePath stringByAppendingPathComponent:@"Contents/Frameworks/Sparkle.framework"];
            NSCAssert([[NSBundle bundleWithPath:framework] load], @"Sparkle load failed");
            void *controller = con_sparkle_init_controller();
            NSCAssert(controller, @"controller initialization failed");
            NSCAssert(con_sparkle_automatic_checks(controller) == 0, @"fixture starts with checks off");
            id updater = ((id (*)(id, SEL))objc_msgSend)((__bridge id)controller, NSSelectorFromString(@"updater"));
            NSCAssert(!((BOOL (*)(id, SEL))objc_msgSend)(updater, NSSelectorFromString(@"automaticallyDownloadsUpdates")), @"silent download policy ignored");
            NSCAssert(((BOOL (*)(id, SEL))objc_msgSend)(updater, NSSelectorFromString(@"canCheckForUpdates")), @"manual check unavailable while automatic checks off");
            NSCAssert(con_sparkle_set_automatic_checks(controller, 1), @"cannot enable");
            NSCAssert(con_sparkle_automatic_checks(controller) == 1, @"getter did not follow setter");
            NSCAssert([defaults boolForKey:@"SUEnableAutomaticChecks"], @"choice not persisted by Sparkle");
            NSCAssert(con_sparkle_set_automatic_checks(controller, 0), @"cannot disable");
            NSCAssert(con_sparkle_automatic_checks(controller) == 0, @"disable did not reach Sparkle");
            NSCAssert(![defaults boolForKey:@"SUEnableAutomaticChecks"], @"disabled choice not persisted");
            NSCAssert([defaults boolForKey:@"SUAutomaticallyUpdate"], @"unrelated preference overwritten");
            NSCAssert(con_sparkle_automatic_checks(NULL) == -1, @"unavailable is not a disabled preference");
            NSCAssert(!con_sparkle_set_automatic_checks(NULL, 1), @"unavailable setter succeeded");
            puts("Sparkle update consent bridge passed");
        } @finally {
            [defaults removePersistentDomainForName:domain];
        }
    }
}
