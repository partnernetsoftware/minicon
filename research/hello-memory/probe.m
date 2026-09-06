#import <Cocoa/Cocoa.h>
#include <unistd.h>
#include <string.h>
int main(int argc, const char **argv) {
 const char *mode=argc>1?argv[1]:"linked";
 if(!strcmp(mode,"linked")){puts("READY");fflush(stdout);sleep(120);return 0;}
 @autoreleasepool {
  if(!strcmp(mode,"foundation")){(void)[NSDate date]; puts("READY");fflush(stdout);sleep(120);return 0;}
  if(!strcmp(mode,"locale-current")) { (void)NSLocale.currentLocale; puts("READY");fflush(stdout);sleep(120);return 0; }
  if(!strcmp(mode,"locale-fixed")) { (void)[NSLocale localeWithLocaleIdentifier:@"en_US_POSIX"]; puts("READY");fflush(stdout);sleep(120);return 0; }
  if(!strcmp(mode,"app-class")) { (void)[NSApplication instancesRespondToSelector:@selector(run)]; puts("READY");fflush(stdout);sleep(120);return 0; }
  if(!strncmp(mode,"component-",10)) {
   (void)[NSApplication instancesRespondToSelector:@selector(run)];
   if(!strcmp(mode,"component-appearance")) (void)[NSAppearance appearanceNamed:NSAppearanceNameDarkAqua];
   if(!strcmp(mode,"component-screen")) (void)NSScreen.screens;
   if(!strcmp(mode,"component-workspace")) (void)NSWorkspace.sharedWorkspace;
   if(!strcmp(mode,"component-pasteboard")) (void)NSPasteboard.generalPasteboard;
   if(!strcmp(mode,"component-font")) (void)[NSFont systemFontOfSize:12];
   puts("READY");fflush(stdout);sleep(120);return 0;
  }
  NSApplication *app=NSApplication.sharedApplication;
  [app setActivationPolicy:NSApplicationActivationPolicyAccessory];
  if(!strcmp(mode,"app-init")){puts("READY");fflush(stdout);sleep(120);return 0;}
  BOOL windowMode=strncmp(mode,"app-",4)!=0;
  NSWindow *window=nil;
  if(windowMode){
   BOOL borderless=strstr(mode,"borderless")!=NULL;
   NSUInteger style=borderless?NSWindowStyleMaskBorderless:NSWindowStyleMaskTitled|NSWindowStyleMaskClosable|NSWindowStyleMaskResizable;
   if(strstr(mode,"titleonly")) style=NSWindowStyleMaskTitled;
   if(strstr(mode,"titleclose")) style=NSWindowStyleMaskTitled|NSWindowStyleMaskClosable;
   if(strstr(mode,"borderresize")) style=NSWindowStyleMaskResizable;
   window=[[NSWindow alloc] initWithContentRect:NSMakeRect(20,20,320,200) styleMask:style backing:NSBackingStoreBuffered defer:NO];
   [window setTitle:@"Memory research"];
   if(strstr(mode,"noshadow")) [window setHasShadow:NO];
   if(strstr(mode,"hello")){
    NSTextField *label=strstr(mode,"editable")?[[NSTextField alloc] initWithFrame:NSZeroRect]:[NSTextField labelWithString:@"Hello, world!"];
    if(strstr(mode,"editable")) [label setStringValue:@"Hello, world!"];
    [label setFrame:NSMakeRect(20,150,220,24)];
    [window.contentView addSubview:label];
    if(strstr(mode,"editable") && !strstr(mode,"only")) {
     BOOL accepted=[window makeFirstResponder:label];
     id responder=[window firstResponder];
     if(!strstr(mode,"responder")) (void)[label inputContext];
     NSTextInputContext *context=!strstr(mode,"responder") && [responder respondsToSelector:@selector(inputContext)]?[responder inputContext]:nil;
     fprintf(stderr,"INPUT accepted=%d field_editor=%d context=%d\n",accepted,[responder isKindOfClass:[NSTextView class]],context!=nil);
    }
   }
   if(!strstr(mode,"hidden")) [window orderFront:nil];
  }
  if(strstr(mode,"menu")) {
   NSMenu *menu=[[NSMenu alloc] initWithTitle:@"Memory research"];
   if(strstr(mode,"menu-text")) [menu addItemWithTitle:@"Quit" action:@selector(terminate:) keyEquivalent:@"q"];
   else if(strstr(mode,"menu-class")) (void)[NSMenuItem instancesRespondToSelector:@selector(title)];
   else if(!strstr(mode,"menu-empty")) [menu addItem:[NSMenuItem separatorItem]];
   [app setMainMenu:menu];
  }
  if(strcmp(mode,"app-loop") && !strstr(mode,"nofinish")) [app finishLaunching];
  puts("READY");fflush(stdout);
  for(int i=0;i<1200;i++){
   @autoreleasepool {
    NSEvent *event=[app nextEventMatchingMask:NSEventMaskAny untilDate:[NSDate dateWithTimeIntervalSinceNow:.1] inMode:NSDefaultRunLoopMode dequeue:YES];
    if(event) [app sendEvent:event];
    [app updateWindows];
   }
  }
 }
 return 0;
}
