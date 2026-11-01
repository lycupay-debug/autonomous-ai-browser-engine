/*
 * Single-Renderer Jail lifecycle proof for FreeBSD.
 * Host mode creates an isolated rootfs and UDS, starts one low-privilege
 * process in one Jail, authenticates its kernel peer identity, checks its
 * kernel-reported jail membership, verifies filesystem isolation, exercises
 * a jail-scoped RCTL limit, and destroys the Jail.
 */
#include <sys/types.h>
#include <sys/socket.h>
#include <sys/sysctl.h>
#include <sys/ucred.h>
#include <sys/user.h>
#include <sys/un.h>
#include <sys/mman.h>
#include <sys/time.h>
#include <sys/wait.h>

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

#define RENDERER_UID 65534
#define RENDERER_GID 65534
#define MEMORY_PROBE_BYTES (256UL * 1024UL * 1024UL)

static int
write_all(int fd, const void *buffer, size_t length)
{
    const char *cursor = buffer;
    while (length > 0) {
        ssize_t written = write(fd, cursor, length);
        if (written < 0 && errno == EINTR)
            continue;
        if (written <= 0)
            return -1;
        cursor += written;
        length -= (size_t)written;
    }
    return 0;
}

static int
read_all(int fd, void *buffer, size_t length)
{
    char *cursor = buffer;
    while (length > 0) {
        ssize_t received = read(fd, cursor, length);
        if (received < 0 && errno == EINTR)
            continue;
        if (received <= 0)
            return -1;
        cursor += received;
        length -= (size_t)received;
    }
    return 0;
}

static int
renderer_child(const char *socket_path, const char *host_marker)
{
    /* No host listener, broker control FD, or unrelated descriptor may cross exec. */
    for (int inherited_fd = 3; inherited_fd < 1024; inherited_fd++) {
        if (fcntl(inherited_fd, F_GETFD) >= 0 || errno != EBADF)
            return 15;
    }
    struct sockaddr_un address;
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0)
        return 10;
    memset(&address, 0, sizeof(address));
    address.sun_family = AF_UNIX;
    if (strlen(socket_path) >= sizeof(address.sun_path))
        return 11;
    strlcpy(address.sun_path, socket_path, sizeof(address.sun_path));
    if (connect(fd, (struct sockaddr *)&address, sizeof(address)) != 0)
        return 12;

    int marker_visible = access(host_marker, F_OK) == 0;
    char ready[128];
    int ready_length = snprintf(ready, sizeof(ready), "READY uid=%u gid=%u marker_visible=%d\n",
        (unsigned)geteuid(), (unsigned)getegid(), marker_visible);
    if (ready_length <= 0 || (size_t)ready_length >= sizeof(ready) ||
        write_all(fd, ready, (size_t)ready_length) != 0)
        return 13;

    char command;
    if (read_all(fd, &command, 1) != 0 || command != 'M')
        return 14;
    void *memory = mmap(NULL, MEMORY_PROBE_BYTES, PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANON, -1, 0);
    const char *result;
    if (memory == MAP_FAILED) {
        result = errno == ENOMEM ? "MEMORY_DENIED\n" : "MEMORY_ERROR\n";
    } else {
        memset(memory, 0x5a, MEMORY_PROBE_BYTES);
        result = "MEMORY_ALLOWED\n";
        munmap(memory, MEMORY_PROBE_BYTES);
    }
    (void)write_all(fd, result, strlen(result));

    if (read_all(fd, &command, 1) != 0 || command != 'P')
        return 16;
    pid_t first_child = fork();
    if (first_child < 0) {
        result = "PROC_FIRST_FORK_ERROR\n";
    } else if (first_child == 0) {
        for (;;)
            pause();
    } else {
        pid_t second_child = fork();
        if (second_child < 0 && errno == EAGAIN) {
            result = "PROC_DENIED\n";
        } else {
            result = "PROC_ALLOWED\n";
            if (second_child == 0)
                _exit(0);
            if (second_child > 0)
                (void)kill(second_child, SIGKILL);
            (void)waitpid(second_child, NULL, 0);
        }
        (void)kill(first_child, SIGKILL);
        (void)waitpid(first_child, NULL, 0);
    }
    (void)write_all(fd, result, strlen(result));

    /* Keep the Renderer alive so the host can verify forced Jail teardown. */
    char shutdown_command;
    (void)read_all(fd, &shutdown_command, 1);
    close(fd);
    return 0;
}

