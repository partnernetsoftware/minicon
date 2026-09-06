// Research-only capability comparison, not a qualified terminal host.
#import <Cocoa/Cocoa.h>
#import <QuartzCore/QuartzCore.h>
#include <sys/mman.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#ifndef RESEARCH_POLICY
#define RESEARCH_POLICY NSApplicationActivationPolicyAccessory
#endif

@interface PixelInputView : NSView <NSTextInputClient>
@property(copy) NSAttributedString *marked;
@property NSRange selection;
@end
@implementation PixelInputView
- (BOOL)acceptsFirstResponder { return YES; }
- (void)keyDown:(NSEvent *)event { [self interpretKeyEvents:@[event]]; }
- (void)insertText:(id)text replacementRange:(NSRange)range {
    (void)range;
    NSString *string = [text isKindOfClass:NSAttributedString.class] ? [text string] : text;
    fprintf(stderr, "COMMIT %s\n", string.UTF8String);
    self.marked = nil;
}
- (void)setMarkedText:(id)text selectedRange:(NSRange)selection replacementRange:(NSRange)range {
    (void)range;
    self.marked = [text isKindOfClass:NSAttributedString.class] ? text : [[NSAttributedString alloc] initWithString:text];
    self.selection = selection;
}
- (void)unmarkText { self.marked = nil; }
- (BOOL)hasMarkedText { return self.marked.length != 0; }
- (NSRange)markedRange { return self.hasMarkedText ? NSMakeRange(0, self.marked.length) : NSMakeRange(NSNotFound, 0); }
- (NSRange)selectedRange { return self.hasMarkedText ? self.selection : NSMakeRange(0, 0); }
- (NSArray<NSAttributedStringKey> *)validAttributesForMarkedText { return @[]; }
- (NSAttributedString *)attributedSubstringForProposedRange:(NSRange)range actualRange:(NSRangePointer)actual {
    NSRange available = NSMakeRange(0, self.marked.length);
    if (range.location == NSNotFound || range.location > available.length) {
        if (actual) *actual = NSMakeRange(NSNotFound, 0);
        return nil;
    }
    NSRange clipped = NSIntersectionRange(range, available);
    if (actual) *actual = clipped;
    return self.marked ? [self.marked attributedSubstringFromRange:clipped] : nil;
}
- (NSUInteger)characterIndexForPoint:(NSPoint)point { (void)point; return NSNotFound; }
- (NSRect)firstRectForCharacterRange:(NSRange)range actualRange:(NSRangePointer)actual {
    if (actual) *actual = range;
    return [self.window convertRectToScreen:[self convertRect:NSMakeRect(20, 20, 10, 20) toView:nil]];
}
- (void)doCommandBySelector:(SEL)selector { (void)selector; }
@end

