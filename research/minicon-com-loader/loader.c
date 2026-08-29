/* minicon.com trampoline: pick six-cell payload, exec. Not the terminal.
 * cosmocc fat: ISA is compile-time, OS is runtime — never #ifdef _WIN32.
 */
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <spawn.h>
#include <unistd.h>

#ifdef __COSMOPOLITAN__
#include <cosmo.h>
#endif

extern char **environ;

static const char *host_cell(void) {
#ifdef __COSMOPOLITAN__
#if defined(__aarch64__)
    if (IsXnu()) return "osx-aarch64";
    if (IsLinux()) return "lnx-aarch64";
    if (IsWindows()) return "win-aarch64";
#else
    if (IsXnu()) return "osx-x86_64";
    if (IsLinux()) return "lnx-x86_64";
    if (IsWindows()) return "win-x86_64";
#endif
    return NULL;
#else
#if defined(__APPLE__) && defined(__aarch64__)
    return "osx-aarch64";
#elif defined(__APPLE__) && defined(__x86_64__)
    return "osx-x86_64";
#elif defined(__linux__) && defined(__aarch64__)
    return "lnx-aarch64";
#elif defined(__linux__) && defined(__x86_64__)
    return "lnx-x86_64";
#else
    return NULL;
#endif
#endif
}

static int is_windows_cell(const char *cell) {
    return cell && strncmp(cell, "win-", 4) == 0;
}

static void join3(char *out, size_t n, const char *a, const char *b, const char *c) {
    snprintf(out, n, "%s/%s/%s", a, b, c);
}

static int copy_file(const char *src, const char *dst) {
    FILE *in = fopen(src, "rb");
    FILE *out;
    char buf[1 << 16];
    size_t n;
    if (!in) return -1;
    out = fopen(dst, "wb");
    if (!out) {
        fclose(in);
        return -1;
    }
    while ((n = fread(buf, 1, sizeof buf, in)) > 0) {
        if (fwrite(buf, 1, n, out) != n) {
            fclose(in);
            fclose(out);
            return -1;
        }
    }
    fclose(in);
    fclose(out);
    return 0;
}

static void payload_dest(char *dst, size_t n, const char *cell, const char *leaf) {
#ifdef __COSMOPOLITAN__
    if (IsWindows()) {
        /* Fixed public path: /tmp + execv overlay is unreliable on Win32. */
        snprintf(dst, n, "/C/Users/Public/minicon-payload.exe");
        return;
    }
#endif
    snprintf(dst, n, "/tmp/minicon-%s-%s", cell, leaf);
}

static int run_payload(const char *dst, char **nargv) {
#ifdef __COSMOPOLITAN__
    if (IsWindows()) {
        pid_t pid;
        int st;
        if (posix_spawn(&pid, dst, NULL, NULL, nargv, environ) != 0) {
            fprintf(stderr, "minicon.com: posix_spawn %s: %s\n", dst, strerror(errno));
            return 6;
        }
        if (waitpid(pid, &st, 0) < 0) {
            fprintf(stderr, "minicon.com: waitpid: %s\n", strerror(errno));
            return 6;
        }
        if (WIFEXITED(st)) return WEXITSTATUS(st);
        return 7;
    }
#endif
    execv(dst, nargv);
    fprintf(stderr, "minicon.com: execv %s: %s\n", dst, strerror(errno));
    return 6;
}

int main(int argc, char **argv) {
    const char *cell = host_cell();
    const char *root = getenv("MINICON_COM_CELLS");
    const char *leaf;
    char src[1024];
    char dst[1024];
    struct stat st;
    char **nargv;
    int i;

    if (!cell) {
        fprintf(stderr, "minicon.com: unknown host cell\n");
        return 2;
    }
    leaf = is_windows_cell(cell) ? "minicon.exe" : "minicon";
    if (!root) {
#ifdef __COSMOPOLITAN__
        root = "/zip/cells";
#else
        fprintf(stderr, "minicon.com: set MINICON_COM_CELLS to packed cells dir\n");
        return 2;
#endif
    }

    join3(src, sizeof src, root, cell, leaf);
    if (stat(src, &st) != 0) {
        fprintf(stderr, "minicon.com: missing payload %s (%s)\n", src, strerror(errno));
        return 3;
    }

    payload_dest(dst, sizeof dst, cell, leaf);
    if (copy_file(src, dst) != 0) {
        fprintf(stderr, "minicon.com: extract %s -> %s failed\n", src, dst);
        return 4;
    }
    if (chmod(dst, 0755) != 0) {
        fprintf(stderr, "minicon.com: chmod %s: %s\n", dst, strerror(errno));
        return 4;
    }
    nargv = calloc((size_t)argc + 1, sizeof *nargv);
    if (!nargv) return 5;
    nargv[0] = dst;
    for (i = 1; i < argc; i++) nargv[i] = argv[i];
    return run_payload(dst, nargv);
}
