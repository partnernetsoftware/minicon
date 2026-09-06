#include <dlfcn.h>
int main(void){void *p=dlopen("/usr/lib/libSystem.B.dylib",RTLD_NOW);if(!p)return 2;return dlclose(p);}