static void releasePixels(void *info, const void *data, size_t bytes) {
    (void)info;
    munmap((void *)data, bytes);
}
static BOOL presentPixels(NSView *view) {
    view.wantsLayer = YES;
    CALayer *layer = [CALayer layer];
    [view.layer addSublayer:layer];
    layer.anchorPoint = CGPointZero;
    layer.geometryFlipped = YES;
    layer.frame = view.bounds;
    layer.contentsScale = view.window.backingScaleFactor;
    layer.contentsGravity = kCAGravityTopLeft;
    size_t width = (size_t)(view.bounds.size.width * layer.contentsScale);
    size_t height = (size_t)(view.bounds.size.height * layer.contentsScale);
    if (!width || !height || width > SIZE_MAX / 4 / height) return NO;
    size_t bytes = width * height * 4;
    uint32_t *pixels = mmap(NULL, bytes, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
    if (pixels == MAP_FAILED) return NO;
    for (size_t y = 0; y < height; y++)
        for (size_t x = 0; x < width; x++)
            pixels[y * width + x] = ((x / 32 + y / 32) & 1) ? 0x00304050 : 0x00102030;
    if (mprotect(pixels, bytes, PROT_READ) != 0) { munmap(pixels, bytes); return NO; }
    CGDataProviderRef provider = CGDataProviderCreateWithData(NULL, pixels, bytes, releasePixels);
    if (!provider) { munmap(pixels, bytes); return NO; }
    CGColorSpaceRef color = CGDisplayCopyColorSpace(CGMainDisplayID());
    if (!color) { CGDataProviderRelease(provider); return NO; }
    CGImageRef image = CGImageCreate(width, height, 8, 32, width * 4, color,
        kCGBitmapByteOrder32Little | kCGImageAlphaNoneSkipFirst, provider, NULL, false, kCGRenderingIntentDefault);
    if (image) layer.contents = (__bridge id)image;
    fprintf(stderr, "PIXELS width=%zu height=%zu bytes=%zu image=%d\n", width, height, bytes, image != NULL);
    BOOL success = image != NULL;
    if (image) CGImageRelease(image);
    CGColorSpaceRelease(color);
    CGDataProviderRelease(provider);
    return success;
}
static void installMenu(NSApplication *app) {
    NSMenu *bar = [[NSMenu alloc] initWithTitle:@""];
    NSMenuItem *root = [[NSMenuItem alloc] initWithTitle:@"" action:NULL keyEquivalent:@""];
    [bar addItem:root];
    NSMenu *menu = [[NSMenu alloc] initWithTitle:@""];
    [menu addItemWithTitle:@"About Memory research" action:@selector(orderFrontStandardAboutPanel:) keyEquivalent:@""];
    [menu addItem:NSMenuItem.separatorItem];
    NSMenu *services = [[NSMenu alloc] initWithTitle:@""];
    NSMenuItem *serviceItem = [menu addItemWithTitle:@"Services" action:NULL keyEquivalent:@""];
    serviceItem.submenu = services;
    [menu addItemWithTitle:@"Hide Memory research" action:@selector(hide:) keyEquivalent:@"h"];
    NSMenuItem *others = [menu addItemWithTitle:@"Hide Others" action:@selector(hideOtherApplications:) keyEquivalent:@"h"];
    others.keyEquivalentModifierMask = NSEventModifierFlagOption | NSEventModifierFlagCommand;
    [menu addItemWithTitle:@"Show All" action:@selector(unhideAllApplications:) keyEquivalent:@""];
    [menu addItem:NSMenuItem.separatorItem];
    [menu addItemWithTitle:@"Quit Memory research" action:@selector(terminate:) keyEquivalent:@"q"];
    root.submenu = menu;
    app.servicesMenu = services;
    app.mainMenu = bar;
}
int main(int argc, const char **argv) {
    const char *mode = argc > 1 ? argv[1] : "pixels-input-menu";
    @autoreleasepool {
        NSApplication *app = NSApplication.sharedApplication;
        [app setActivationPolicy:RESEARCH_POLICY];
        if (strstr(mode, "menu")) installMenu(app);
        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(20, 20, 960, 600)
            styleMask:NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskResizable
            backing:NSBackingStoreBuffered defer:NO];
        window.title = @"Memory research";
        PixelInputView *view = [[PixelInputView alloc] initWithFrame:NSMakeRect(0, 0, 960, 600)];
        window.contentView = view;
        BOOL responder = [window makeFirstResponder:view];
        BOOL context = NO;
        if (strstr(mode, "input")) context = view.inputContext != nil;
        [window orderFront:nil];
        [app finishLaunching];
        if (strstr(mode, "pixels") && !presentPixels(view)) return 2;
        fprintf(stderr, "INPUT responder=%d context=%d mode=%s\n", responder, context, mode);
        fprintf(stderr, "WINDOW %ld\n", (long)window.windowNumber);
        puts("READY"); fflush(stdout);
        for (int i = 0; i < 1200; i++) {
            @autoreleasepool {
                NSEvent *event = [app nextEventMatchingMask:NSEventMaskAny
                    untilDate:[NSDate dateWithTimeIntervalSinceNow:.1]
                    inMode:NSDefaultRunLoopMode dequeue:YES];
                if (event) [app sendEvent:event];
                [app updateWindows];
                if (i == 20 && strstr(mode, "pixels")) {
                    CALayer *pixels = view.layer.sublayers.firstObject;
                    fprintf(stderr, "GEOMETRY view=%gx%g root=%gx%g pixels=%gx%g image=%d\n",
                        view.bounds.size.width, view.bounds.size.height,
                        view.layer.bounds.size.width, view.layer.bounds.size.height,
                        pixels.bounds.size.width, pixels.bounds.size.height, pixels.contents != nil);
                }
            }
        }
    }
    return 0;
}
