#!/bin/sh
# Assemble a root-owned FreeBSD Renderer rootfs from a built executable and its resolved shared libraries.
set -eu

if [ "$(uname -s)" != "FreeBSD" ]; then
  echo "ERROR: Renderer rootfs assembly requires FreeBSD" >&2
  exit 77
fi
if [ "$(id -u)" -ne 0 ]; then
  echo "ERROR: run Renderer rootfs assembly as root" >&2
  exit 77
fi
if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: $0 <renderer-binary> <new-rootfs-directory> [runtime-library-directory]" >&2
  exit 64
fi

RENDERER=$(realpath "$1")
ROOTFS=$2
case "$ROOTFS" in /*) ;; *) echo "ERROR: rootfs path must be absolute" >&2; exit 64 ;; esac
ROOTFS_PARENT=$(realpath "$(dirname "$ROOTFS")")
ROOTFS="$ROOTFS_PARENT/$(basename "$ROOTFS")"
RUNTIME_LIBRARY_DIR=${3:-"${AIBE_RUNTIME_PREFIX:-/tmp/aibe-runtime-root/usr/local}/lib"}
RUNTIME_LIBRARY_DIR=$(realpath "$RUNTIME_LIBRARY_DIR")

if [ ! -f "$RENDERER" ] || [ ! -x "$RENDERER" ]; then
  echo "ERROR: Renderer executable is missing or not executable: $RENDERER" >&2
  exit 66
fi
if [ ! -d "$RUNTIME_LIBRARY_DIR" ]; then
  echo "ERROR: runtime library directory does not exist: $RUNTIME_LIBRARY_DIR" >&2
  exit 66
fi
if [ -e "$ROOTFS" ] || [ "$ROOTFS" = "/" ]; then
  echo "ERROR: refusing to reuse an existing or unsafe rootfs path: $ROOTFS" >&2
  exit 73
fi
parent_owner=$(stat -f '%u' "$ROOTFS_PARENT")
parent_mode=$(stat -f '%p' "$ROOTFS_PARENT")
parent_permissions=$((0$parent_mode))
if [ "$parent_owner" -ne 0 ] || { [ $((parent_permissions & 18)) -ne 0 ] && [ $((parent_permissions & 512)) -eq 0 ]; }; then
  echo "ERROR: rootfs parent must be root-owned and not untrusted-writable unless sticky" >&2
  exit 77
fi

DEPENDENCIES=""
completed=0
cleanup() {
  if [ -n "$DEPENDENCIES" ]; then rm -f "$DEPENDENCIES"; fi
  if [ "$completed" -ne 1 ] && [ -d "$ROOTFS" ]; then
    find "$ROOTFS" -type f -flags schg -exec chflags noschg,nouchg {} \; 2>/dev/null || true
    rm -rf "$ROOTFS"
  fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

mkdir -p "$ROOTFS"
chmod 0755 "$ROOTFS"
for directory in lib libexec usr usr/lib usr/local usr/local/lib etc dev proc run tmp; do
  mkdir -p "$ROOTFS/$directory"
  chmod 0755 "$ROOTFS/$directory"
done
chmod 0711 "$ROOTFS/run"
chmod 1777 "$ROOTFS/tmp"

LDD_OUTPUT=$(LD_LIBRARY_PATH="$RUNTIME_LIBRARY_DIR" ldd "$RENDERER")
if printf '%s\n' "$LDD_OUTPUT" | grep -q 'not found'; then
  echo "ERROR: unresolved Renderer shared-library dependency" >&2
  printf '%s\n' "$LDD_OUTPUT" >&2
  exit 69
fi

DEPENDENCIES=$(mktemp /tmp/aibe-renderer-deps.XXXXXX)
printf '%s\n' "$LDD_OUTPUT" | awk '
  /=>[[:space:]]*\// { for (i = 1; i <= NF; i++) if ($i == "=>" && $(i + 1) ~ /^\//) print $(i + 1) }
' | sort -u > "$DEPENDENCIES"
printf '%s\n' /libexec/ld-elf.so.1 >> "$DEPENDENCIES"

while IFS= read -r source; do
  [ -n "$source" ] || continue
  [ -f "$source" ] || { echo "ERROR: dependency file missing: $source" >&2; exit 66; }
  case "$source" in
    "$RUNTIME_LIBRARY_DIR"/*)
      relative="usr/local/lib/${source##*/}"
      ;;
    /lib/*|/usr/lib/*|/libexec/*)
      relative=${source#/}
      ;;
    *)
      echo "ERROR: dependency is outside approved FreeBSD base/runtime library paths: $source" >&2
      exit 69
      ;;
  esac
  resolved=$(realpath "$source")
  owner=$(stat -f '%u' "$resolved")
  permissions=$(stat -f '%Lp' "$resolved")
  numeric_permissions=$((0$permissions))
  if [ "$owner" -ne 0 ] || [ $((numeric_permissions & 18)) -ne 0 ]; then
    echo "ERROR: shared library must be root-owned and not group/world writable: $resolved" >&2
    exit 77
  fi
  destination="$ROOTFS/$relative"
  mkdir -p "$(dirname "$destination")"
  if [ -e "$destination" ]; then
    if ! cmp -s "$resolved" "$destination"; then
      echo "ERROR: conflicting dependency basename: $destination" >&2
      exit 73
    fi
  else
    source_flags=$(stat -f '%Sf' "$resolved")
    if [ "$source_flags" = "-" ] && ln "$resolved" "$destination" 2>/dev/null; then
      :
    else
      cp -p "$resolved" "$destination"
      # Base-system files may carry schg; copies inside the temporary rootfs
      # must be removable at teardown without changing the host inode flags.
      chflags noschg,nouchg "$destination"
    fi
  fi
done < "$DEPENDENCIES"

cp -p "$RENDERER" "$ROOTFS/renderer"
chflags noschg,nouchg "$ROOTFS/renderer"
chmod 0555 "$ROOTFS/renderer"
{
  echo "renderer=$(basename "$RENDERER")"
  echo "renderer_sha256=$(sha256 -q "$RENDERER")"
  echo "runtime_library_dir=/usr/local/lib"
  echo "network_policy=ip4-disabled,ip6-disabled"
  echo "devfs=dedicated-restricted-ruleset-mounted-by-supervisor"
  echo "rootfs_policy=root-owned,non-writable-runtime,private-tmp,private-run"
} > "$ROOTFS/etc/aibe-runtime-manifest.txt"
chmod 0444 "$ROOTFS/etc/aibe-runtime-manifest.txt"

printf 'Renderer rootfs assembled: %s\n' "$ROOTFS"
printf 'Resolved dependencies: %s\n' "$(wc -l < "$DEPENDENCIES" | tr -d ' ')"
printf 'Renderer SHA-256: %s\n' "$(sha256 -q "$ROOTFS/renderer")"
completed=1
