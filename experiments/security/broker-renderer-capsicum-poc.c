/*
 * Broker/Renderer process-boundary prototype for FreeBSD Capsicum.
 * The broker retains an unrestricted process; the child receives one IPC FD,
 * limits that FD's rights, enters capability mode, and verifies denied actions.
 */
#include <sys/capsicum.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <errno.h>
#include <arpa/inet.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define RENDERER_IPC_FD 3

static int renderer_process(int inherited_fd)
{
    if (dup2(inherited_fd, RENDERER_IPC_FD) < 0) {
        perror("dup2 renderer IPC");
        return EXIT_FAILURE;
    }
    if (inherited_fd != RENDERER_IPC_FD) {
        (void)close(inherited_fd);
    }
    closefrom(RENDERER_IPC_FD + 1);

    cap_rights_t rights;
    cap_rights_init(&rights, CAP_READ, CAP_WRITE, CAP_FSTAT);
    if (cap_rights_limit(RENDERER_IPC_FD, &rights) != 0) {
        perror("cap_rights_limit renderer IPC");
        return EXIT_FAILURE;
    }
    if (cap_enter() != 0) {
        perror("cap_enter renderer");
        return EXIT_FAILURE;
    }

    char request[32] = {0};
    if (read(RENDERER_IPC_FD, request, sizeof(request) - 1) <= 0) {
        perror("renderer read IPC");
        return EXIT_FAILURE;
    }
    if (strcmp(request, "health.get") != 0) {
        (void)fprintf(stderr, "FAIL: unexpected broker request: %s\n", request);
        return EXIT_FAILURE;
    }
    if (write(RENDERER_IPC_FD, "ok", 2) != 2) {
        perror("renderer write IPC");
        return EXIT_FAILURE;
    }

    errno = 0;
    int path_fd = open("/etc/passwd", O_RDONLY);
    if (path_fd >= 0) {
        (void)close(path_fd);
        (void)fprintf(stderr, "FAIL: renderer opened a host path in capability mode\n");
        return EXIT_FAILURE;
    }
    if (errno != ECAPMODE) {
        (void)fprintf(stderr, "FAIL: path open returned errno=%d, expected ECAPMODE\n", errno);
        return EXIT_FAILURE;
    }

    int network_fd = socket(AF_INET, SOCK_STREAM, 0);
    if (network_fd < 0) {
        if (errno != ECAPMODE && errno != ENOTCAPABLE) {
            (void)fprintf(stderr, "FAIL: socket() returned unexpected errno=%d\n", errno);
            return EXIT_FAILURE;
        }
    } else {
        cap_rights_t network_rights;
        cap_rights_init(&network_rights, CAP_READ, CAP_WRITE, CAP_FSTAT);
        if (cap_rights_limit(network_fd, &network_rights) != 0) {
            perror("cap_rights_limit renderer network socket");
            (void)close(network_fd);
            return EXIT_FAILURE;
        }

        struct sockaddr_in destination;
        memset(&destination, 0, sizeof(destination));
        destination.sin_len = sizeof(destination);
        destination.sin_family = AF_INET;
        destination.sin_port = htons(443);
        if (inet_pton(AF_INET, "1.1.1.1", &destination.sin_addr) != 1) {
            (void)fprintf(stderr, "FAIL: test IPv4 address parse failed\n");
            (void)close(network_fd);
            return EXIT_FAILURE;
        }

        errno = 0;
        int connect_result = connect(network_fd, (struct sockaddr *)&destination, sizeof(destination));
        int connect_errno = errno;
        (void)close(network_fd);
        if (connect_result == 0 || (connect_errno != ENOTCAPABLE && connect_errno != ECAPMODE)) {
            (void)fprintf(stderr, "FAIL: outbound connect returned result=%d errno=%d; expected capability denial\n", connect_result, connect_errno);
            return EXIT_FAILURE;
        }
    }

    (void)puts("PASS: renderer IPC works; path open denied; outbound connect denied by Capsicum rights");
    (void)fflush(stdout);
    return EXIT_SUCCESS;
}

int main(void)
{
    int channels[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, channels) != 0) {
        perror("socketpair broker/renderer");
        return EXIT_FAILURE;
    }

    pid_t child = fork();
    if (child < 0) {
        perror("fork renderer");
        (void)close(channels[0]);
        (void)close(channels[1]);
        return EXIT_FAILURE;
    }

    if (child == 0) {
        (void)close(channels[0]);
        _exit(renderer_process(channels[1]));
    }

    (void)close(channels[1]);
    if (write(channels[0], "health.get", 10) != 10) {
        perror("broker write IPC");
        (void)close(channels[0]);
        return EXIT_FAILURE;
    }
    char response[3] = {0};
    if (read(channels[0], response, 2) != 2 || strcmp(response, "ok") != 0) {
        (void)fprintf(stderr, "FAIL: broker received an invalid renderer response\n");
        (void)close(channels[0]);
        return EXIT_FAILURE;
    }
    (void)close(channels[0]);

    int status = 0;
    if (waitpid(child, &status, 0) < 0) {
        perror("waitpid renderer");
        return EXIT_FAILURE;
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != EXIT_SUCCESS) {
        (void)fprintf(stderr, "FAIL: renderer process did not pass all restrictions\n");
        return EXIT_FAILURE;
    }

    (void)puts("PASS: broker/renderer IPC and Capsicum process-boundary prototype completed");
    return EXIT_SUCCESS;
}
