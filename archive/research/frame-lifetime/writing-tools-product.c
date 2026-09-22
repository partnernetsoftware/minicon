/* Unsupported causal experiment ONLY. No AppKit dependency in observer dylib. */
#include <objc/runtime.h>
#include <crt_externs.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
static BOOL no_writing_tools(id self, SEL cmd) {(void)self;(void)cmd;return NO;}
__attribute__((constructor)) static void probe_init(void) {
    if(strcmp(getprogname(),"final-minicon"))return;
    int argc=*_NSGetArgc();char **argv=*_NSGetArgv();
    if(argc<2 || !strcmp(argv[1],"cli")){unsetenv("DYLD_INSERT_LIBRARIES");return;}
    unsetenv("DYLD_INSERT_LIBRARIES"); /* PTY and later descendants never inherit. */
    Class cls=objc_getClass("NSTextView");
    Method method=cls?class_getClassMethod(cls,sel_registerName("_supportsWritingTools")):NULL;
    char result[16]={0},self[16]={0},cmd[16]={0};
    if(method){method_getReturnType(method,result,sizeof(result));method_getArgumentType(method,0,self,sizeof(self));method_getArgumentType(method,1,cmd,sizeof(cmd));}
    if(!method || method_getNumberOfArguments(method)!=2 || strcmp(result,@encode(BOOL)) || strcmp(self,"@") || strcmp(cmd,":")){
        fprintf(stderr,"RESEARCH_PRIVATE_GUARD_FAILED class=%d result=%s self=%s cmd=%s\n",cls!=NULL,result,self,cmd);_exit(86);
    }
    const char *mode=getenv("RESEARCH_PRIVATE_NO");
    if(mode&&!strcmp(mode,"1"))method_setImplementation(method,(IMP)no_writing_tools);
    fprintf(stderr,"RESEARCH_PRIVATE_GUARD_OK pid=%d override=%d\n",getpid(),mode&&!strcmp(mode,"1"));
}
