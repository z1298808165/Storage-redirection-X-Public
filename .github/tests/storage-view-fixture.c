// 仅供隔离测试：在指定测试应用的 namespace 中构造迟到的系统视图，不修改生产开关。
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <sched.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/sysmacros.h>
#include <unistd.h>

#ifndef OPEN_TREE_CLONE
#define OPEN_TREE_CLONE 1
#endif
#ifndef MOVE_MOUNT_F_EMPTY_PATH
#define MOVE_MOUNT_F_EMPTY_PATH 4
#endif

static void fail(const char *step) {
    perror(step);
    exit(1);
}

static bool parse_id(const char *text, int *value) {
    char *end;
    errno = 0;
    long number = strtol(text, &end, 10);
    if (*text < '0' || *text > '9' || *end || errno || number > INT_MAX) return false;
    *value = (int)number;
    return true;
}

int main(int argc, char **argv) {
    if (argc != 7 || getuid() != 0) {
        fprintf(stderr, "usage: storage-view-fixture pid uid user target mode expected-dev\n");
        return 2;
    }
    int pid, uid, user;
    if (!parse_id(argv[1], &pid) || !parse_id(argv[2], &uid) || !parse_id(argv[3], &user)) return 2;
    const char *target = argv[4], *mode = argv[5];
    char path[256], expected[256], command[256] = {0};
    struct stat proc;
    snprintf(path, sizeof(path), "/proc/%d", pid);
    if (pid <= 1 || uid < 10000 || user < 0 || uid / 100000 != user ||
        stat(path, &proc) || proc.st_uid != (uid_t)uid) {
        fprintf(stderr, "target process identity mismatch\n");
        return 2;
    }
    snprintf(path, sizeof(path), "/proc/%d/cmdline", pid);
    FILE *file = fopen(path, "r");
    if (!file || !fgets(command, sizeof(command), file)) fail("cmdline");
    fclose(file);
    if (strcmp(command, "me.fakerqu.test.storageredirect")) {
        fprintf(stderr, "only the storage test app is supported\n");
        return 2;
    }
    bool allowed = !strcmp(target, "/storage/emulated");
    snprintf(expected, sizeof(expected), "/storage/emulated/%d", user);
    allowed |= !strcmp(mode, "missing") && !strcmp(target, expected);
    const char *views[] = {"user", "installer", "androidwritable"};
    for (size_t i = 0; i < sizeof(views) / sizeof(*views); ++i) {
        snprintf(expected, sizeof(expected), "/mnt/%s/%d/emulated", views[i], user);
        allowed |= !strcmp(target, expected);
        snprintf(expected, sizeof(expected), "/mnt/%s/%d/emulated/%d", views[i], user, user);
        allowed |= !strcmp(mode, "missing") && !strcmp(target, expected);
    }
    if (!allowed || (strcmp(mode, "cover") && strcmp(mode, "replace") &&
                     strcmp(mode, "missing") && strcmp(mode, "foreign"))) {
        fprintf(stderr, "invalid target or mode\n");
        return 2;
    }
    unsigned major_id, minor_id;
    int consumed = 0;
    if (sscanf(argv[6], "%u:%u%n", &major_id, &minor_id, &consumed) != 2 || argv[6][consumed]) return 2;
    dev_t host = makedev(major_id, minor_id);
    snprintf(path, sizeof(path), "/proc/%d/ns/mnt", pid);
    int app_ns = open(path, O_RDONLY | O_CLOEXEC);
    int init_ns = open("/proc/1/ns/mnt", O_RDONLY | O_CLOEXEC);
    struct stat app_stat, init_stat;
    if (app_ns < 0 || init_ns < 0 || fstat(app_ns, &app_stat) || fstat(init_ns, &init_stat)) fail("namespace identity");
    if (app_stat.st_dev == init_stat.st_dev && app_stat.st_ino == init_stat.st_ino) {
        fprintf(stderr, "refusing the init namespace\n");
        return 2;
    }
    if (setns(init_ns, CLONE_NEWNS)) fail("init namespace");
    snprintf(path, sizeof(path), "/mnt/user/%d/emulated", user);
    int tree = (int)syscall(SYS_open_tree, AT_FDCWD, path, OPEN_TREE_CLONE | O_CLOEXEC);
    if (tree < 0) fail("clone system view");
    if (setns(app_ns, CLONE_NEWNS)) fail("app namespace");
    struct stat before;
    if (stat(target, &before) || before.st_dev != host) {
        fprintf(stderr, "fault requires the expected storage view\n");
        return 1;
    }
    if (mount(NULL, target, NULL, MS_PRIVATE, NULL)) fail("isolate target");
    if (!strcmp(mode, "replace") || !strcmp(mode, "missing")) {
        // 只摘测试前确认的 SRX 层，遇到其它超级块立即停止。
        unsigned count = 0;
        struct stat live;
        while (!stat(target, &live) && live.st_dev == host) {
            if (++count > 8 || umount2(target, MNT_DETACH)) fail("detach SRX view");
        }
    }
    if (!strcmp(mode, "foreign")) {
        if (mount("issue18-foreign", target, "tmpfs", MS_NOSUID | MS_NODEV, "mode=0755")) fail("foreign view");
    } else if (strcmp(mode, "missing")) {
        if (syscall(SYS_move_mount, tree, "", AT_FDCWD, target, MOVE_MOUNT_F_EMPTY_PATH)) fail("cover system view");
    }
    close(tree);
    close(init_ns);
    close(app_ns);
    printf("fault_applied mode=%s pid=%d uid=%d user=%d target=%s\n", mode, pid, uid, user, target);
    return 0;
}
