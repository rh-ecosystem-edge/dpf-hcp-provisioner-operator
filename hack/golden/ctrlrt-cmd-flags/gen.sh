#!/bin/bash
# Regenerates crates/ctrlrt-cmd/tests/fixtures/flags/* from the real Go manager binary
# (cmd/main.go): usage output, flag errors and exit codes, with os.Args[0] = "manager".
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
F=$ROOT/crates/ctrlrt-cmd/tests/fixtures/flags
BIN=$(mktemp -d)/manager
(cd "$ROOT" && go build -mod=vendor -o "$BIN" ./cmd)
mkdir -p "$F"
run() {
  local name=$1; shift
  printf '%s\n' "$@" > "$F/$name.args"
  set +e
  bash -c 'exec -a manager "$0" "$@"' "$BIN" "$@" > "$F/$name.stdout" 2> "$F/$name.stderr"
  echo $? > "$F/$name.exit"
  set -e
}
run help -h
run help2 --help
run undefined --bogus
run bad_level -zap-log-level=foo
run bad_level_zero --zap-log-level 0
run bad_bool -leader-elect=maybe
run needs_arg -metrics-cert-path
run bad_encoder --zap-encoder=xml
run bad_stack --zap-stacktrace-level=debug
run bad_time --zap-time-encoding=nano
run bad_syntax ---x
run bad_syntax2 -=x
