#!/bin/sh
# Build and run the minimal V8 and SpiderMonkey C++ embedding probes.
set -eu
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
PREFIX=${AIBE_RUNTIME_PREFIX:-/tmp/aibe-runtime-root/usr/local}
OUT_DIR=${AIBE_NATIVE_EMBED_OUT:-/tmp/aibe-native-embed}
CXX=${CXX:-c++}
mkdir -p "$OUT_DIR"

"$CXX" -std=c++20 -Wall -Wextra -Werror \
  -DV8_COMPRESS_POINTERS \
  -DV8_31BIT_SMIS_ON_64BIT_ARCH \
  -DV8_ENABLE_SANDBOX \
  -isystem "$PREFIX/include" \
  "$REPO_DIR/experiments/js-runtime/native-embed/v8/main.cc" \
  -L"$PREFIX/lib" -Wl,-rpath,"$PREFIX/lib" \
  -lv8 -lv8_libplatform -lv8_libbase \
  -lthird_party_partition_alloc_src_partition_alloc_allocator_core \
  -lthird_party_partition_alloc_src_partition_alloc_allocator_shim \
  -lthird_party_partition_alloc_src_partition_alloc_allocator_base \
  -lthird_party_abseil-cpp_absl -lchrome_zlib \
  -licui18n -licuuc -licudata -pthread \
  -o "$OUT_DIR/aibe-v8-embed"

"$CXX" -std=c++20 -Wall -Wextra -Werror -DXP_UNIX -DXP_FREEBSD \
  -isystem "$PREFIX/include/mozjs-153" \
  "$REPO_DIR/experiments/js-runtime/native-embed/spidermonkey/main.cpp" \
  -L"$PREFIX/lib" -Wl,-rpath,"$PREFIX/lib" \
  -lmozjs-153 -lnspr4 -lplc4 -lplds4 -pthread \
  -o "$OUT_DIR/aibe-spidermonkey-embed"

export LD_LIBRARY_PATH="$PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
printf '%s\n' '=== V8 embedding ==='
"$OUT_DIR/aibe-v8-embed"
printf '%s\n' '=== SpiderMonkey embedding ==='
"$OUT_DIR/aibe-spidermonkey-embed"
