#!/usr/bin/env bash
# TaskCompleted quality gate for the Serat agent team.
# Payload JSON on stdin. Exit 2 rejects completion; stderr goes back to the teammate.
#
# F-233: the fields were previously extracted with `grep -o '"subject":"[^"]*"'`.
# That pattern fails three ways and every failure looks identical to the teammate:
# it matches nothing when the payload is pretty-printed with a space after the
# colon (which is how it is actually serialised), it truncates at the first
# embedded \" , and grep is line-based so a description containing a newline is
# cut short. Any of those leaves both variables empty, so rule 1 rejects with
# "cite the FR-/NFR-/US-/ADR- IDs" against a description citing eleven of them.
# That is F-107's shape -- the guard fires and means the wrong thing -- with the
# extra cost that it teaches the wrong lesson: the teammate edits the task until
# they conclude the gate is broken, which is the right conclusion by the wrong
# route. Reproduced independently by architect and rust-core-dev.
#
# Parse the payload as JSON rather than pattern-matching it.
payload=$(cat)

fields=$(printf '%s' "$payload" | python -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(3)
def get(*names):
    for n in names:
        v = d.get(n)
        if isinstance(v, str) and v:
            return v
    for container in ("task", "payload"):
        c = d.get(container)
        if isinstance(c, dict):
            for n in names:
                v = c.get(n)
                if isinstance(v, str) and v:
                    return v
    return ""
# Newlines are collapsed so the downstream greps stay line-oriented and cannot
# truncate a multi-line description the way the old pattern silently did.
print(" ".join(get("subject", "title").split()))
print(" ".join(get("description", "body", "activeForm").split()))
' 2>/dev/null)

# A gate that cannot read its input must say so, not reject the work. Failing
# open on a parse error is deliberate: the alternative is F-233 again, where an
# unreadable payload is indistinguishable from a task that cited nothing.
if [ -z "$fields" ]; then
  echo "Task completion gate: could not parse the TaskCompleted payload as JSON; content rules 1-2 skipped. Please report this." >&2
else
  subject=$(printf '%s' "$fields" | sed -n '1p')
  desc=$(printf '%s' "$fields" | sed -n '2p')

  # Rule 1: every task must cite at least one requirement/story/ADR ID.
  if ! printf '%s %s' "$subject" "$desc" | grep -qE '(FR|NFR)-[A-Z]{3}-[0-9]{2}|US-[0-9]{2}|ADR-[0-9]{4}'; then
    echo "Task completion rejected: cite the FR-/NFR-/US-/ADR- IDs this task serves in the description (CLAUDE.md working agreement #2)." >&2
    exit 2
  fi

  # Rule 2: implementation/design/test/release tasks must link an artifact path or branch.
  if printf '%s' "$subject" | grep -qiE '\[P[0-9]/(impl|design|test|release|req)'; then
    if ! printf '%s' "$desc" | grep -qiE 'docs/|crates/|app/|bench/|schema/|grammar/|contracts/|task/|branch'; then
      echo "Task completion rejected: add the artifact path or branch (task/<id>-<slug>) to the description." >&2
      exit 2
    fi
  fi
fi

# Rule 3: test suites must pass where they exist.
#
# F-234: this runs in the repository root -- the lead's integration copy -- and
# not in the completing teammate's worktree. Sharing the root's target/ with the
# lead's own builds produces LNK1104: one cargo reaches the run phase and holds
# its test binaries open while another tries to relink them, so a green tree
# reports a link failure and the teammate is told their tests fail when they do
# not. A dedicated target directory decouples the gate from every other build in
# the repository. It costs one extra build cache and buys a result that means
# what it says.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target/.task-gate}"

if [ -f Cargo.toml ]; then
  if ! cargo test --workspace --quiet >/dev/null 2>&1; then
    echo "Task completion rejected: cargo test --workspace is failing (gate target dir: $CARGO_TARGET_DIR)." >&2
    exit 2
  fi
  if ! cargo clippy --workspace --quiet -- -D warnings >/dev/null 2>&1; then
    echo "Task completion rejected: cargo clippy -D warnings is not clean (gate target dir: $CARGO_TARGET_DIR)." >&2
    exit 2
  fi
fi
if [ -f app/package.json ] && grep -q '"test"' app/package.json; then
  (cd app && npm test --silent >/dev/null 2>&1) || { echo "Task completion rejected: npm test in app/ is failing." >&2; exit 2; }
fi
exit 0
