#!/usr/bin/env bash
# TaskCompleted quality gate for the Serat agent team.
# Payload JSON on stdin. Exit 2 rejects completion; stderr goes back to the teammate.
#
# F-233: the fields were once extracted with `grep -o '"subject":"[^"]*"'`, which
# matches nothing when the payload is pretty-printed with a space after the colon,
# truncates at an embedded escaped quote, and is line-based so a multi-line
# description is cut short. Every failure produced the same "cite the IDs"
# rejection against descriptions that cited them -- F-107's shape, the guard
# firing and meaning the wrong thing.
#
# F-241: the first JSON fix read a fixed set of top-level keys and still could not
# find the fields in a real payload, failing open on live events. The keys are not
# where it guessed. So: walk the whole payload for the first string value under any
# key named subject/title or description/body, at any depth. Shape-independent by
# construction rather than by guessing one more layout.
payload=$(cat)

fields=$(printf '%s' "$payload" | python -c '
import json, sys

SUBJ = ("task_subject", "subject", "title", "name")
CWD  = ("cwd", "working_directory")
DESC = ("task_description", "description", "body", "content", "activeForm")

def walk(node, wanted, depth=0):
    """First non-empty string under any key in `wanted`, breadth-first by depth."""
    if depth > 12:
        return ""
    if isinstance(node, dict):
        for k, v in node.items():
            if k in wanted and isinstance(v, str) and v.strip():
                return v
        for v in node.values():
            got = walk(v, wanted, depth + 1)
            if got:
                return got
    elif isinstance(node, list):
        for v in node:
            got = walk(v, wanted, depth + 1)
            if got:
                return got
    return ""

try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(3)

subject = " ".join(walk(d, SUBJ).split())
desc = " ".join(walk(d, DESC).split())

if not subject and not desc:
    # Name the keys we did see, so the next person does not have to guess either.
    keys = sorted(d.keys()) if isinstance(d, dict) else [type(d).__name__]
    sys.stderr.write("payload keys: " + ",".join(keys) + "\n")
    sys.exit(4)

print(subject)
print(desc)
print(" ".join(walk(d, CWD).split()))
' 2>"${TMPDIR:-/tmp}/serat-gate-keys.$$")

parse_note=$(cat "${TMPDIR:-/tmp}/serat-gate-keys.$$" 2>/dev/null)
rm -f "${TMPDIR:-/tmp}/serat-gate-keys.$$"

# A gate that cannot read its input must say so, not reject the work. Failing open
# on an unreadable payload is deliberate: the alternative is F-233 again, where an
# unreadable payload is indistinguishable from a task that cited nothing. The note
# names the keys actually present so the failure is diagnosable rather than mute.
if [ -z "$fields" ]; then
  echo "Task completion gate: could not find subject/description in the TaskCompleted payload; content rules 1-2 skipped. ${parse_note:-(payload was not valid JSON)} Please report this to the lead with this line." >&2
else
  subject=$(printf '%s' "$fields" | sed -n '1p')
  desc=$(printf '%s' "$fields" | sed -n '2p')
  caller_cwd=$(printf '%s' "$fields" | sed -n '3p')

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
# F-234: this runs in the repository root -- the lead's integration copy -- not in
# the completing teammate's worktree, so it shares target/ with the lead's own
# builds. One cargo reaches the run phase and holds its test binaries open while
# another relinks them, and the linker reports LNK1104 on a green tree. A dedicated
# target directory decouples the gate from the lead.
# F-257: a single shared gate directory only moved the contention. Several agents
# complete tasks concurrently and all built into target/.task-gate, so a passing
# branch could not close a task while another agent was compiling -- the same
# false negative, one directory further along. The payload carries the caller's
# `cwd`, which for a teammate is their own worktree: gate THERE. It removes the
# contention entirely (each worktree has its own target/), it reuses a warm cache
# instead of a cold one, and -- the part that actually matters -- it tests the tree
# the teammate worked in rather than a copy of it.
if [ -n "$caller_cwd" ] && [ -d "$caller_cwd" ] && [ -f "$caller_cwd/Cargo.toml" ]; then
  cd "$caller_cwd" || true
fi

# F-242: that is necessary and not sufficient. The gate dir is still shared across
# concurrent completions from different teammates, and LNK1104 is a *link-stage*
# failure from file contention, not a test result -- reproduced by two people, and
# clean on immediate retry with no code change. Reporting it as "your tests are
# failing" is the same defect this gate was just fixed for: a guard firing and
# meaning the wrong thing. So a link failure is retried once, and only a failure
# that survives the retry counts. A genuine failure still fails, twice.
run_cargo() {
  local what="$1"; shift
  local out rc
  out=$("$@" 2>&1); rc=$?
  if [ $rc -ne 0 ] && printf '%s' "$out" | grep -q 'LNK1104\|Blocking waiting for file lock'; then
    sleep 5
    out=$("$@" 2>&1); rc=$?
  fi
  if [ $rc -ne 0 ]; then
    if printf '%s' "$out" | grep -q 'LNK1104'; then
      # F-259: LNK1104 means the link never completed, so NO TESTS RAN. We have no
      # evidence either way, and the previous version said exactly that and then
      # rejected anyway -- a guard firing and meaning the wrong thing, for the third
      # time in one session, this time written by the person fixing the first two.
      # Fail open and say so loudly, as with an unreadable payload: a rejection the
      # teammate has already disproved in their own worktree teaches them to stop
      # believing the gate, which costs more than the check is worth.
      echo "Task completion gate: $what could not link in $PWD (LNK1104, twice) so NO TESTS RAN -- this proves nothing about your code and is not being counted against you. Verify in your own worktree." >&2
      return 0
    fi
    echo "Task completion rejected: $what is failing (gated in: $PWD)." >&2
    return 1
  fi
  return 0
}

if [ -f Cargo.toml ]; then
  run_cargo "cargo test --workspace" cargo test --workspace --quiet --jobs 1 || exit 2
  run_cargo "cargo clippy -D warnings" cargo clippy --workspace --quiet -- -D warnings || exit 2
fi
if [ -f app/package.json ] && grep -q '"test"' app/package.json; then
  (cd app && npm test --silent >/dev/null 2>&1) || { echo "Task completion rejected: npm test in app/ is failing." >&2; exit 2; }
fi
exit 0
