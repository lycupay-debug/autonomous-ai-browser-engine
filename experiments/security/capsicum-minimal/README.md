# Minimal Capsicum negative test

This small FreeBSD-only program forks a child, enters Capsicum capability mode, and verifies that opening `/etc/passwd` by pathname is denied.

It does **not** test descriptor-rights minimization, IPC peer authentication, network confinement, jail isolation, renderer compromise, cross-origin isolation, or a complete browser sandbox. Passing this test is one narrow piece of evidence only.

Build and run on FreeBSD:

```sh
clang -Wall -Wextra -Werror -O2 src/main.c -o /tmp/aibe-capsicum-minimal
/tmp/aibe-capsicum-minimal
```
