#!/usr/bin/env bash
# The gate: run before closing any card. Small on purpose; every check says what it catches.
# A check that cannot run is skipped loudly. Ends with ALL GREEN or RED and exits non-zero on RED.
# Slow suites (database, end-to-end) live outside the gate, but are required when you touch their area (see CLAUDE.md).
set -u
cd "$(dirname "$0")/.."

failed=()
skipped=()

# run "<what this catches>" <command...>
run() {
  local what="$1"
  shift
  printf '\n==> %s\n    $ %s\n' "$what" "$*"
  if "$@"; then
    printf '    ok\n'
  else
    printf '    FAILED (exit %s): %s\n' "$?" "$what"
    failed+=("$what")
  fi
}

# skip "<what is not checked>" "<why>"
skip() {
  printf '\n==> SKIPPED: %s\n    why: %s\n' "$1" "$2"
  skipped+=("$1")
}

# has <binary>: true when the tool is installed.
has() { command -v "$1" >/dev/null 2>&1; }

# Unfilled template placeholders: the process files still have parts nobody wrote.
unfilled() {
  local marker="MAP"":FILL"
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    ! git grep -nI --untracked -e "$marker"
  else
    ! grep -rIn --exclude-dir=.git --exclude-dir=node_modules --exclude-dir=.scratch --exclude-dir=target -e "$marker" .
  fi
}
run "no unfilled template placeholders" unfilled

# MAP:FILL stack-checks: the project's real checks, each as `run "<what it catches>" <command>`, e.g. formatting, lint, type check, unit tests, build. Guard each tool with `has <tool> || skip "<check>" "<tool> not installed"`. Keep the whole gate to a few minutes.

printf '\n'
for s in "${skipped[@]+"${skipped[@]}"}"; do printf 'skipped: %s\n' "$s"; done
if [ "${#failed[@]}" -gt 0 ]; then
  for f in "${failed[@]}"; do printf 'failed:  %s\n' "$f"; done
  printf 'RED\n'
  exit 1
fi
printf 'ALL GREEN\n'
