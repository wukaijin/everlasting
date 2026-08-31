#include <stdio.h>
#include <linux/landlock.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <errno.h>
#include <string.h>
int main(void) {
    int v = syscall(SYS_landlock_create_ruleset, NULL, 0, LANDLOCK_CREATE_RULESET_VERSION);
    if (v < 0) { printf("Landlock 不可用: %s\n", strerror(errno)); return 1; }
    printf("Landlock ABI v%d 可用\n", v);
    return 0;
}
