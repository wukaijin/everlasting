/* Landlock 沙盒探针:控制 EXECUTE+全部写位(读不控),验证 4 件事 */
#define _GNU_SOURCE
#include <stdio.h>
#include <linux/landlock.h>
#include <sys/syscall.h>
#include <sys/prctl.h>
#include <unistd.h>
#include <errno.h>
#include <string.h>
#include <fcntl.h>
#include <sys/wait.h>

static int ll_fd(void) {
    return syscall(SYS_landlock_create_ruleset, NULL, 0, LANDLOCK_CREATE_RULESET_VERSION);
}
static int add_rule(int fd, const char *path, __u64 access) {
    struct landlock_path_beneath_attr a = {.allowed_access = access};
    a.parent_fd = open(path, O_PATH | O_CLOEXEC);
    if (a.parent_fd < 0) return -1;
    int r = syscall(SYS_landlock_add_rule, fd, LANDLOCK_RULE_PATH_BENEATH, &a, 0);
    close(a.parent_fd);
    return r;
}
static void apply_sandbox(void) {
    int abi = ll_fd();
    struct landlock_ruleset_attr rs = {
        .handled_access_fs =
            LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_WRITE_FILE |
            LANDLOCK_ACCESS_FS_REMOVE_DIR | LANDLOCK_ACCESS_FS_REMOVE_FILE |
            LANDLOCK_ACCESS_FS_MAKE_CHAR | LANDLOCK_ACCESS_FS_MAKE_DIR | LANDLOCK_ACCESS_FS_MAKE_REG |
            LANDLOCK_ACCESS_FS_MAKE_SOCK | LANDLOCK_ACCESS_FS_MAKE_FIFO | LANDLOCK_ACCESS_FS_MAKE_BLOCK |
            LANDLOCK_ACCESS_FS_MAKE_SYM
    };
    int fd = syscall(SYS_landlock_create_ruleset, &rs, sizeof(rs), 0);
    if (fd < 0) { fprintf(stderr, "create_ruleset: %s\n", strerror(errno)); _exit(99); }
    /* exec 允许面:系统 PATH 目录 + 项目 + /tmp;/init 与 /mnt/c 不给 */
    const char *exec_ok[] = {"/usr", "/bin", "/sbin", "/lib", "/lib64", "/tmp/sbx-test/proj", "/tmp", "/home/linuxbrew/.linuxbrew", "/run/user/1000", "/dev", NULL};
    for (int i = 0; exec_ok[i]; i++)
        if (add_rule(fd, exec_ok[i], LANDLOCK_ACCESS_FS_EXECUTE) < 0) { fprintf(stderr, "rule %s: %s\n", exec_ok[i], strerror(errno)); _exit(99); }
    /* 写允许面:项目 + /tmp */
    const char *rw[] = {"/tmp/sbx-test/proj", "/tmp", NULL};
    for (int i = 0; rw[i]; i++)
        if (add_rule(fd, rw[i], LANDLOCK_ACCESS_FS_WRITE_FILE |
                               LANDLOCK_ACCESS_FS_REMOVE_DIR | LANDLOCK_ACCESS_FS_REMOVE_FILE |
                               LANDLOCK_ACCESS_FS_MAKE_DIR | LANDLOCK_ACCESS_FS_MAKE_REG | LANDLOCK_ACCESS_FS_MAKE_SOCK |
                               LANDLOCK_ACCESS_FS_MAKE_FIFO | LANDLOCK_ACCESS_FS_MAKE_SYM | LANDLOCK_ACCESS_FS_MAKE_CHAR) < 0)
        { fprintf(stderr, "rw rule %s: %s\n", rw[i], strerror(errno)); _exit(99); }
    const char *devw[] = {"/dev/null", "/dev/zero", "/dev/full", "/dev/random", "/dev/urandom", "/dev/tty", NULL};
    for (int i = 0; devw[i]; i++)
        if (add_rule(fd, devw[i], LANDLOCK_ACCESS_FS_WRITE_FILE) < 0)
            { fprintf(stderr, "dev rule %s: %s\n", devw[i], strerror(errno)); _exit(99); }
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)) _exit(99);
    if (syscall(SYS_landlock_restrict_self, fd, 0)) { fprintf(stderr, "restrict_self: %s\n", strerror(errno)); _exit(99); }
}
int main(int argc, char **argv) {
    /* argv[1] = 动作: exec <path> | write <path> | read <path> */
    if (argc < 3) return 2;
    if (fork() == 0) {
        apply_sandbox();
        if (!strcmp(argv[1], "exec")) { execl(argv[2], argv[2], (char*)NULL); fprintf(stderr, "  exec 失败: %s\n", strerror(errno)); _exit(1); }
        if (!strcmp(argv[1], "write")) { int f = open(argv[2], O_CREAT|O_WRONLY, 0644); if (f<0) { fprintf(stderr, "  写入被拦: %s\n", strerror(errno)); _exit(1);} write(f, "x", 1); fprintf(stderr, "  写入成功\n"); _exit(0);}
        if (!strcmp(argv[1], "read"))  { int f = open(argv[2], O_RDONLY); if (f<0) { fprintf(stderr,"  读取被拦: %s\n", strerror(errno)); _exit(1);} fprintf(stderr,"  读取成功\n"); _exit(0);}
        _exit(2);
    }
    int st; wait(&st); return WEXITSTATUS(st);
}
