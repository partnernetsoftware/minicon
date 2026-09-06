"""UNSUPPORTED private-method causal negative control. Never a product patch."""
from pathlib import Path
import subprocess as s, os, time, json, hashlib
root=Path.cwd(); out=root/os.environ.get('WRITING_TOOLS_OUT','target/frame-lifetime/writing-tools-private');out.mkdir(exist_ok=True)
s.run(['python3','research/frame-lifetime/prepare-external-stages.py'],env={**os.environ,'EXTERNAL_STAGE_DETAIL':'1'},check=True)
source=(root/'target/frame-lifetime/external-stages.m').read_text()
source=source.replace('app.servicesMenu = services;', '''if (getenv("DISABLE_WRITING_TOOLS")) {
        if (@available(macOS 15.2, *)) {
            for (NSMenu *item in @[bar, menu, services]) item.automaticallyInsertsWritingToolsItems=NO;
            fprintf(stderr,"WRITING_TOOLS_FLAG_NO menus=3\\n");
        }
    }
    app.servicesMenu = services;''')
source=source.replace('[app finishLaunching];','installMenu(app);\n        checkpoint("pre-finish");\n        [app finishLaunching];')
source=source.replace('        installMenu(app);\n        settle(app);\n        checkpoint("complete-menu");','        checkpoint("complete-menu");')
if os.environ.get('WRITING_TOOLS_TRAITS'):
    source=source.replace('<NSTextInputClient>', '<NSTextInputClient, NSTextInputTraits>')
    source=source.replace('@implementation PixelInputView', '@implementation PixelInputView\n- (NSWritingToolsBehavior)writingToolsBehavior { return NSWritingToolsBehaviorNone; }')
source='#import <objc/runtime.h>\n'+source
source=source.replace('int main(void) {', """static BOOL research_no_writing_tools(id self, SEL cmd) { (void)self;(void)cmd;return NO; }
int main(void) {
    Method method=class_getClassMethod(NSTextView.class,sel_registerName("_supportsWritingTools"));
    char encoding[32]={0};
    if(method)method_getReturnType(method,encoding,sizeof(encoding));
    if(!method || method_getNumberOfArguments(method)!=2 || strcmp(encoding,@encode(BOOL))!=0) {
        fprintf(stderr,"PRIVATE_GUARD_FAILED return=%s\\n",encoding);return 4;
    }
    fprintf(stderr,"PRIVATE_GUARD_OK return=%s args=2\\n",encoding);
    if(getenv("PRIVATE_WRITING_TOOLS_NO")) {
        method_setImplementation(method,(IMP)research_no_writing_tools);
        fprintf(stderr,"UNSUPPORTED_PRIVATE_NEGATIVE_CONTROL_INSTALLED\\n");
    }
""")
(out/'probe.m').write_text(source)
binary=out/'probe'
s.run(['clang','-O2','-fobjc-arc','-framework','Cocoa','-framework','QuartzCore','-framework','CoreGraphics','-Wl,-sectcreate,__TEXT,__info_plist,research/hello-memory/compat.plist','research/frame-lifetime/external-pages.c',str(out/'probe.m'),'-o',str(binary)],check=True)
results={}
for mode in ['default','private-no']:
    run=out/mode;run.mkdir(exist_ok=True)
    env={**os.environ,'EXTERNAL_STAGE_DIRECTORY':str(run)};env.pop('DISABLE_WRITING_TOOLS',None)
    env.pop('PRIVATE_WRITING_TOOLS_NO',None)
    if mode=='private-no':env['PRIVATE_WRITING_TOOLS_NO']='1'
    with (run/'host.log').open('w') as log:
        p=s.Popen([str(binary)],env=env,stdout=log,stderr=s.STDOUT)
        try:
            for _ in range(400):
                if '\nREADY\n' in (run/'host.log').read_text():break
                if p.poll() is not None:raise RuntimeError('native exited early')
                time.sleep(.1)
            else:raise RuntimeError('native ready timeout')
            rss=int(s.check_output(['ps','-o','rss=','-p',str(p.pid)],text=True).strip())*1024
            v=s.run(['vmmap','-resident',str(p.pid)],capture_output=True,text=True,timeout=30);(run/'vmmap.txt').write_text(v.stdout+v.stderr)
            if v.returncode:raise RuntimeError('vmmap failure')
            ledgers={}
            for name in ['pre-finish','finish-launching','event-settle','complete-menu']:
                parsed=json.loads(s.check_output(['python3','research/frame-lifetime/attribute-external.py',str(run/(name+'.tsv')),str(run/'vmmap.txt')],text=True))
                ledgers[name]=parsed['ledger'];
            results[mode]={'rss_bytes':rss,'ledgers':ledgers,'writing_tools_ui_mapped':'WritingToolsUI' in v.stdout}
            print(mode,json.dumps(results[mode]),flush=True)
        finally:
            if p.poll() is None:p.terminate()
            p.wait(timeout=10)
(out/'results.json').write_text(json.dumps({'sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),'runs':results},indent=2))
