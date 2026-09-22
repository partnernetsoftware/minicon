#import <Cocoa/Cocoa.h>
#include <stdio.h>
#include <string.h>

// Research only: a fresh process walks the same initialization sequence.
// stdin handshakes let the parent sample each completed stage without racing.
static void ready(const char *stage) {
    puts(stage);
    fflush(stdout);
    if (getchar() == EOF) exit(0);
}
int main(int argc, const char **argv) {
    BOOL drain = argc > 1 && strcmp(argv[1], "drain") == 0;
    NSAutoreleasePool *outer = [[NSAutoreleasePool alloc] init];
    ready("foundation-pool");
    NSApplication *app = nil;
    for (int stage = 0; stage < 4; stage++) {
        NSAutoreleasePool *pool = drain ? [[NSAutoreleasePool alloc] init] : nil;
        switch (stage) {
            case 0: app = NSApplication.sharedApplication; break;
            case 1: [app setActivationPolicy:NSApplicationActivationPolicyAccessory]; break;
            case 2: [app finishLaunching]; break;
            case 3:
                for (int i = 0; i < 10; i++) {
                    @autoreleasepool {
                        NSEvent *event = [app nextEventMatchingMask:NSEventMaskAny
                            untilDate:[NSDate dateWithTimeIntervalSinceNow:.01]
                            inMode:NSDefaultRunLoopMode dequeue:YES];
                        if (event) [app sendEvent:event];
                        [app updateWindows];
                    }
                }
                break;
        }
        [pool drain];
        const char *names[] = {"shared-application", "accessory-policy", "finish-launching", "dispatch-events"};
        ready(names[stage]);
    }
    [outer drain];
    ready("outer-pool-drained");
    return 0;
}
