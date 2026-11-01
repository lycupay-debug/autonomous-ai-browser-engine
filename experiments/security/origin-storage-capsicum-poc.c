/*
 * Per-origin storage descriptor isolation proof for a FreeBSD renderer.
 * The broker owns both origin directories but transfers only Origin A's
 * directory descriptor over the renderer IPC socket using SCM_RIGHTS.
 * The renderer closes inherited copies, accepts one descriptor, restricts
 * its rights, enters Capsicum, and verifies cross-origin traversal is denied.
 */
#include <sys/capsicum.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/uio.h>
#include <sys/wait.h>
#include <signal.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define RENDERER_IPC_FD 3
#define ORIGIN_STORAGE_FD 4

static int write_secret(const char *path, const char *value)
{
    int fd = open(path, O_WRONLY | O_CREAT | O_EXCL, 0600);
    if (fd < 0) return -1;
    size_t length = strlen(value);
    int result = write(fd, value, length) == (ssize_t)length ? 0 : -1;
    (void)close(fd);
    return result;
}

static int limit_standard_descriptors(void)
{
    cap_rights_t rights;
    cap_rights_init(&rights, CAP_READ);
    if (cap_rights_limit(STDIN_FILENO, &rights) != 0) {
        perror("limit renderer stdin rights");
        return -1;
    }
    cap_rights_init(&rights, CAP_WRITE);
    if (cap_rights_limit(STDOUT_FILENO, &rights) != 0 ||
        cap_rights_limit(STDERR_FILENO, &rights) != 0) {
        perror("limit renderer stdout/stderr rights");
        return -1;
    }
    return 0;
}

static int authorize_renderer_peer(int ipc_fd)
{
    uid_t peer_uid = (uid_t)-1;
    gid_t peer_gid = (gid_t)-1;
    if (getpeereid(ipc_fd, &peer_uid, &peer_gid) != 0) {
        perror("getpeereid renderer IPC peer");
        return -1;
    }
    if (peer_uid != geteuid() || peer_gid != getegid()) {
        (void)fprintf(stderr, "FAIL: renderer peer UID/GID does not match broker policy\n");
        errno = EPERM;
        return -1;
    }
    return 0;
}

static int send_origin_storage_fd(int ipc_fd, int storage_fd)
{
    char marker = 'A';
    struct iovec iov = { .iov_base = &marker, .iov_len = sizeof(marker) };
    char control[CMSG_SPACE(sizeof(int))];
    memset(control, 0, sizeof(control));

    struct msghdr message;
    memset(&message, 0, sizeof(message));
    message.msg_iov = &iov;
    message.msg_iovlen = 1;
    message.msg_control = control;
    message.msg_controllen = sizeof(control);

    struct cmsghdr *header = CMSG_FIRSTHDR(&message);
    if (header == NULL) {
        errno = EINVAL;
        return -1;
    }
    header->cmsg_level = SOL_SOCKET;
    header->cmsg_type = SCM_RIGHTS;
    header->cmsg_len = CMSG_LEN(sizeof(int));
    memcpy(CMSG_DATA(header), &storage_fd, sizeof(storage_fd));

    ssize_t sent = sendmsg(ipc_fd, &message, 0);
    if (sent != (ssize_t)sizeof(marker)) {
        if (sent >= 0) errno = EIO;
        return -1;
    }
    return 0;
}

static void close_received_rights(struct msghdr *message)
{
    for (struct cmsghdr *header = CMSG_FIRSTHDR(message);
         header != NULL;
         header = CMSG_NXTHDR(message, header)) {
        if (header->cmsg_level != SOL_SOCKET || header->cmsg_type != SCM_RIGHTS ||
            header->cmsg_len < CMSG_LEN(0) ||
            header->cmsg_len > message->msg_controllen) continue;
        size_t payload_length = header->cmsg_len - CMSG_LEN(0);
        size_t count = payload_length / sizeof(int);
        for (size_t i = 0; i < count; i++) {
            int fd = -1;
            memcpy(&fd, (char *)CMSG_DATA(header) + i * sizeof(int), sizeof(fd));
            if (fd >= 0) (void)close(fd);
        }
    }
}

