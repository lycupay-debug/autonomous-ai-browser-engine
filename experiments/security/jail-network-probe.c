/* Verify whether a FreeBSD jail with ip4=disable can reach a host-local TCP listener. */
#include <sys/socket.h>
#include <arpa/inet.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

int main(int argc, char **argv)
{
    if (argc != 4) {
        (void)fprintf(stderr, "usage: jail-network-probe <allow|deny> <IPv4> <port>\n");
        return EXIT_FAILURE;
    }
    int expect_denied = strcmp(argv[1], "deny") == 0;
    struct sockaddr_in destination;
    memset(&destination, 0, sizeof(destination));
    destination.sin_len = sizeof(destination);
    destination.sin_family = AF_INET;
    destination.sin_port = htons((uint16_t)strtoul(argv[3], NULL, 10));
    if (inet_pton(AF_INET, argv[2], &destination.sin_addr) != 1) {
        (void)fprintf(stderr, "invalid IPv4 address\n");
        return EXIT_FAILURE;
    }

    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        if (expect_denied && (errno == EAFNOSUPPORT || errno == EPROTONOSUPPORT || errno == ECAPMODE || errno == ENOTCAPABLE)) {
            (void)printf("PASS: IPv4 socket denied, errno=%d\n", errno);
            return EXIT_SUCCESS;
        }
        perror("socket");
        return EXIT_FAILURE;
    }

    int result = connect(fd, (struct sockaddr *)&destination, sizeof(destination));
    int connect_errno = errno;
    (void)close(fd);

    if (expect_denied) {
        if (result < 0 && (connect_errno == EADDRNOTAVAIL || connect_errno == ENETUNREACH ||
                           connect_errno == EAFNOSUPPORT || connect_errno == EPROTONOSUPPORT || connect_errno == ECAPMODE ||
                           connect_errno == ENOTCAPABLE)) {
            (void)printf("PASS: jail network connect denied, errno=%d\n", connect_errno);
            return EXIT_SUCCESS;
        }
        (void)fprintf(stderr, "FAIL: jail connect result=%d errno=%d\n", result, connect_errno);
        return EXIT_FAILURE;
    }

    if (result == 0) {
        (void)puts("PASS: host baseline TCP connection succeeded");
        return EXIT_SUCCESS;
    }
    (void)fprintf(stderr, "FAIL: host baseline connect errno=%d\n", connect_errno);
    return EXIT_FAILURE;
}
