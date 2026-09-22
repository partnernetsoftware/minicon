from pathlib import Path
import os
root=Path.cwd();source=(root/'archive/research/hello-memory/pixel-host.m').read_text();source=source[:source.index('int main(')]
source+='''
extern int external_residency_snapshot(const char *path);
static void checkpoint(const char *name) {
    char path[2048];
    snprintf(path,sizeof(path),"%s/%s.tsv",getenv("EXTERNAL_STAGE_DIRECTORY"),name);
    int result=external_residency_snapshot(path);
    fprintf(stderr,"CHECKPOINT %s result=%d\\n",name,result);
    if(result)exit(3);
}
static void settle(NSApplication *app) {
    NSDate *end=[NSDate dateWithTimeIntervalSinceNow:1.0];
    while(end.timeIntervalSinceNow>0) {
        @autoreleasepool {
            NSEvent *event=[app nextEventMatchingMask:NSEventMaskAny untilDate:[NSDate dateWithTimeIntervalSinceNow:.05] inMode:NSDefaultRunLoopMode dequeue:YES];
            if(event)[app sendEvent:event];
            [app updateWindows];
        }
    }
}
int main(void) {
    checkpoint("observer-first");
    checkpoint("observer-warm");
    @autoreleasepool {
        NSApplication *app=NSApplication.sharedApplication;
        checkpoint("shared-application");
        [app setActivationPolicy:NSApplicationActivationPolicyRegular];
        NSWindow *window=[[NSWindow alloc] initWithContentRect:NSMakeRect(20,20,960,600)
            styleMask:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable|NSWindowStyleMaskResizable backing:NSBackingStoreBuffered defer:NO];
        window.title=@"Memory research";
        PixelInputView *view=[[PixelInputView alloc] initWithFrame:NSMakeRect(0,0,960,600)];
        window.contentView=view;
        BOOL responder=[window makeFirstResponder:view];
        BOOL context=view.inputContext!=nil;
        [window orderFront:nil];
        [app finishLaunching];
        if(!presentPixels(view,"pixels-input"))return 2;
        settle(app);
        checkpoint("window-pixels-input");
        installMenu(app);
        settle(app);
        checkpoint("complete-menu");
        fprintf(stderr,"INPUT responder=%d context=%d WINDOW=%ld\\n",responder,context,(long)window.windowNumber);
        puts("READY");fflush(stdout);
        for(int i=0;i<100;i++)settle(app);
    }
    return 0;
}
'''
if os.environ.get('EXTERNAL_STAGE_DETAIL'):
    source=source.replace('[app setActivationPolicy:NSApplicationActivationPolicyRegular];','[app setActivationPolicy:NSApplicationActivationPolicyRegular];\n        checkpoint("regular-policy");')
    source=source.replace('window.title=@"Memory research";', 'checkpoint("window-init");\n        window.title=@"Memory research";')
    source=source.replace('[window orderFront:nil];\n        [app finishLaunching];\n        if(!presentPixels(view,"pixels-input"))return 2;\n        settle(app);\n        checkpoint("window-pixels-input");', 'if(!presentPixels(view,"pixels-input"))return 2;\n        checkpoint("content-input-pixels");\n        [window orderFront:nil];\n        checkpoint("order-front");\n        [app finishLaunching];\n        checkpoint("finish-launching");\n        settle(app);\n        checkpoint("event-settle");')
if os.environ.get('EXTERNAL_STAGE_DETAIL'):
    source=source.replace('BOOL responder=[window makeFirstResponder:view];', 'checkpoint("content-view");\n        BOOL responder=[window makeFirstResponder:view];\n        checkpoint("first-responder");')
    source=source.replace('BOOL context=view.inputContext!=nil;', 'BOOL context=view.inputContext!=nil;\n        checkpoint("input-context");')
    source=source.replace('checkpoint("content-input-pixels");','checkpoint("pixels-present");')
(root/'target/frame-lifetime/external-stages.m').write_text(source)