static int
run_command(char *const argv[])
{
    pid_t pid = fork();
    if (pid < 0)
        return -1;
    if (pid == 0) {
        execv(argv[0], argv);
        _exit(127);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid)
        return -1;
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128;
}

static int
read_jail_id(const char *name)
{
    char command[256];
    (void)snprintf(command, sizeof(command), "/usr/sbin/jls -j %s jid 2>/dev/null", name);
    FILE *pipe = popen(command, "r");
    if (pipe == NULL)
        return -1;
    int jid = -1;
    (void)fscanf(pipe, "%d", &jid);
    int status = pclose(pipe);
    return status == 0 ? jid : -1;
}

static int
read_process_info(pid_t pid, struct kinfo_proc *process)
{
    int mib[4] = {CTL_KERN, KERN_PROC, KERN_PROC_PID, pid};
    size_t length = sizeof(*process);
    if (sysctl(mib, 4, process, &length, NULL, 0) != 0)
        return -1;
    return length == sizeof(*process) ? 0 : -1;
}

static int
read_line(int fd, char *buffer, size_t capacity)
{
    size_t used = 0;
    while (used + 1 < capacity) {
        char byte;
        if (read_all(fd, &byte, 1) != 0)
            return -1;
        buffer[used++] = byte;
        if (byte == '\n') {
            buffer[used] = '\0';
            return 0;
        }
    }
    errno = EMSGSIZE;
    return -1;
}

