#!/usr/bin/env bash
# Move a task card between tasks/todo, tasks/todo-human and tasks/done with `git mv`, and set its status line.
#   bash scripts/task-move.sh 014 done
#   bash scripts/task-move.sh 017b todo-human
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <NNN> <todo|todo-human|done>" >&2
  exit 2
fi
id="$1"
to="$2"
case "$to" in
  todo | todo-human | done) ;;
  *)
    echo "target must be todo, todo-human or done, not '$to'" >&2
    exit 2
    ;;
esac

shopt -s nullglob
cards=(tasks/todo/"$id"-*.md tasks/todo-human/"$id"-*.md tasks/done/"$id"-*.md)
if [ "${#cards[@]}" -ne 1 ]; then
  echo "expected exactly one card numbered $id under tasks/, found ${#cards[@]}: ${cards[*]-}" >&2
  exit 1
fi
card="${cards[0]}"
target="tasks/$to/$(basename "$card")"

if [ "$card" != "$target" ]; then
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1 && git ls-files --error-unmatch "$card" >/dev/null 2>&1; then
    git mv "$card" "$target"
  else
    mv "$card" "$target"
  fi
fi

# The status token: the first `- status:` line.
if grep -q '^- status:' "$target"; then
  tmp="$(mktemp "${target}.XXXXXX")"
  awk -v s="$to" '!done && /^- status:/ { print "- status: " s; done = 1; next } { print }' "$target" >"$tmp"
  mv "$tmp" "$target"
else
  echo "warning: $target has no '- status:' line" >&2
fi
echo "$card -> $target (status: $to)"
