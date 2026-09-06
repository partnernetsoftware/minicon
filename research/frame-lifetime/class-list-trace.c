/* Observation only: never inspect Class contents or alter enumeration results. */
#include <objc/runtime.h>
#include <mach/mach.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <stdint.h>
#include <stdatomic.h>
static _Thread_local unsigned depth;
static _Atomic unsigned sequence;
static int enabled;
static unsigned long long external_bytes(int *result) {
    task_vm_info_data_t vm={0};mach_msg_type_number_t count=TASK_VM_INFO_COUNT;
    *result=task_info(mach_task_self(),TASK_VM_INFO,(task_info_t)&vm,&count);
    return vm.external;
}
static void record(const char *api,void *caller,unsigned long long before,unsigned long long after,int before_kr,int after_kr,int count) {
    char line[512];unsigned n=atomic_fetch_add(&sequence,1);
    if(n>=64)return;
    int length=snprintf(line,sizeof(line),"CLASS_API seq=%u api=%s caller=%p before=%llu after=%llu delta=%lld before_kr=%d after_kr=%d returned_count=%d\n",n,api,caller,before,after,(long long)after-(long long)before,before_kr,after_kr,count);
    if(length>0&&(size_t)length<sizeof(line)){
        ssize_t offset=0;while(offset<length){ssize_t w=write(STDERR_FILENO,line+offset,(size_t)(length-offset));if(w<0&&errno==EINTR)continue;if(w<=0)break;offset+=w;}
    }
}
static Class *traced_copy(unsigned int *outCount) {
    int entry_errno=errno;
    if(!enabled||depth){errno=entry_errno;return objc_copyClassList(outCount);}
    depth++;
    int a=0,b=0;unsigned long long before=external_bytes(&a);
    errno=entry_errno;Class *result=objc_copyClassList(outCount);int result_errno=errno;
    unsigned long long after=external_bytes(&b);
    /* Only the caller-provided scalar count is read; returned Class objects are not touched. */
    record("objc_copyClassList",__builtin_return_address(0),before,after,a,b,outCount?(int)*outCount:-1);
    depth--;errno=result_errno;return result;
}
static int traced_get(Class *buffer,int bufferCount) {
    int entry_errno=errno;
    if(!enabled||depth){errno=entry_errno;return objc_getClassList(buffer,bufferCount);}
    depth++;
    int a=0,b=0;unsigned long long before=external_bytes(&a);
    errno=entry_errno;int result=objc_getClassList(buffer,bufferCount);int result_errno=errno;
    unsigned long long after=external_bytes(&b);
    record("objc_getClassList",__builtin_return_address(0),before,after,a,b,result);
    depth--;errno=result_errno;return result;
}
#define INTERPOSE(replacement,replacee) \
__attribute__((used)) static const struct {const void *replacement;const void *replacee;} interpose_##replacee \
__attribute__((section("__DATA,__interpose")))={(const void *)(uintptr_t)&replacement,(const void *)(uintptr_t)&replacee}
INTERPOSE(traced_copy,objc_copyClassList);
INTERPOSE(traced_get,objc_getClassList);
__attribute__((constructor)) static void initialize(void) {
    const char *target=getenv("CLASS_TRACE_TARGET");enabled=target&&!strcmp(target,getprogname());
}