static int receive_origin_storage_fd(int ipc_fd)
{
    char marker = 0;
    struct iovec iov = { .iov_base = &marker, .iov_len = sizeof(marker) };
    char control[CMSG_SPACE(sizeof(int) * 4)];
    memset(control, 0, sizeof(control));
    struct msghdr message;
    memset(&message, 0, sizeof(message));
    message.msg_iov = &iov;
    message.msg_iovlen = 1;
    message.msg_control = control;
    message.msg_controllen = sizeof(control);
    ssize_t received = recvmsg(ipc_fd, &message, 0);
    if (received != (ssize_t)sizeof(marker) || marker != 'A' ||
        (message.msg_flags & (MSG_CTRUNC | MSG_TRUNC)) != 0) {
        close_received_rights(&message);
        errno = EPROTO;
        return -1;
    }
    struct cmsghdr *header = CMSG_FIRSTHDR(&message);
    if (header == NULL || header->cmsg_level != SOL_SOCKET ||
        header->cmsg_type != SCM_RIGHTS ||
        header->cmsg_len != CMSG_LEN(sizeof(int)) ||
        CMSG_NXTHDR(&message, header) != NULL) {
        close_received_rights(&message);
        errno = EPROTO;
        return -1;
    }
    int received_fd = -1;
    memcpy(&received_fd, CMSG_DATA(header), sizeof(received_fd));
    if (received_fd < 0 || received_fd != ORIGIN_STORAGE_FD) {
        close_received_rights(&message);
        errno = EPROTO;
        return -1;
    }
    if (fcntl(received_fd, F_SETFD, FD_CLOEXEC) != 0) {
        int saved_errno = errno;
        close_received_rights(&message);
        errno = saved_errno;
        return -1;
    }
    return received_fd;
}

static int test_rejects_multiple_storage_fds(void)
{
    int channels[2] = {-1, -1};
    int sources[2] = {STDIN_FILENO, STDOUT_FILENO};
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, channels) != 0) return -1;
    pid_t child = fork();
    if (child < 0) { (void)close(channels[0]); (void)close(channels[1]); return -1; }
    if (child == 0) {
        (void)close(channels[0]);
        if (dup2(channels[1], RENDERER_IPC_FD) < 0) _exit(EXIT_FAILURE);
        if (channels[1] != RENDERER_IPC_FD) (void)close(channels[1]);
        closefrom(RENDERER_IPC_FD + 1);
        int result = receive_origin_storage_fd(RENDERER_IPC_FD);
        if (result >= 0) { (void)close(result); _exit(EXIT_FAILURE); }
        errno = 0;
        if (fcntl(ORIGIN_STORAGE_FD, F_GETFD) != -1 || errno != EBADF) _exit(EXIT_FAILURE);
        errno = 0;
        if (fcntl(ORIGIN_STORAGE_FD + 1, F_GETFD) != -1 || errno != EBADF) _exit(EXIT_FAILURE);
        _exit(EXIT_SUCCESS);
    }
    (void)close(channels[1]);
    char marker = 'A';
    struct iovec iov = { .iov_base = &marker, .iov_len = sizeof(marker) };
    char control[CMSG_SPACE(sizeof(sources))];
    memset(control, 0, sizeof(control));
    struct msghdr message;
    memset(&message, 0, sizeof(message));
    message.msg_iov = &iov;
    message.msg_iovlen = 1;
    message.msg_control = control;
    message.msg_controllen = sizeof(control);
    struct cmsghdr *header = CMSG_FIRSTHDR(&message);
    if (header == NULL) { (void)close(channels[0]); (void)waitpid(child, NULL, 0); return -1; }
    header->cmsg_level = SOL_SOCKET;
    header->cmsg_type = SCM_RIGHTS;
    header->cmsg_len = CMSG_LEN(sizeof(sources));
    memcpy(CMSG_DATA(header), sources, sizeof(sources));
    ssize_t sent = sendmsg(channels[0], &message, 0);
    (void)close(channels[0]);
    int status = 0;
    if (waitpid(child, &status, 0) < 0 || sent != (ssize_t)sizeof(marker) ||
        !WIFEXITED(status) || WEXITSTATUS(status) != EXIT_SUCCESS) return -1;
    (void)puts("PASS: malformed multi-FD SCM_RIGHTS message rejected; all received descriptors closed");
    (void)fflush(stdout);
    return 0;
}

