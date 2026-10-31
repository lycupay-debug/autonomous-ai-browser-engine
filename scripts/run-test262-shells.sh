#!/bin/sh
# Run the same limited Test262 subset in extracted FreeBSD V8 and SpiderMonkey shells.
set -eu
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TEST262_ROOT=${TEST262_ROOT:-/tmp/aibe-test262}
PREFIX=${AIBE_RUNTIME_PREFIX:-/tmp/aibe-runtime-root/usr/local}
REVISION=91c70759363c46557b694792bf34a2551a7af5ba
OUT_DIR=${AIBE_TEST262_OUT:-/tmp/aibe-test262-native}
mkdir -p "$OUT_DIR"

if [ ! -d "$TEST262_ROOT/.git" ]; then
  echo "Test262 checkout missing; run scripts/run-test262-subset.sh first." >&2
  exit 2
fi

git -C "$TEST262_ROOT" checkout --detach "$REVISION" >/dev/null 2>&1
cd "$REPO_DIR"
TEST262_REVISION="$REVISION" cargo run -q -p aibe-boa-host-poc --bin test262_subset --locked -- \
  --list "$TEST262_ROOT" \
  test/language/expressions/addition \
  test/language/expressions/bitwise-or \
  test/language/expressions/bitwise-and \
  test/language/expressions/multiplication \
  test/language/expressions/relational \
  test/language/statements/let > "$OUT_DIR/manifest.tsv"

export LD_LIBRARY_PATH="$PREFIX/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
run_engine() {
  engine_name=$1
  engine_path=$2
  passed=0
  failed=0
  failures="$OUT_DIR/${engine_name}-failures.txt"
  : > "$failures"

  while IFS="$(printf '\t')" read -r test_file expected_error; do
    test_source="$OUT_DIR/current-test.js"
    output="$OUT_DIR/current-output.txt"
    cat "$TEST262_ROOT/harness/sta.js" "$TEST262_ROOT/harness/assert.js" "$test_file" > "$test_source"

    set +e
    "$engine_path" "$test_source" > "$output" 2>&1
    status=$?
    set -e

    if [ -n "$expected_error" ]; then
      if [ "$status" -ne 0 ] && grep -Fq "$expected_error" "$output"; then
        passed=$((passed + 1))
      else
        failed=$((failed + 1))
        printf '%s\tnegative=%s\tstatus=%s\n' "$test_file" "$expected_error" "$status" >> "$failures"
      fi
    elif [ "$status" -eq 0 ]; then
      passed=$((passed + 1))
    else
      failed=$((failed + 1))
      printf '%s\tstatus=%s\n' "$test_file" "$status" >> "$failures"
    fi
  done < "$OUT_DIR/manifest.tsv"

  printf '%s: passed=%s failed=%s total=%s\n' "$engine_name" "$passed" "$failed" "$((passed + failed))"
  if [ "$failed" -gt 0 ]; then
    sed -n '1,20p' "$failures" >&2
    return 1
  fi
}

run_engine v8 "$PREFIX/bin/d8"
run_engine spidermonkey "$PREFIX/bin/js153"
