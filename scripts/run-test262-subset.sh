#!/bin/sh
# Fetch a pinned Test262 revision and run the supported synchronous subset.
set -eu
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
TEST262_ROOT=${TEST262_ROOT:-/tmp/aibe-test262}
TEST262_REVISION=91c70759363c46557b694792bf34a2551a7af5ba

if [ ! -d "$TEST262_ROOT/.git" ]; then
  git clone --depth 1 --filter=blob:none --sparse https://github.com/tc39/test262.git "$TEST262_ROOT"
fi

git -C "$TEST262_ROOT" fetch --depth 1 origin "$TEST262_REVISION"
git -C "$TEST262_ROOT" checkout --detach "$TEST262_REVISION"
git -C "$TEST262_ROOT" sparse-checkout set \
  harness \
  test/language/expressions/addition \
  test/language/expressions/bitwise-or \
  test/language/expressions/bitwise-and \
  test/language/expressions/multiplication \
  test/language/expressions/relational \
  test/language/statements/let

cd "$REPO_DIR"
TEST262_REVISION="$TEST262_REVISION" cargo run -p aibe-boa-host-poc --bin test262_subset --locked -- \
  "$TEST262_ROOT" \
  test/language/expressions/addition \
  test/language/expressions/bitwise-or \
  test/language/expressions/bitwise-and \
  test/language/expressions/multiplication \
  test/language/expressions/relational \
  test/language/statements/let
