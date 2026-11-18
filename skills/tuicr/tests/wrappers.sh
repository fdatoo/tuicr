#!/usr/bin/env bash
set -eu

skill_dir=${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}
test_dir=$(mktemp -d)
test_dir=$(cd "$test_dir" && pwd -P)
trap 'rm -rf "$test_dir"' EXIT
cd "$test_dir"

export MOCK_DIR="$test_dir" MOCK_CHECKOUT="$test_dir/reviewer's checkout"
bin="$test_dir/mock's bin"
mkdir "$bin" "$MOCK_CHECKOUT" plain
ln -s "$bin" plain-bin
git init -q "$MOCK_CHECKOUT"
git init -q plain
export PATH="$bin:$PATH"

cat > "$bin/tuicr" <<'MOCK'
#!/bin/bash
if [[ "${1:-}" == --help ]]; then
  [[ "${MOCK_STDOUT:-yes}" != yes ]] || echo --stdout
  exit 0
fi
pwd -P > "$MOCK_DIR/actual-cwd"
printf '%s\0' "$@" > "$MOCK_DIR/actual-args"
if [[ "${MOCK_STATUS:-0}" != 0 ]]; then
  echo 'error: conflicting review flags' >&2
  exit "$MOCK_STATUS"
fi
if [[ "${MOCK_EXPORT:-no}" == yes ]]; then
  echo 'mock exported review'
fi
MOCK

cat > "$bin/multiplexer" <<'MOCK'
#!/bin/bash
set -eu
kind=${0##*/}
printf '%q ' "$kind" "$@" >> "$MOCK_DIR/multiplexer-argv"
printf '\n' >> "$MOCK_DIR/multiplexer-argv"
run() {
  (cd "$cwd" && exec "$@") </dev/null > "$MOCK_DIR/pane" 2>&1 &
}
parse_split() {
  cwd=.
  while [[ $# -gt 0 ]]; do
    case "$1" in
      -c|--cwd) cwd=$2; shift 2 ;;
      -F|-t|-l|-s|--split|--label|--name|--direction) shift 2 ;;
      -b) if [[ "$kind" == rex ]]; then shift 2; else shift; fi ;;
      --) shift; break ;;
      -*) shift ;;
      *) break ;;
    esac
  done
  if [[ "$kind" == tmux && $# == 1 ]]; then
    run "$PANE_SHELL" -c "$1"
  else
    run "$@"
  fi
}
case "$kind $1 ${2:-}" in
  'tmux display-message '*)
    case "${!#}" in
      '#{pane_height}') echo 30 ;;
      '#{pane_active}') echo 1 ;;
    esac ;;
  'tmux list-panes '*) echo bash ;;
  'tmux select-pane '*) : ;;
  'tmux split-window '*) shift; parse_split "$@"; echo '%2' ;;
  'tmux wait-for -S') touch "$MOCK_DIR/$3" ;;
  'tmux wait-for '*)
    for (( tries=0; tries<100; tries++ )); do
      [[ ! -e "$MOCK_DIR/$2" ]] || exit 0
      sleep 0.05
    done
    exit 1 ;;
  'herdr pane split')
    printf '%s\n' "$7" > "$MOCK_DIR/pane-cwd"
    echo '{"result":{"pane":{"pane_id":"pane:2"}}}' ;;
  'herdr pane run') cwd=$(cat "$MOCK_DIR/pane-cwd"); run "$PANE_SHELL" -c "$4" ;;
  'herdr pane wait-output')
    for (( tries=0; tries<100; tries++ )); do
      if line=$(grep -F -- "$5" "$MOCK_DIR/pane"); then
        jq -n --arg line "$line" '{result:{matched_line:$line}}'
        exit 0
      fi
      if grep -Eq 'unexpected EOF|unmatched' "$MOCK_DIR/pane"; then break; fi
      sleep 0.05
    done
    cat "$MOCK_DIR/pane" >&2
    exit 1 ;;
  'herdr pane close') : ;;
  'cmux new-pane '*) echo 'OK surface:2 pane:2 workspace:1' ;;
  'cmux send '*) "$PANE_SHELL" -c "$4" ;;
  'rex split '*) shift; parse_split "$@"; echo '{"block":"block:2"}' ;;
  'zellij action dump-layout') echo 'layout {}' ;;
  'zellij run '*) shift; parse_split "$@" ;;
  *) printf 'Unrecognized mux call: %s %s\n' "$kind" "$*" >&2; exit 1 ;;
esac
MOCK

