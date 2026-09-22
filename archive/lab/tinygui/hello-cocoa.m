/* ObjC AppKit floor. Same 960x600 logical window as the Rust cocoa rung.
 * No timer, no display-link, no activate (court must not steal MiniCon focus).
 */
#import <Cocoa/Cocoa.h>

int main(void) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyRegular];
        NSRect rect = NSMakeRect(0, 0, 960, 600);
        NSUInteger style = NSWindowStyleMaskTitled | NSWindowStyleMaskClosable |
                           NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable;
        NSWindow *window =
            [[NSWindow alloc] initWithContentRect:rect
                                        styleMask:style
                                          backing:NSBackingStoreBuffered
                                            defer:NO];
        [window setTitle:@"tinygui objc"];
        [window center];
        [window makeKeyAndOrderFront:nil];
        [NSApp run];
    }
    return 0;
}