static int renderer_read_origin_a(int inherited_ipc_fd, int inherited_origin_a_fd, int inherited_origin_b_fd)
{
    if (dup2(inherited_ipc_fd, RENDERER_IPC_FD) < 0) {
        perror("dup2 renderer IPC");
        return EXIT_FAILURE;
    }
    if (inherited_ipc_fd != RENDERER_IPC_FD) (void)close(inherited_ipc_fd);
    if (inherited_origin_a_fd >= 0) (void)close(inherited_origin_a_fd);
    if (inherited_origin_b_fd >= 0) (void)close(inherited_origin_b_fd);
    closefrom(RENDERER_IPC_FD + 1);

    errno = 0;
    if (fcntl(ORIGIN_STORAGE_FD, F_GETFD) != -1 || errno != EBADF) {
        (void)fprintf(stderr, "FAIL: inherited origin descriptor remained open before SCM_RIGHTS receive\n");
        return EXIT_FAILURE;
    }
    if (limit_standard_descriptors() != 0) return EXIT_FAILURE;

    int storage_fd = receive_origin_storage_fd(RENDERER_IPC_FD);
    if (storage_fd < 0) {
        perror("renderer receive Origin A storage FD");
        return EXIT_FAILURE;
    }

    cap_rights_t ipc_rights;
    cap_rights_init(&ipc_rights, CAP_READ, CAP_WRITE, CAP_FSTAT);
    if (cap_rights_limit(RENDERER_IPC_FD, &ipc_rights) != 0) {
        perror("limit renderer IPC rights");
        return EXIT_FAILURE;
    }

    cap_rights_t storage_rights;
    cap_rights_init(&storage_rights, CAP_LOOKUP, CAP_READ, CAP_FSTAT);
    if (cap_rights_limit(storage_fd, &storage_rights) != 0) {
        perror("limit origin storage rights");
        return EXIT_FAILURE;
    }
    if (cap_enter() != 0) {
        perror("renderer cap_enter");
        return EXIT_FAILURE;
    }

    int own_file = openat(storage_fd, "secret.txt", O_RDONLY);
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
    int other_origin = openat(storage_fd, "../origin-b/secret.txt", O_RDONLY);
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
    int created = openat(storage_fd, "created.txt", O_WRONLY | O_CREAT, 0600);
    if (created >= 0) {
        (void)close(created);
        (void)fprintf(stderr, "FAIL: renderer created a file without write/create rights\n");
        return EXIT_FAILURE;
    }
    if (errno != ENOTCAPABLE) {
        (void)fprintf(stderr, "FAIL: file creation returned errno=%d, expected ENOTCAPABLE\n", errno);
        return EXIT_FAILURE;
    }

    if (write(RENDERER_IPC_FD, "ready", 5) != 5) {
        perror("renderer ready IPC");
        return EXIT_FAILURE;
    }
    (void)puts("PASS: renderer received only Origin A via SCM_RIGHTS; Origin B traversal and writes denied");
    (void)fflush(stdout);
    (void)close(storage_fd);
    (void)close(RENDERER_IPC_FD);
    return EXIT_SUCCESS;
}

