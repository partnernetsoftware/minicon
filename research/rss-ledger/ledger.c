/* Research only. Read TASK_VM_INFO for this process; no remote task ports. */
#include <mach/mach.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <time.h>
#include <errno.h>
static int output = -1;
static pid_t owner;
static void sample(unsigned index) {
    task_vm_info_data_t vm = {0};
    mach_msg_type_number_t count = TASK_VM_INFO_COUNT;
    kern_return_t result = task_info(mach_task_self(), TASK_VM_INFO,
                                    (task_info_t)&vm, &count);
    char line[2048];
    struct timespec now;
    clock_gettime(CLOCK_MONOTONIC, &now);
    int length = snprintf(line, sizeof(line),
      "{\"pid\":%d,\"index\":%u,\"monotonic_ns\":%llu,\"result\":%d,\"count\":%u,"
      "\"page_size\":%d,\"resident\":%llu,\"resident_peak\":%llu,\"internal\":%llu,"
      "\"external\":%llu,\"reusable\":%llu,\"compressed\":%llu,\"compressed_peak\":%llu,"
      "\"compressed_lifetime\":%llu,\"phys_footprint\":%llu,\"device\":%llu,"
      "\"purgeable_volatile_pmap\":%llu,\"purgeable_volatile_resident\":%llu,"
      "\"purgeable_nonvolatile\":%lld,\"purgeable_nonvolatile_compressed\":%lld,"
      "\"graphics_footprint\":%lld,\"graphics_footprint_compressed\":%lld,"
      "\"graphics_nofootprint\":%lld,\"graphics_nofootprint_compressed\":%lld,"
      "\"decompressions\":%d}\n",
      owner,index,(unsigned long long)now.tv_sec*1000000000+now.tv_nsec,result,count,
      vm.page_size,vm.resident_size,vm.resident_size_peak,vm.internal,
      vm.external,vm.reusable,vm.compressed,vm.compressed_peak,
      vm.compressed_lifetime,vm.phys_footprint,vm.device,
      vm.purgeable_volatile_pmap,vm.purgeable_volatile_resident,
      vm.ledger_purgeable_nonvolatile,vm.ledger_purgeable_novolatile_compressed,
      vm.ledger_tag_graphics_footprint,vm.ledger_tag_graphics_footprint_compressed,
      vm.ledger_tag_graphics_nofootprint,vm.ledger_tag_graphics_nofootprint_compressed,
      vm.decompressions);
    if (length > 0 && (size_t)length < sizeof(line)) {
       ssize_t sent=0;
       while(sent<length) {
          ssize_t n=write(output,line+sent,(size_t)(length-sent));
          if(n<0 && errno==EINTR) continue;
          if(n<=0) break;
          sent+=n;
       }
    }
}
static void *worker(void *unused) {
    (void)unused;
    for(unsigned i=0;i<20;i++) {
       sleep(1);
       if(getpid()!=owner) break;
       sample(i+1);
    }
    close(output);
    return NULL;
}
__attribute__((constructor)) static void initialize(void) {
    const char *expected=getenv("RSS_LEDGER_EXECUTABLE");
    if(!expected || strcmp(expected,getprogname())) return;
    /* Children do not inherit the injected library or diagnostic selectors. */
    unsetenv("DYLD_INSERT_LIBRARIES");
    const char *path=getenv("RSS_LEDGER_OUTPUT");
    if(!path) return; /* loaded-only interference control */
    output=open(path,O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC,0600);
    if(output<0) return;
    owner=getpid();
    pthread_attr_t attr;
    pthread_t thread;
    pthread_attr_init(&attr);
    pthread_attr_setstacksize(&attr,64*1024);
    pthread_attr_setdetachstate(&attr,PTHREAD_CREATE_DETACHED);
    int result=pthread_create(&thread,&attr,worker,NULL);
    pthread_attr_destroy(&attr);
    if(result) { close(output);output=-1; }
}
