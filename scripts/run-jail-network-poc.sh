#!/bin/sh
# Prove that ip4=disable blocks a jail from reaching a host-local IPv4 listener.
set -eu
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PORT=$((20000 + ($$ % 30000)))
JAIL_NAME="aibe-phase0-net-$$"
JAIL_ROOT=$(mktemp -d /tmp/aibe-jail-network.XXXXXX)
PROBE=/tmp/aibe-jail-network-probe-$$
LISTENER_PID=

cleanup() {
  if [ -n "$LISTENER_PID" ]; then
    kill "$LISTENER_PID" 2>/dev/null || true
    wait "$LISTENER_PID" 2>/dev/null || true
  fi
  jail -r "$JAIL_NAME" >/dev/null 2>&1 || true
  rm -rf "$JAIL_ROOT"
  rm -f "$PROBE"
}
trap cleanup EXIT INT TERM

clang -Wall -Wextra -Werror -O2 -static \
  "$REPO_DIR/experiments/security/jail-network-probe.c" -o "$PROBE"

start_listener() {
  python3 -c 'import socket,sys; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1); s.bind(("127.0.0.1",int(sys.argv[1]))); s.listen(1); c,_=s.accept(); c.close(); s.close()' "$PORT" &
  LISTENER_PID=$!
  sleep 1
}

printf '%s\n' '=== Host baseline: same TCP endpoint must be reachable ==='
start_listener
"$PROBE" allow 127.0.0.1 "$PORT"
wait "$LISTENER_PID"
LISTENER_PID=

printf '%s\n' '=== Jail: ip4=disable must block the same endpoint ==='
mkdir -p "$JAIL_ROOT"
cp "$PROBE" "$JAIL_ROOT/netprobe"
start_listener
jail -c name="$JAIL_NAME" path="$JAIL_ROOT" host.hostname=aibe-phase0-network ip4=disable ip6=disable command=/netprobe deny 127.0.0.1 "$PORT"
kill "$LISTENER_PID" 2>/dev/null || true
wait "$LISTENER_PID" 2>/dev/null || true
LISTENER_PID=
printf '%s\n' 'PASS: jail IPv4 network isolation negative test'
