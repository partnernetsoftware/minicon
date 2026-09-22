/* Read-only dlopen observer. Original arguments/return/errno preserved. */
#include <dlfcn.h>
#include <mach/mach.h>
#include <pthread.h>
#include <errno.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <stdatomic.h>

static _Thread_local unsigned depth;
static _Atomic unsigned sequence,active;
static unsigned long long external(int *kr) {
 task_vm_info_data_t info={0};mach_msg_type_number_t count=TASK_VM_INFO_COUNT;
 *kr=task_info(mach_task_self(),TASK_VM_INFO,(task_info_t)&info,&count);return info.external;
}
static void *observed(const char *path,int flags) {
 int incoming=errno;
 if(depth){errno=incoming;return dlopen(path,flags);}
 depth++;unsigned seq=atomic_fetch_add(&sequence,1);unsigned concurrent=atomic_fetch_add(&active,1);
 uint64_t thread=0;pthread_threadid_np(NULL,&thread);
 int a=0,b=0;unsigned long long before=external(&a);
 void *caller=__builtin_return_address(0);errno=incoming;void *result=dlopen(path,flags);int returned=errno;
 unsigned long long after=external(&b);unsigned still=atomic_fetch_sub(&active,1)-1;
 if(seq<128){char line[2048];int n=snprintf(line,sizeof(line),"DLOPEN seq=%u thread=%llu caller=%p flags=%d path=%s success=%d before=%llu after=%llu kr=%d,%d overlap=%u,%u\n",seq,(unsigned long long)thread,caller,flags,path?path:"<NULL>",result!=NULL,before,after,a,b,concurrent,still);if(n>0&&(size_t)n<sizeof(line))write(2,line,(size_t)n);}
 depth--;errno=returned;return result;
}
__attribute__((used)) static const struct {const void *replacement;const void *replacee;} interpose
__attribute__((section("__DATA,__interpose")))={(const void *)(uintptr_t)&observed,(const void *)(uintptr_t)&dlopen};
__attribute__((constructor)) static void initialize(void) {
 unsetenv("DYLD_INSERT_LIBRARIES");
}
