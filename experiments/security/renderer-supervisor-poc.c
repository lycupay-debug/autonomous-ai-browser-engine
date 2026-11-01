/* Minimal Broker supervisor restart test after a renderer process crash. */
#include <sys/socket.h>
#include <sys/wait.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static pid_t spawn_renderer(int channel, int crash)
{
    pid_t child = fork();
    if (child != 0) return child;
    if (crash) {
        (void)raise(SIGKILL);
        _exit(EXIT_FAILURE);
    }
    char request[16] = {0};
    if (read(channel, request, sizeof(request) - 1) <= 0 || strcmp(request, "health.get") != 0) {
        _exit(EXIT_FAILURE);
    }
    if (write(channel, "healthy", 7) != 7) _exit(EXIT_FAILURE);
    _exit(EXIT_SUCCESS);
}

int main(void)
{
    int first[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, first) != 0) {
        perror("first renderer socketpair");
        return EXIT_FAILURE;
    }
    pid_t crashed = spawn_renderer(first[1], 1);
    (void)close(first[0]);
    (void)close(first[1]);
    if (crashed < 0) {
        perror("spawn first renderer");
        return EXIT_FAILURE;
    }
    int status = 0;
    if (waitpid(crashed, &status, 0) < 0 || !WIFSIGNALED(status) || WTERMSIG(status) != SIGKILL) {
        (void)fprintf(stderr, "FAIL: supervisor did not observe renderer crash\n");
        return EXIT_FAILURE;
    }
    (void)puts("PASS: supervisor observed renderer SIGKILL");

    int restarted[2];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, restarted) != 0) {
        perror("restart renderer socketpair");
        return EXIT_FAILURE;
    }
    pid_t replacement = spawn_renderer(restarted[1], 0);
    (void)close(restarted[1]);
    if (replacement < 0) {
        perror("spawn replacement renderer");
        (void)close(restarted[0]);
        return EXIT_FAILURE;
    }
    if (write(restarted[0], "health.get", 10) != 10) {
        perror("send replacement health request");
        (void)close(restarted[0]);
        return EXIT_FAILURE;
    }
    char response[8] = {0};
    if (read(restarted[0], response, 7) != 7 || strcmp(response, "healthy") != 0) {
        (void)fprintf(stderr, "FAIL: replacement renderer did not become healthy\n");
        (void)close(restarted[0]);
        return EXIT_FAILURE;
    }
    (void)close(restarted[0]);
    if (waitpid(replacement, &status, 0) < 0 || !WIFEXITED(status) || WEXITSTATUS(status) != EXIT_SUCCESS) {
        (void)fprintf(stderr, "FAIL: replacement renderer did not exit cleanly\n");
        return EXIT_FAILURE;
    }
    (void)puts("PASS: supervisor restarted renderer and health IPC succeeded");
    return EXIT_SUCCESS;
}