static int
renderer_host(const char *self)
{
    char root_template[] = "/tmp/aibe-renderer-jail-root.XXXXXX";
    char *root = mkdtemp(root_template);
    if (root == NULL) {
        perror("mkdtemp");
        return 1;
    }
    char run_dir[PATH_MAX], tmp_dir[PATH_MAX], socket_path[PATH_MAX];
    char binary_path[PATH_MAX], marker_path[PATH_MAX];
    char jail_name[96], hostname[96];
    (void)snprintf(run_dir, sizeof(run_dir), "%s/run", root);
    (void)snprintf(tmp_dir, sizeof(tmp_dir), "%s/tmp", root);
    (void)snprintf(socket_path, sizeof(socket_path), "%s/broker.sock", run_dir);
    (void)snprintf(binary_path, sizeof(binary_path), "%s/renderer-jail-poc", root);
    (void)snprintf(marker_path, sizeof(marker_path), "/tmp/aibe-host-only-marker-%ld", (long)getpid());
    (void)snprintf(jail_name, sizeof(jail_name), "aibe-renderer-jail-%ld", (long)getpid());
    (void)snprintf(hostname, sizeof(hostname), "%s", jail_name);

    int result = 1;
    int listener = -1, connection = -1;
    pid_t jail_launcher = -1;
    pid_t renderer_pid = -1;
    int rule_created = 0;
    char line[256];
    struct timeval timeout = {.tv_sec = 8, .tv_usec = 0};

    if (mkdir(run_dir, 0755) != 0 || mkdir(tmp_dir, 0700) != 0) {
        perror("mkdir rootfs");
        goto cleanup;
    }
    if (chmod(root, 0755) != 0 || chmod(run_dir, 0711) != 0) {
        perror("chmod rootfs");
        goto cleanup;
    }
    int source = open(self, O_RDONLY);
    int destination = open(binary_path, O_WRONLY | O_CREAT | O_EXCL, 0555);
    if (source < 0 || destination < 0) {
        perror("open probe binary");
        if (source >= 0) close(source);
        if (destination >= 0) close(destination);
        goto cleanup;
    }
    char copy_buffer[65536];
    ssize_t count;
    while ((count = read(source, copy_buffer, sizeof(copy_buffer))) > 0) {
        if (write_all(destination, copy_buffer, (size_t)count) != 0) {
            perror("copy probe binary");
            close(source); close(destination);
            goto cleanup;
        }
    }
    close(source);
    close(destination);
    if (count < 0) {
        perror("read probe binary");
        goto cleanup;
    }
    int marker = open(marker_path, O_WRONLY | O_CREAT | O_EXCL, 0600);
    if (marker < 0) {
        perror("create host marker");
        goto cleanup;
    }
    (void)write_all(marker, "host-only\n", 10);
    close(marker);

    listener = socket(AF_UNIX, SOCK_STREAM, 0);
    if (listener < 0) { perror("socket"); goto cleanup; }
    struct sockaddr_un address;
    memset(&address, 0, sizeof(address));
    address.sun_family = AF_UNIX;
    if (strlen(socket_path) >= sizeof(address.sun_path)) {
        fprintf(stderr, "socket path too long\n");
        goto cleanup;
    }
    strlcpy(address.sun_path, socket_path, sizeof(address.sun_path));
    if (bind(listener, (struct sockaddr *)&address, sizeof(address)) != 0 ||
        chmod(socket_path, 0600) != 0 || chown(socket_path, RENDERER_UID, RENDERER_GID) != 0 ||
        listen(listener, 4) != 0 || setsockopt(listener, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0 ||
        fcntl(listener, F_SETFD, FD_CLOEXEC) != 0) {
        perror("prepare broker UDS");
        goto cleanup;
    }

    /* Construct argv with explicit execv arguments; no shell interpolation. */
    char path_arg[PATH_MAX + 16], host_arg[160], name_arg[160];
    (void)snprintf(path_arg, sizeof(path_arg), "path=%s", root);
    (void)snprintf(host_arg, sizeof(host_arg), "host.hostname=%s", hostname);
    (void)snprintf(name_arg, sizeof(name_arg), "name=%s", jail_name);
    char *jail_argv[] = {"/usr/sbin/jail", "-q", "-c", "-u", "nobody", name_arg,
        path_arg, host_arg, "ip4=disable", "ip6=disable",
        "command=/renderer-jail-poc", "--child", "/run/broker.sock", marker_path, NULL};
    jail_launcher = fork();
    if (jail_launcher < 0) { perror("fork jail"); goto cleanup; }
    if (jail_launcher == 0) {
        execv(jail_argv[0], jail_argv);
        _exit(127);
    }

    fd_set ready_fds;
    FD_ZERO(&ready_fds);
    FD_SET(listener, &ready_fds);
    int ready = select(listener + 1, &ready_fds, NULL, NULL, &timeout);
    if (ready <= 0) {
        if (ready == 0) errno = ETIMEDOUT;
        perror("wait for Renderer UDS");
        goto cleanup;
    }
    connection = accept(listener, NULL, NULL);
    if (connection < 0) { perror("accept Renderer UDS"); goto cleanup; }
    if (setsockopt(connection, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) != 0) {
        perror("set Renderer UDS timeout");
        goto cleanup;
    }
    struct xucred credentials;
    socklen_t credential_length = sizeof(credentials);
    if (getsockopt(connection, SOL_LOCAL, LOCAL_PEERCRED, &credentials, &credential_length) != 0) {
        perror("LOCAL_PEERCRED"); goto cleanup;
    }
    if (credential_length != sizeof(credentials) || credentials.cr_version != XUCRED_VERSION ||
        credentials.cr_ngroups < 1 || credentials.cr_ngroups > XU_NGROUPS || credentials.cr_pid <= 0 ||
        credentials.cr_uid != RENDERER_UID || credentials.cr_gid != RENDERER_GID) {
        fprintf(stderr, "FAIL: unexpected kernel peer credentials pid=%d uid=%u gid=%u\n",
            credentials.cr_pid, credentials.cr_uid, credentials.cr_gid);
        goto cleanup;
    }
    renderer_pid = credentials.cr_pid;
    int jid = read_jail_id(jail_name);
    struct kinfo_proc process = {0};
    int process_info_result = read_process_info(credentials.cr_pid, &process);
    if (jid <= 0 || process_info_result != 0 || process.ki_pid != credentials.cr_pid || process.ki_jid != jid || process.ki_uid != RENDERER_UID) {
        fprintf(stderr, "FAIL: peer process is not in expected Jail (jid=%d process_jid=%d uid=%u)\n",
            jid, process.ki_jid, process.ki_uid);
        goto cleanup;
    }
    if (read_line(connection, line, sizeof(line)) != 0 ||
        strstr(line, "uid=65534") == NULL || strstr(line, "gid=65534") == NULL ||
        strstr(line, "marker_visible=0") == NULL) {
        fprintf(stderr, "FAIL: Renderer identity/filesystem probe: %s\n", line);
        goto cleanup;
    }
    printf("PASS: kernel peer PID=%d UID=%u GID=%u is in Jail JID=%d\n",
        credentials.cr_pid, credentials.cr_uid, credentials.cr_gid, jid);
    printf("PASS: isolated rootfs hides host-only marker (%s)\n", line);
    printf("PASS: no host listener or unrelated descriptor was inherited across exec\n");

    char rule[256];
    (void)snprintf(rule, sizeof(rule), "jail:%s:vmemoryuse:deny=128M/jail", jail_name);
    char *rctl_argv[] = {"/usr/bin/rctl", "-a", rule, NULL};
    if (run_command(rctl_argv) != 0) {
        fprintf(stderr, "FAIL: unable to apply jail-scoped vmemoryuse rule\n");
        goto cleanup;
    }
    rule_created = 1;
    (void)snprintf(rule, sizeof(rule), "jail:%s:maxproc:deny=2/jail", jail_name);
    if (run_command(rctl_argv) != 0) {
        fprintf(stderr, "FAIL: unable to apply jail-scoped maxproc rule\n");
        goto cleanup;
    }
    (void)snprintf(rule, sizeof(rule), "jail:%s", jail_name);
    char *usage_argv[] = {"/usr/bin/rctl", "-hu", rule, NULL};
    printf("=== live jail resource snapshot ===\n");
    (void)run_command(usage_argv);

    if (write_all(connection, "M", 1) != 0 || read_line(connection, line, sizeof(line)) != 0 ||
        strcmp(line, "MEMORY_DENIED\n") != 0) {
        fprintf(stderr, "FAIL: jail vmemoryuse rule did not deny 256 MiB mapping: %s\n", line);
        goto cleanup;
    }
    printf("PASS: jail-scoped vmemoryuse limit denied 256 MiB mapping (%s)", line);
    if (write_all(connection, "P", 1) != 0 || read_line(connection, line, sizeof(line)) != 0 ||
        strcmp(line, "PROC_DENIED\n") != 0) {
        fprintf(stderr, "FAIL: jail maxproc rule did not deny the second fork: %s\n", line);
        goto cleanup;
    }
    printf("PASS: jail-scoped maxproc limit denied the second fork (%s)", line);
    result = 0;

cleanup:
    if (jail_launcher > 0) {
        if (read_jail_id(jail_name) > 0) {
            char *remove_argv[] = {"/usr/sbin/jail", "-r", jail_name, NULL};
            (void)run_command(remove_argv);
        }
        int status = 0;
        (void)waitpid(jail_launcher, &status, 0);
        if (read_jail_id(jail_name) > 0) {
            fprintf(stderr, "FAIL: Jail remained after supervisor teardown\n");
            result = 1;
        } else if (renderer_pid > 0) {
            struct kinfo_proc after_teardown = {0};
            if (read_process_info(renderer_pid, &after_teardown) == 0) {
                fprintf(stderr, "FAIL: Renderer PID %d survived Jail teardown (jid=%d)\n",
                    renderer_pid, after_teardown.ki_jid);
                result = 1;
            } else if (result == 0) {
                printf("PASS: one-Renderer Jail was destroyed and Renderer PID %d exited\n",
                    renderer_pid);
            }
        }
    }
    if (connection >= 0) close(connection);
    if (rule_created) {
        char rule[160];
        (void)snprintf(rule, sizeof(rule), "jail:%s", jail_name);
        char *remove_rule_argv[] = {"/usr/bin/rctl", "-r", rule, NULL};
        (void)run_command(remove_rule_argv);
    }
    if (listener >= 0) close(listener);
    if (socket_path[0] != '\0') (void)unlink(socket_path);
    (void)unlink(marker_path);
    (void)unlink(binary_path);
    (void)rmdir(run_dir);
    (void)rmdir(tmp_dir);
    (void)rmdir(root);
    return result;
}

int
main(int argc, char **argv)
{
    if (argc >= 2 && strcmp(argv[1], "--child") == 0) {
        if (argc != 4)
            return 2;
        return renderer_child(argv[2], argv[3]);
    }
    if (geteuid() != 0) {
        fprintf(stderr, "This FreeBSD Jail proof must run as root.\n");
        return 77;
    }
    char resolved[PATH_MAX];
    if (realpath(argv[0], resolved) == NULL) {
        perror("realpath probe");
        return 1;
    }
    return renderer_host(resolved);
}
