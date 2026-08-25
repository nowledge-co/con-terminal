#import <Foundation/Foundation.h>
#import <UserNotifications/UserNotifications.h>
#include <stdbool.h>

bool con_ghostty_show_desktop_notification(const char *title, const char *body) {
    if (title == NULL || body == NULL) {
        return false;
    }

    NSString *notificationTitle = [NSString stringWithUTF8String:title];
    NSString *notificationBody = [NSString stringWithUTF8String:body];
    if (notificationTitle == nil || notificationBody == nil) {
        return false;
    }

    UNUserNotificationCenter *center = UNUserNotificationCenter.currentNotificationCenter;
    UNAuthorizationOptions options = UNAuthorizationOptionAlert | UNAuthorizationOptionSound;
    [center requestAuthorizationWithOptions:options
                          completionHandler:^(BOOL granted, NSError *error) {
        if (error != nil) {
            NSLog(@"[con-notification] authorization failed: %@", error);
            return;
        }
        if (!granted) {
            return;
        }

        UNMutableNotificationContent *content = [[UNMutableNotificationContent alloc] init];
        content.title = notificationTitle;
        content.body = notificationBody;
        content.sound = UNNotificationSound.defaultSound;

        UNNotificationRequest *request = [UNNotificationRequest
            requestWithIdentifier:NSUUID.UUID.UUIDString
                           content:content
                           trigger:nil];
        [center addNotificationRequest:request
                 withCompletionHandler:^(NSError *scheduleError) {
            if (scheduleError != nil) {
                NSLog(@"[con-notification] scheduling failed: %@", scheduleError);
            }
        }];
    }];

    return true;
}
