#include <objc/runtime.h>
#include <stdlib.h>
int main(void) {int count=objc_getClassList(NULL,0);unsigned int copied=0;Class *classes=objc_copyClassList(&copied);free(classes);return count<0||copied==0;}
