/*
 * Per-origin storage descriptor isolation proof for a FreeBSD renderer.
 * The broker owns both origin directories but passes only Origin A's directory
 * descriptor to the renderer. The renderer enters Capsicum and must not read
 * Origin B through path traversal or create files in Origin A's storage.
 */
#include <sys/capsicum.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define ORIGIN_STORAGE_FD 3

static int write_secret(const char *path, const char *value)
{
    int fd = open(path, O_WRONLY | O_CREAT | O_EXCL, 0600);
    if (fd < 0) return -1;
    size_t length = strlen(value);
    int result = write(fd, value, length) == (ssize_t)length ? 0 : -1;
    (void)close(fd);
    return result;
}

static int renderer_read_origin_a(int inherited_fd)
{
    if (dup2(inherited_fd, ORIGIN_STORAGE_FD) < 0) {
        perror("dup2 origin storage");
        return EXIT_FAILURE;
    }
    if (inherited_fd != ORIGIN_STORAGE_FD) (void)close(inherited_fd);
    closefrom(ORIGIN_STORAGE_FD + 1);

    cap_rights_t rights;
    cap_rights_init(&rights, CAP_LOOKUP, CAP_READ, CAP_FSTAT);
    if (cap_rights_limit(ORIGIN_STORAGE_FD, &rights) != 0) {
        perror("limit origin storage rights");
        return EXIT_FAILURE;
    }
    if (cap_enter() != 0) {
        perror("renderer cap_enter");
        return EXIT_FAILURE;
    }

    int own_file = openat(ORIGIN_STORAGE_FD, "secret.txt", O_RDONLY);
    if (own_file < 0) {
        perror("renderer read own origin storage");
        return EXIT_FAILURE;
    }
    char value[32] = {0};
    ssize_t count = read(own_file, value, sizeof(value) - 1);
    (void)close(own_file);
    if (count != 8 || strcmp(value, "origin-A") != 0) {
        (void)fprintf(stderr, "FAIL: renderer did not read Origin A storage\n");
        return EXIT_FAILURE;
    }

    errno = 0;
    int other_origin = openat(ORIGIN_STORAGE_FD, "../origin-b/secret.txt", O_RDONLY);
    if (other_origin >= 0) {
        (void)close(other_origin);
        (void)fprintf(stderr, "FAIL: renderer escaped its Origin A storage directory\n");
        return EXIT_FAILURE;
    }
    if (errno != ENOTCAPABLE && errno != ECAPMODE && errno != ENOENT) {
        (void)fprintf(stderr, "FAIL: cross-origin openat returned unexpected errno=%d\n", errno);
        return EXIT_FAILURE;
    }

    errno = 0;
    int created = openat(ORIGIN_STORAGE_FD, "created.txt", O_WRONLY | O_CREAT, 0600);
    if (created >= 0) {
        (void)close(created);
        (void)fprintf(stderr, "FAIL: renderer created a file without write/create rights\n");
        return EXIT_FAILURE;
    }
    if (errno != ENOTCAPABLE) {
        (void)fprintf(stderr, "FAIL: file creation returned errno=%d, expected ENOTCAPABLE\n", errno);
        return EXIT_FAILURE;
    }

    (void)puts("PASS: renderer reads Origin A; Origin B traversal and file creation are denied");
    (void)fflush(stdout);
    return EXIT_SUCCESS;
}

int main(void)
{
    char root[] = "/tmp/aibe-origin-storage.XXXXXX";
    if (mkdtemp(root) == NULL) {
        perror("mkdtemp");
        return EXIT_FAILURE;
    }

    char origin_a[PATH_MAX];
    char origin_b[PATH_MAX];
    char file_a[PATH_MAX];
    char file_b[PATH_MAX];
    (void)snprintf(origin_a, sizeof(origin_a), "%s/origin-a", root);
    (void)snprintf(origin_b, sizeof(origin_b), "%s/origin-b", root);
    (void)snprintf(file_a, sizeof(file_a), "%s/secret.txt", origin_a);
    (void)snprintf(file_b, sizeof(file_b), "%s/secret.txt", origin_b);

    if (mkdir(origin_a, 0700) != 0 || mkdir(origin_b, 0700) != 0 ||
        write_secret(file_a, "origin-A") != 0 || write_secret(file_b, "origin-B") != 0) {
        perror("create temporary origin storage");
        return EXIT_FAILURE;
    }

    int origin_a_fd = open(origin_a, O_RDONLY | O_DIRECTORY);
    if (origin_a_fd < 0) {
        perror("open Origin A directory");
        return EXIT_FAILURE;
    }

    pid_t child = fork();
    if (child < 0) {
        perror("fork renderer");
        (void)close(origin_a_fd);
        return EXIT_FAILURE;
    }
    if (child == 0) {
        _exit(renderer_read_origin_a(origin_a_fd));
    }
    (void)close(origin_a_fd);

    int status = 0;
    if (waitpid(child, &status, 0) < 0 || !WIFEXITED(status) || WEXITSTATUS(status) != EXIT_SUCCESS) {
        (void)fprintf(stderr, "FAIL: Origin A renderer isolation test failed\n");
        return EXIT_FAILURE;
    }

    (void)unlink(file_a);
    (void)unlink(file_b);
    (void)rmdir(origin_a);
    (void)rmdir(origin_b);
    (void)rmdir(root);
    (void)puts("PASS: broker passed only the per-origin storage capability");
    return EXIT_SUCCESS;
}
