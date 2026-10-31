# Minimal Capsicum negative test

This FreeBSD-only program prepares an unlinked temporary file, forks a child, reduces the inherited descriptor to `CAP_READ` and `CAP_FSTAT`, and enters Capsicum capability mode.

Expected checks:

- Reading from the inherited descriptor remains allowed.
- Writing through the descriptor is denied with `ENOTCAPABLE`.
- Opening `/etc/passwd` by pathname is denied with `ECAPMODE` after `cap_enter()`.

This is still only a narrow primitive test. It does **not** test a complete renderer sandbox, network confinement, jail isolation, process credential separation, IPC peer authentication, cross-origin isolation, or resistance to a compromised browser process.

Build and run on FreeBSD:

```sh
clang -Wall -Wextra -Werror -O2 src/main.c -o /tmp/aibe-capsicum-minimal
/tmp/aibe-capsicum-minimal
```
