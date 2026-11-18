#!/usr/bin/env bash
# Sourced by the tuicr-wrapper-*.sh scripts. Not an entry point on its own.

# Splits "$@" on the `[directory] [-- tuicr-args...]` convention every
# wrapper shares. Sets TUICR_TARGET_DIR and the TUICR_PASSTHROUGH_ARGS array
# instead of returning them, since a sourced function's positional
# parameters are its own and can't shift the caller's.
# shellcheck disable=SC2034 # These variables are read by the sourcing wrappers.
tuicr_parse_args() {
  TUICR_TARGET_DIR="."
  if [[ "${1:-}" != "--" && -n "${1:-}" ]]; then
    TUICR_TARGET_DIR="$1"
    shift
  fi
  if [[ "${1:-}" == "--" ]]; then
    shift
  fi
  TUICR_PASSTHROUGH_ARGS=("$@")
}

# Prints each argument bash-%q-quoted with a leading space, e.g.
# `tuicr_quote_args -w` prints " -w", so the caller can append the result
# straight onto a command string: cmd="tuicr$(tuicr_quote_args "${args[@]}")"
tuicr_quote_args() {
  local arg quoted
  for arg in "$@"; do
    printf -v quoted ' %q' "$arg"
    printf '%s' "$quoted"
  done
}

# True if the installed tuicr accepts --stdout, which exports the review
# straight to a file instead of prompting the human to save & copy on exit.
tuicr_stdout_supported() {
  tuicr --help 2>&1 | grep -q -- '--stdout'
}

# Waits while "$@" — a check that succeeds while another tuicr is open —
# keeps succeeding, so a second launch queues behind the review on screen
# instead of exiting 0 with nothing exported, which an agent reads as an
# approval. Gives up after TUICR_WAIT_TIMEOUT seconds (default 300) with exit
# 75 (EX_TEMPFAIL): rerun once the other review is closed. Callers must define
# log_info, log_warn and log_error before invoking this.
tuicr_wait_for_running() {
  local timeout="${TUICR_WAIT_TIMEOUT:-300}"
  local waited=0
  "$@" || return 0
  log_warn "tuicr is already running in another pane; waiting up to ${timeout}s for it to close"
  while "$@"; do
    if (( waited >= timeout )); then
      log_error "tuicr is still running after ${timeout}s; rerun once that review is closed"
      exit 75
    fi
    sleep 1
    waited=$(( waited + 1 ))
  done
  log_info "The other tuicr closed; launching this review"
}

tuicr_report_saved_comments() {
  log_info "Read saved feedback with: tuicr review comments$(tuicr_quote_args --repo "$1" --session SLUG)"
}

# Print an explicit export if there was one. A plain :q leaves saved comments
# in the session, even though it does not export anything.
tuicr_report_stdout_output() {
  local output_file="$1"
  local target_dir="$2"
  if [[ -s "$output_file" ]]; then
    echo ""
    echo "=== TUICR INSTRUCTIONS ==="
    cat "$output_file"
    echo "=== END TUICR INSTRUCTIONS ==="
  else
    tuicr_report_saved_comments "$target_dir"
  fi
  if [[ -n "$output_file" ]]; then
    rm -f "$output_file"
  fi
}
