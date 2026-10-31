#!/bin/sh
# Capture the native build environment without changing host configuration.
set -eu
printf '%s\n' '=== FreeBSD ==='
freebsd-version -kru
printf '%s\n' '=== Architecture ==='
uname -a
printf '%s\n' '=== Compiler ==='
cc --version | head -n 2
printf '%s\n' '=== Rust ==='
rustc -Vv
cargo -V
printf '%s\n' '=== Git ==='
git --version
printf '%s\n' '=== Disk ==='
df -h .
printf '%s\n' '=== Cargo workspace ==='
cargo metadata --no-deps --format-version 1
