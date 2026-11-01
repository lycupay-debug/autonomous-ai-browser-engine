#include <sys/types.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/capsicum.h>
#include <signal.h>
#include <sys/wait.h>
#include <fcntl.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int
main(int argc, char **argv)
{
    if (argc != 2)
        return 2;
    int random_fd = open("/dev/urandom", O_RDONLY);
    if (random_fd < 0)
        return 18;
    close(random_fd);
    errno = 0;
    int forbidden_device = open("/dev/bpf", O_RDONLY);
    if (forbidden_device >= 0 || errno != ENOENT)
        return 19;
    for (int inherited_fd = 3; inherited_fd < 1024; inherited_fd++) {
        if (fcntl(inherited_fd, F_GETFD) >= 0 || errno != EBADF)
            return 8;
    }
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0)
        return 3;
    struct sockaddr_un address;
    memset(&address, 0, sizeof(address));
    address.sun_family = AF_UNIX;
    if (strlen(argv[1]) >= sizeof(address.sun_path))
        return 4;
    strlcpy(address.sun_path, argv[1], sizeof(address.sun_path));
    if (connect(fd, (struct sockaddr *)&address, sizeof(address)) != 0)
        return 5;
    for (int inherited_fd = 3; inherited_fd < 1024; inherited_fd++) {
        if (fcntl(inherited_fd, F_GETFD) >= 0 && inherited_fd != fd)
            return 9;
        if (errno != EBADF && inherited_fd != fd)
            return 10;
    }
    if (cap_enter() != 0) { perror("cap_enter"); return 11; }
    unsigned int cap_mode = 0;
    if (cap_getmode(&cap_mode) != 0 || cap_mode != 1) { perror("cap_getmode"); return 12; }
    errno = 0;
    int forbidden_fd = open("/capability-probe", O_RDONLY);
    if (forbidden_fd >= 0 || errno != ECAPMODE) { dprintf(2, "open after cap_enter: fd=%d errno=%d expected=%d\n", forbidden_fd, errno, ENOTCAPABLE); return 13; }

    pid_t descendant = fork();
    if (descendant < 0)
        return 6;
    if (descendant == 0) {
        for (;;)
            pause();
    }
    errno = 0;
    pid_t denied_descendant = fork();
    if (denied_descendant >= 0) {
        if (denied_descendant == 0)
            _exit(0);
        (void)kill(denied_descendant, SIGKILL);
        (void)waitpid(denied_descendant, NULL, 0);
        return 20;
    }
    if (errno != EAGAIN)
        return 21;
    char message[64];
    int length = snprintf(message, sizeof(message), "CAPMODE %u DESCENDANT %ld\n", cap_mode, (long)descendant);
    if (length <= 0 || write(fd, message, (size_t)length) != length)
        return 7;

    char byte;
    while (read(fd, &byte, 1) > 0)
        ;
    close(fd);
    return 0;
}
