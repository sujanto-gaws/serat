# Serat — sample project documents

Serat is a local-first, Obsidian-alternative knowledge app (Tauri v2 + Vue 3, plain .md + SQLite). These four documents were produced before this repo existed and are placed here as the worked example of the SDLC artifact chain.

| Phase | Document | Path |
|---|---|---|
| 01 Planning | Project charter + risk register (restructured from the Serat Blueprint v0.1) | docs/01-planning/project-charter.md, docs/01-planning/risk-register.md |
| 02 Requirements | PRD (135 FR/NFR, traceability, glossary, assumptions) + user stories (40) — restructured from Requirements Baseline 1.0 | docs/02-requirements/prd.md, docs/02-requirements/user-stories.md |
| 03 Design | Architecture (SDD 1.0 restructured), threat model, twelve ADRs from the decision register | docs/03-design/architecture.md, docs/03-design/threat-model.md, docs/03-design/adr/0001…0012 |
| 03 Design / UI | Serat Identity — name, brand colours, typography | docs/03-design/ui/serat-identity.md |
| 03 Design | Index DDL, runnable as delivered | schema/001_init.sql |
| 04 Implementation | Coding standards (incl. the per-teammate worktree workflow), developer setup | docs/04-implementation/coding-standards.md, docs/04-implementation/dev-setup.md |
| 05 Testing | Test plan (TC-nnn), benchmark vault shape and invocation, the reference machine budgets are measured on | docs/05-testing/test-plan.md, docs/05-testing/benchmark-vault.md, docs/05-testing/reference-machine.md |
| 05 Testing | Benchmark vault generator — `cargo run --release -p vaultgen -- --preset 20k` | bench/vaultgen/, bench/README.md |
| 01 Planning | P0 spike task list and status | docs/01-planning/tasks-p0-spike.md |

Originals (styled HTML with embedded diagrams) are in docs/_source/. The Markdown copies are pandoc conversions: tables and headings are intact; the six inline diagrams in the design description are marked *(embedded image)* — open the HTML original for those.

ID conventions already used by these documents (adopt them in CLAUDE.md going forward): `FR-AREA-nn` functional, `NFR-AREA-nn` non-functional, `US-nn` user stories, `A-nn` assumptions, `C-nn` constraints. Test cases use `TC-nnn`, because `T-nn` is reserved for threats in the threat model. IDs are permanent — dropped items are marked dropped, never reused.
