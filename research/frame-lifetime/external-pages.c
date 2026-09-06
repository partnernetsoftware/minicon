/* Research only: pmap-oriented external page census. Not product code. */
#include <mach/mach.h>
#include <mach/mach_vm.h>
#include <mach/vm_statistics.h>
#include <sys/sysctl.h>
#include <libproc.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <time.h>
#include <errno.h>

static int vm_receipt(FILE *out, const char *phase) {
    task_vm_info_data_t vm = {0};
    mach_msg_type_number_t count = TASK_VM_INFO_COUNT;
    kern_return_t kr = task_info(mach_task_self(), TASK_VM_INFO, (task_info_t)&vm, &count);
    fprintf(out, "LEDGER\t%s\t%d\t%llu\t%llu\t%llu\t%llu\t%llu\n", phase,kr,
      vm.resident_size,vm.internal,vm.external,vm.reusable,vm.compressed);
    return kr == KERN_SUCCESS ? 0 : -1;
}
/* Call between native initialization stages, or once in an injected product.
 * No pixel/header reads, madvise, page touching, or remote task port. */
int external_residency_snapshot(const char *path) {
    int fd=open(path,O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC,0600);
    if(fd<0)return -1;
    FILE *out=fdopen(fd,"w");
    if(!out){close(fd);return -1;}
    int old=0, enabled=1;size_t old_size=sizeof(old);
    if(sysctlbyname("vm.self_region_footprint",&old,&old_size,&enabled,sizeof(enabled))) {
        fprintf(out,"ERROR\tself_region_footprint\t%d\n",errno);fclose(out);return -1;
    }
    int result=vm_receipt(out,"before");
    unsigned long long total=0,errors=0,regions=0;
    mach_vm_address_t address=0;uint32_t depth=0;
    const mach_vm_size_t page=(mach_vm_size_t)getpagesize();
    int dispositions[1024];
    for (;;) {
        mach_vm_size_t size=0;
        vm_region_submap_info_data_64_t info={0};
        mach_msg_type_number_t count=VM_REGION_SUBMAP_INFO_COUNT_64;
        kern_return_t kr=mach_vm_region_recurse(mach_task_self(),&address,&size,&depth,(vm_region_recurse_info_t)&info,&count);
        if(kr==KERN_INVALID_ADDRESS)break;
        if(kr!=KERN_SUCCESS || size==0 || address+size<address){errors++;break;}
        if(info.is_submap){depth++;continue;}
        mach_vm_address_t end=address+size;
        char filename[PROC_PIDPATHINFO_MAXSIZE]={0};
        proc_regionfilename(getpid(),address,filename,sizeof(filename));
        for(char *p=filename;*p;p++)if(*p=='\t'||*p=='\n'||*p=='\r')*p='?';
        fprintf(out,"REGION\t%llx\t%llx\t%d\t%d\t%u\t%s\n",address,end,info.external_pager,info.share_mode,info.user_tag,filename);
        regions++;
        for(mach_vm_address_t cursor=address;cursor<end;) {
            mach_vm_size_t bytes=end-cursor;
            if(bytes>page*1024)bytes=page*1024;
            mach_vm_size_t n=(bytes+page-1)/page;
            kr=mach_vm_page_range_query(mach_task_self(),cursor,bytes,(mach_vm_address_t)(uintptr_t)dispositions,&n);
            if(kr!=KERN_SUCCESS || n!=(bytes+page-1)/page || n>1024){fprintf(out,"QUERY_ERROR\t%llx\t%d\t%llu\n",cursor,kr,n);errors++;cursor+=bytes;continue;}
            mach_vm_address_t run=0;
            for(mach_vm_size_t i=0;i<n;i++) {
                int external=(dispositions[i]&(VM_PAGE_QUERY_PAGE_PRESENT|VM_PAGE_QUERY_PAGE_EXTERNAL))==(VM_PAGE_QUERY_PAGE_PRESENT|VM_PAGE_QUERY_PAGE_EXTERNAL) && !(dispositions[i]&VM_PAGE_QUERY_PAGE_REUSABLE);
                mach_vm_address_t here=cursor+i*page;
                if(external){total+=page;if(!run)run=here;}
                else if(run){fprintf(out,"EXTERNAL_RUN\t%llx\t%llx\n",run,here);run=0;}
            }
            if(run)fprintf(out,"EXTERNAL_RUN\t%llx\t%llx\n",run,cursor+n*page);
            cursor+=bytes;
        }
        address=end;
    }
    if(vm_receipt(out,"after"))result=-1;
    fprintf(out,"TOTAL\t%llu\t%llu\t%llu\t%llu\n",total,regions,errors,page);
    if(sysctlbyname("vm.self_region_footprint",NULL,NULL,&old,sizeof(old))){fprintf(out,"ERROR\trestore\t%d\n",errno);result=-1;}
    if(errors)result=-1;
    if(fclose(out))result=-1;
    return result;
}
static void *worker(void *unused) {
    (void)unused;
    sleep(4);
    const char *path=getenv("EXTERNAL_RESIDENCY_OUTPUT");
    if(path)external_residency_snapshot(path);
    return NULL;
}
__attribute__((constructor)) static void initialize(void) {
    const char *expected=getenv("EXTERNAL_RESIDENCY_EXECUTABLE");
    if(!expected || strcmp(expected,getprogname()))return;
    unsetenv("DYLD_INSERT_LIBRARIES");
    if(!getenv("EXTERNAL_RESIDENCY_OUTPUT"))return; /* loaded-only control */
    pthread_t thread;pthread_attr_t attr;
    pthread_attr_init(&attr);pthread_attr_setstacksize(&attr,128*1024);
    pthread_attr_setdetachstate(&attr,PTHREAD_CREATE_DETACHED);
    pthread_create(&thread,&attr,worker,NULL);pthread_attr_destroy(&attr);
}