int main(void)
{
    char root[] = "/tmp/aibe-origin-storage.XXXXXX";
    char origin_a[PATH_MAX];
    char origin_b[PATH_MAX];
    char file_a[PATH_MAX];
    char file_b[PATH_MAX];
    int origin_a_fd = -1;
    int origin_b_fd = -1;
    int channels[2] = {-1, -1};
    pid_t child = -1;
    int status = 0;
    int result = EXIT_FAILURE;
    int root_created = 0;

    if (mkdtemp(root) == NULL) {
        perror("mkdtemp");
        return EXIT_FAILURE;
    }
    root_created = 1;
    if (test_rejects_multiple_storage_fds() != 0) {
        (void)fprintf(stderr, "FAIL: malformed SCM_RIGHTS descriptor cleanup test\n");
        goto cleanup;
    }
    (void)snprintf(origin_a, sizeof(origin_a), "%s/origin-a", root);
    (void)snprintf(origin_b, sizeof(origin_b), "%s/origin-b", root);
    (void)snprintf(file_a, sizeof(file_a), "%s/secret.txt", origin_a);
    (void)snprintf(file_b, sizeof(file_b), "%s/secret.txt", origin_b);

    if (mkdir(origin_a, 0700) != 0 || mkdir(origin_b, 0700) != 0 ||
        write_secret(file_a, "origin-A") != 0 || write_secret(file_b, "origin-B") != 0) {
        perror("create temporary origin storage");
        goto cleanup;
    }

    origin_a_fd = open(origin_a, O_RDONLY | O_DIRECTORY);
    origin_b_fd = open(origin_b, O_RDONLY | O_DIRECTORY);
    if (origin_a_fd < 0 || origin_b_fd < 0) {
        perror("open origin storage directories");
        goto cleanup;
    }
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, channels) != 0) {
        perror("create broker/renderer IPC socketpair");
        goto cleanup;
    }

    child = fork();
    if (child < 0) {
        perror("fork renderer");
        goto cleanup;
    }
    if (child == 0) {
        (void)close(channels[0]);
        _exit(renderer_read_origin_a(channels[1], origin_a_fd, origin_b_fd));
    }

    (void)close(channels[1]);
    channels[1] = -1;
    if (authorize_renderer_peer(channels[0]) != 0) {
        perror("authorize renderer peer before storage grant");
        goto cleanup;
    }
    if (send_origin_storage_fd(channels[0], origin_a_fd) != 0) {
        perror("broker send Origin A storage FD");
        goto cleanup;
    }
    (void)close(origin_a_fd);
    origin_a_fd = -1;
    (void)close(origin_b_fd);
    origin_b_fd = -1;

    char response[5] = {0};
    if (read(channels[0], response, sizeof(response)) != (ssize_t)sizeof(response) ||
        memcmp(response, "ready", sizeof(response)) != 0) {
        (void)fprintf(stderr, "FAIL: broker did not receive renderer ready response\n");
        goto cleanup;
    }

    if (waitpid(child, &status, 0) < 0) {
        perror("waitpid renderer");
        child = -1;
        goto cleanup;
    }
    child = -1;
    if (!WIFEXITED(status) || WEXITSTATUS(status) != EXIT_SUCCESS) {
        (void)fprintf(stderr, "FAIL: Origin A renderer isolation test failed\n");
        goto cleanup;
    }

    (void)puts("PASS: broker transferred only the authorized per-origin storage FD");
    result = EXIT_SUCCESS;

cleanup:
    if (child > 0) {
        (void)kill(child, SIGKILL);
        (void)waitpid(child, NULL, 0);
    }
    if (channels[0] >= 0) (void)close(channels[0]);
    if (channels[1] >= 0) (void)close(channels[1]);
    if (origin_a_fd >= 0) (void)close(origin_a_fd);
    if (origin_b_fd >= 0) (void)close(origin_b_fd);
    (void)unlink(file_a);
    (void)unlink(file_b);
    (void)unlinkat(AT_FDCWD, origin_a, AT_REMOVEDIR);
    (void)unlinkat(AT_FDCWD, origin_b, AT_REMOVEDIR);
    if (root_created) (void)rmdir(root);
    return result;
}
