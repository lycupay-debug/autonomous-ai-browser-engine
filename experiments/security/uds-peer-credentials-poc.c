/* Verify FreeBSD getpeereid() reports the UID/GID of a connected UDS peer. */
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <errno.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(void)
{
    if (geteuid() != 0) {
        (void)fprintf(stderr, "This test requires root to drop the child to the nobody account.\n");
        return EXIT_FAILURE;
    }

    struct passwd *nobody = getpwnam("nobody");
    if (nobody == NULL) {
        (void)fprintf(stderr, "nobody account not found\n");
        return EXIT_FAILURE;
    }

    char path[sizeof(((struct sockaddr_un *)0)->sun_path)];
    (void)snprintf(path, sizeof(path), "/tmp/aibe-peer-cred-%ld.sock", (long)getpid());
    int listener = socket(AF_UNIX, SOCK_STREAM, 0);
    if (listener < 0) {
        perror("socket listener");
        return EXIT_FAILURE;
    }

    struct sockaddr_un address;
    memset(&address, 0, sizeof(address));
    address.sun_family = AF_UNIX;
    (void)strlcpy(address.sun_path, path, sizeof(address.sun_path));
    if (bind(listener, (struct sockaddr *)&address, sizeof(address)) != 0 ||
        chmod(path, 0666) != 0 || listen(listener, 1) != 0) {
        perror("bind/chmod/listen");
        (void)close(listener);
        (void)unlink(path);
        return EXIT_FAILURE;
    }

    pid_t child = fork();
    if (child < 0) {
        perror("fork");
        (void)close(listener);
        (void)unlink(path);
        return EXIT_FAILURE;
    }

    if (child == 0) {
        (void)close(listener);
        if (setgid(nobody->pw_gid) != 0 || setuid(nobody->pw_uid) != 0) {
            perror("drop child credentials");
            _exit(EXIT_FAILURE);
        }
        int client = socket(AF_UNIX, SOCK_STREAM, 0);
        if (client < 0 || connect(client, (struct sockaddr *)&address, sizeof(address)) != 0) {
            perror("nobody connect");
            _exit(EXIT_FAILURE);
        }
        if (write(client, "hello", 5) != 5) {
            perror("nobody write");
            _exit(EXIT_FAILURE);
        }
        char response[3] = {0};
        if (read(client, response, 2) != 2 || strcmp(response, "ok") != 0) {
            (void)fprintf(stderr, "nobody received invalid response\n");
            _exit(EXIT_FAILURE);
        }
        (void)close(client);
        _exit(EXIT_SUCCESS);
    }

    int peer = accept(listener, NULL, NULL);
    if (peer < 0) {
        perror("accept peer");
        (void)close(listener);
        (void)unlink(path);
        return EXIT_FAILURE;
    }

    uid_t peer_uid = (uid_t)-1;
    gid_t peer_gid = (gid_t)-1;
    if (getpeereid(peer, &peer_uid, &peer_gid) != 0) {
        perror("getpeereid");
        (void)close(peer);
        (void)close(listener);
        (void)unlink(path);
        return EXIT_FAILURE;
    }

    char request[6] = {0};
    if (read(peer, request, 5) != 5 || strcmp(request, "hello") != 0) {
        (void)fprintf(stderr, "broker received invalid peer request\n");
        (void)close(peer);
        (void)close(listener);
        (void)unlink(path);
        return EXIT_FAILURE;
    }

    if (peer_uid != nobody->pw_uid || peer_gid != nobody->pw_gid) {
        (void)fprintf(stderr, "FAIL: getpeereid returned uid=%u gid=%u; expected uid=%u gid=%u\n",
            (unsigned)peer_uid, (unsigned)peer_gid, (unsigned)nobody->pw_uid, (unsigned)nobody->pw_gid);
        (void)close(peer);
        (void)close(listener);
        (void)unlink(path);
        return EXIT_FAILURE;
    }

    if (write(peer, "ok", 2) != 2) {
        perror("broker response");
        (void)close(peer);
        (void)close(listener);
        (void)unlink(path);
        return EXIT_FAILURE;
    }

    int status = 0;
    if (waitpid(child, &status, 0) < 0 || !WIFEXITED(status) || WEXITSTATUS(status) != EXIT_SUCCESS) {
        (void)fprintf(stderr, "nobody peer process failed\n");
        (void)close(peer);
        (void)close(listener);
        (void)unlink(path);
        return EXIT_FAILURE;
    }

    (void)printf("PASS: getpeereid authenticated UDS peer uid=%u gid=%u\n",
        (unsigned)peer_uid, (unsigned)peer_gid);
    (void)close(peer);
    (void)close(listener);
    (void)unlink(path);
    return EXIT_SUCCESS;
}
