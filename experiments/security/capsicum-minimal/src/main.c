#include <sys/capsicum.h>
#include <sys/wait.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

int main(void)
{
    pid_t child = fork();
    if (child < 0) {
        perror("fork");
        return EXIT_FAILURE;
    }

    if (child == 0) {
        if (cap_enter() != 0) {
            perror("cap_enter");
            _exit(EXIT_FAILURE);
        }

        errno = 0;
        int fd = open("/etc/passwd", O_RDONLY);
        if (fd >= 0) {
            (void)close(fd);
            (void)fprintf(stderr, "FAIL: path-based open succeeded in capability mode\n");
            _exit(EXIT_FAILURE);
        }

        if (errno != ECAPMODE && errno != ENOTCAPABLE) {
            perror("open after cap_enter");
            _exit(EXIT_FAILURE);
        }

        (void)printf("PASS: path-based open denied after cap_enter (errno=%d)\n", errno);
        (void)fflush(stdout);
        _exit(EXIT_SUCCESS);
    }

    int status = 0;
    if (waitpid(child, &status, 0) < 0) {
        perror("waitpid");
        return EXIT_FAILURE;
    }

    if (!WIFEXITED(status) || WEXITSTATUS(status) != EXIT_SUCCESS) {
        (void)fprintf(stderr, "FAIL: capability-mode child did not pass\n");
        return EXIT_FAILURE;
    }

    (void)puts("PASS: minimal Capsicum negative test completed");
    return EXIT_SUCCESS;
}
