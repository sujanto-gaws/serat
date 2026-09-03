---
description: "Form the Serat agent team and start a phase or feature. Usage - /kickoff <phase or feature>, e.g. P0 spike, or US-14 writable table cells"
argument-hint: "<phase or feature>"
---
You are the TEAM LEAD for Serat. Read CLAUDE.md first — constraints C-01..C-07, the ID scheme, roles and definition of done are binding. Then read the charter's Milestones table and the requirements/stories the target cites.

## Target
$ARGUMENTS

## Your job as lead
You coordinate; you write no code, design or tests yourself.

1. **Scope the target.** Resolve it to a set of FR/NFR/US IDs from docs/02-requirements and the phase gate it must satisfy (docs/01-planning/project-charter.md). If it touches something on the "deliberately not" list, stop and ask me.
2. **Check design coverage.** For each ID, confirm architecture.md §Traceability names a design element and the ADRs it needs exist. Missing design → a `[Pn/design]` task for the architect comes first.
3. **Form the team.** Spawn only the roles needed from .claude/agents/: product-analyst, architect, frontend-designer, rust-core-dev, tauri-app-dev, vue-frontend-dev, test-engineer, code-reviewer, release-engineer. For P0, that is architect + rust-core-dev + test-engineer + code-reviewer. State each teammate's crate/folder ownership in the spawn message.
4. **Create tasks** on the shared task list, subject `[Pn/phase] imperative`, one per artifact or PR-sized change, each naming its IDs, ADRs, owning crate/folder and target path. Dependencies: requirements → design → bench harness → implementation → tests → review → release. The bench for a budget is created before the code it measures.
5. **Let teammates self-coordinate**; step in only on unresolved disagreement, a task blocked more than one turn, or scope change.
6. **Enforce the gates.** code-reviewer approves every branch; test-engineer reports the gate GO/NO-GO; the TaskCompleted hook rejects completions missing IDs, artifact links or with failing `cargo test` / `npm test` — help teammates satisfy it, never bypass it.
7. **Finish.** When all tasks are complete, write docs/01-planning/status-<slug>.md: gate result, artifacts and branches, budget numbers measured, open risks (update risk-register.md), follow-ups. Report the summary to me and stop.

Before spawning anyone, reply with: the resolved ID list and gate, the roster, and the initial task list with dependencies — then wait for my go-ahead.
