#include <sys/capsicum.h>
#include <sys/wait.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static int run_restricted_child(int inherited_fd)
{
    cap_rights_t rights;
    cap_rights_init(&rights, CAP_READ, CAP_FSTAT);
    if (cap_rights_limit(inherited_fd, &rights) != 0) {
        perror("cap_rights_limit");
        return EXIT_FAILURE;
    }

    if (cap_enter() != 0) {
        perror("cap_enter");
        return EXIT_FAILURE;
    }

    char buffer[6] = {0};
    if (read(inherited_fd, buffer, 5) != 5 || strcmp(buffer, "hello") != 0) {
        (void)fprintf(stderr, "FAIL: read through the restricted inherited descriptor\n");
        return EXIT_FAILURE;
    }

    errno = 0;
    if (write(inherited_fd, "X", 1) >= 0 || errno != ENOTCAPABLE) {
        (void)fprintf(stderr, "FAIL: write was not denied by descriptor rights (errno=%d)\n", errno);
        return EXIT_FAILURE;
    }

    errno = 0;
    int denied_fd = open("/etc/passwd", O_RDONLY);
    if (denied_fd >= 0) {
        (void)close(denied_fd);
        (void)fprintf(stderr, "FAIL: path-based open succeeded in capability mode\n");
        return EXIT_FAILURE;
    }
    if (errno != ECAPMODE) {
        (void)fprintf(stderr, "FAIL: path-based open returned unexpected errno=%d\n", errno);
        return EXIT_FAILURE;
    }

    (void)puts("PASS: inherited FD read allowed; FD write denied (ENOTCAPABLE); path open denied (ECAPMODE)");
    (void)fflush(stdout);
    return EXIT_SUCCESS;
}

int main(void)
{
    char template[] = "/tmp/aibe-capsicum-XXXXXX";
    int inherited_fd = mkstemp(template);
    if (inherited_fd < 0) {
        perror("mkstemp");
        return EXIT_FAILURE;
    }
    (void)unlink(template);

    if (write(inherited_fd, "hello", 5) != 5 || lseek(inherited_fd, 0, SEEK_SET) < 0) {
        perror("prepare inherited descriptor");
        (void)close(inherited_fd);
        return EXIT_FAILURE;
    }

    pid_t child = fork();
    if (child < 0) {
        perror("fork");
        (void)close(inherited_fd);
        return EXIT_FAILURE;
    }

    if (child == 0) {
        int result = run_restricted_child(inherited_fd);
        _exit(result);
    }

    int status = 0;
    if (waitpid(child, &status, 0) < 0) {
        perror("waitpid");
        (void)close(inherited_fd);
        return EXIT_FAILURE;
    }
    (void)close(inherited_fd);

    if (!WIFEXITED(status) || WEXITSTATUS(status) != EXIT_SUCCESS) {
        (void)fprintf(stderr, "FAIL: restricted child did not pass\n");
        return EXIT_FAILURE;
    }

    (void)puts("PASS: minimal Capsicum descriptor-rights and capability-mode test completed");
    return EXIT_SUCCESS;
}
