#import <Cocoa/Cocoa.h>
#include <stdlib.h>
// Print only the requested process's ordinary window, never another app's.
int main(int argc, const char **argv) {
    if (argc != 2) return 2;
    pid_t pid = (pid_t)strtol(argv[1], NULL, 10);
    if (pid <= 0) return 2;
    @autoreleasepool {
        NSArray *windows = CFBridgingRelease(CGWindowListCopyWindowInfo(kCGWindowListOptionAll, kCGNullWindowID));
        for (NSDictionary *window in windows) {
            if ([window[(__bridge id)kCGWindowOwnerPID] intValue] != pid) continue;
            if ([window[(__bridge id)kCGWindowLayer] intValue] != 0) continue;
            NSDictionary *bounds = window[(__bridge id)kCGWindowBounds];
            if ([bounds[@"Width"] doubleValue] < 900 || [bounds[@"Height"] doubleValue] < 600) continue;
            printf("%u\n", [window[(__bridge id)kCGWindowNumber] unsignedIntValue]);
            return 0;
        }
    }
    return 1;
}