cat > "$bin/pgrep" <<'MOCK'
#!/bin/bash
[[ -n "${MOCK_RUNNING_CWD:-}" ]] || exit 1
printf '%s\n' 999999999
MOCK
cat > "$bin/lsof" <<'MOCK'
#!/bin/bash
[[ "$*" == '-a -p 999999999 -d cwd -Fn' ]] || exit 1
printf 'p999999999\nfcwd\nn%s\n' "$MOCK_RUNNING_CWD"
MOCK
chmod +x "$bin/tuicr" "$bin/multiplexer" "$bin/pgrep" "$bin/lsof"
for mux in tmux herdr cmux rex zellij; do ln -s multiplexer "$bin/$mux"; done
export TMUX=mock TMUX_PANE=%1 HERDR_ENV=1 CMUX_WORKSPACE_ID=1 REX_SESSION=1 REX_BLOCK=block:1 ZELLIJ=0
export HERDR_BIN=herdr CMUX_BIN=cmux REX_BIN=rex ZELLIJ_BIN=zellij JQ_BIN=jq
export TUICR_WAIT_TIMEOUT=0 TUICR_PANE_DIRECTION=stacked TUICR_PANE_POSITION=stacked

failures=0
invoke() {
  local label=$1 expected=$2 mux=$3 checkout=$4 wrapper status=0
  shift 4
  rm -f pane actual-args actual-cwd multiplexer-argv
  wrapper="tuicr-wrapper-$mux.sh"
  [[ "$mux" != tmux ]] || wrapper=tuicr-wrapper.sh
  perl -e 'alarm shift; exec @ARGV or die $!' 8 /bin/bash "$skill_dir/$wrapper" "$checkout" -- "$@" > wrapper-output 2>&1 || status=$?
  printf '%s: exit %s (expected %s)\n' "$label" "$status" "$expected"
  if [[ "$status" != "$expected" ]]; then
    failures=$((failures + 1))
    cat wrapper-output
    [[ ! -e pane ]] || cat pane
  fi
}
check() {
  local label=$1
  shift
  if "$@"; then
    printf 'PASS: %s\n' "$label"
  else
    printf 'FAIL: %s\n' "$label"
    printf '  checkout seen: %s\n' "$(cat actual-cwd 2>/dev/null || :)"
    [[ ! -e actual-args ]] || od -c actual-args
    failures=$((failures + 1))
  fi
}

PANE_SHELL=/bin/bash MOCK_STATUS=2 invoke 'tmux conflicting flags' 2 tmux "$test_dir/plain" -w -p diff.patch
check 'tmux reports the child error' grep -F 'conflicting review flags' wrapper-output
check 'tmux does not report success on failure' test "$(grep -cF 'tuicr finished' wrapper-output)" -eq 0

for shell in /bin/bash "$(command -v zsh)"; do
  export PANE_SHELL="$shell"
  PATH="$test_dir/plain-bin:$PATH" invoke "Herdr with no extra args under $shell" 0 herdr "$test_dir/plain"
  check 'Herdr forwards no extra args' cmp -s actual-args <(printf '%s\0' --stdout)
  for mux in tmux herdr zellij cmux rex; do
    args=(-w --guide "guide's path.json" "literal \$HOME; *.rs" '' $'a\tb\nc')
    invoke "$mux under $shell with quoted paths and args" 0 "$mux" "$MOCK_CHECKOUT" "${args[@]}"
    if [[ "$mux" == rex ]]; then
      for (( tries=0; tries<100; tries++ )); do
        [[ ! -f actual-args ]] || break
        sleep 0.05
      done
    fi
    extra=()
    case "$mux" in tmux|herdr|zellij) extra=(--stdout) ;; esac
    check "$mux keeps checkout and argv intact" \
      cmp -s actual-cwd <(cd "$MOCK_CHECKOUT" && pwd -P)
    check "$mux keeps argument bytes intact" \
      cmp -s actual-args <(printf '%s\0' "${args[@]}" ${extra[@]+"${extra[@]}"})
    check "$mux points :q readers at comments" grep -F 'tuicr review comments' wrapper-output
    check "$mux does not ask for clipboard output" test "$(grep -ci 'clipboard' wrapper-output)" -eq 0
  done
done

MOCK_STDOUT=no invoke 'tmux without --stdout' 0 tmux "$test_dir/plain"
check 'tmux points at saved feedback without --stdout' grep -F 'tuicr review comments' wrapper-output
MOCK_EXPORT=yes invoke 'tmux exported review' 0 tmux "$test_dir/plain"
check 'tmux retains review export' grep -Fx 'mock exported review' wrapper-output
PATH="$test_dir/plain-bin:$PATH" invoke 'Zellij requested checkout' 0 zellij "$MOCK_CHECKOUT" -w
check 'Zellij works in the requested checkout' cmp -s actual-cwd <(cd "$MOCK_CHECKOUT" && pwd -P)
MOCK_RUNNING_CWD="$MOCK_CHECKOUT/subdir" invoke 'Zellij busy in this checkout' 75 zellij "$MOCK_CHECKOUT" -w
check 'Zellij did not launch a pane while busy' test ! -e multiplexer-argv
MOCK_RUNNING_CWD="${MOCK_CHECKOUT}-another" invoke 'Zellij busy elsewhere' 0 zellij "$MOCK_CHECKOUT" -w
check 'Rex table row documents both variables' grep -E "^\\| .*\\\$REX_SESSION.*\\\$REX_BLOCK" "$skill_dir/SKILL.md"

if (( failures )); then
  printf '%s wrapper checks failed\n' "$failures"
  exit 1
fi
printf 'All wrapper checks passed\n'
