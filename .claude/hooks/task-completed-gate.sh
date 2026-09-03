#!/usr/bin/env bash
# TaskCompleted quality gate for the Serat agent team.
# Payload JSON on stdin. Exit 2 rejects completion; stderr goes back to the teammate.
payload=$(cat)
subject=$(printf '%s' "$payload" | grep -o '"subject":"[^"]*"' | head -1)
desc=$(printf '%s' "$payload" | grep -o '"description":"[^"]*"' | head -1)

# Rule 1: every task must cite at least one requirement/story/ADR ID.
if ! printf '%s%s' "$subject" "$desc" | grep -qE '(FR|NFR)-[A-Z]{3}-[0-9]{2}|US-[0-9]{2}|ADR-[0-9]{4}'; then
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

# Rule 3: test suites must pass where they exist.
if [ -f Cargo.toml ]; then
  cargo test --workspace --quiet >/dev/null 2>&1 || { echo "Task completion rejected: cargo test --workspace is failing." >&2; exit 2; }
  cargo clippy --workspace --quiet -- -D warnings >/dev/null 2>&1 || { echo "Task completion rejected: cargo clippy -D warnings is not clean." >&2; exit 2; }
fi
if [ -f app/package.json ] && grep -q '"test"' app/package.json; then
  (cd app && npm test --silent >/dev/null 2>&1) || { echo "Task completion rejected: npm test in app/ is failing." >&2; exit 2; }
fi
exit 0
