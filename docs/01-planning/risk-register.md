---
project: Serat
artifact: Risk register
phase: 01-planning
derived_from: Serat Blueprint draft v0.1 §12 "Risks, honestly"
---

# Risk Register — Serat

Severity as stated in the blueprint. Likelihood and impact to be split out once P0 produces real numbers. Owner is the lead unless reassigned.

| ID | Risk | Severity | Mitigation | Owner | Status |
|---|---|---|---|---|---|
| RISK-001 | Write-back corrupts notes | Critical | Byte-range patches only; hash guard (stale-range rejection); atomic rename; git auto-commit before any view-originated write in first releases. Property-test the patcher against a corpus of real notes. | lead | Open — de-risk standalone before P2 (next move #4) |
| RISK-002 | Rust ramp-up eats the schedule | High | Small, boring Rust surface; product logic pushed into SQL and Vue; no async-framework gymnastics. P0 exists partly to measure this friction before committing 30 weeks. | lead | Open — measured at P0 gate |
| RISK-003 | CodeMirror 6 hybrid live preview is deep water | High | Largest single frontend cost. Start from an existing CM6 Markdown live-preview implementation, not from zero; time-box inside P1; fall back to split preview if it overruns. | lead | Open — P1 |
| RISK-004 | Binary parsing is an attack surface | High | Every extractor runs in a short-lived child process with wall-clock limit, memory cap and no network; a crash marks the file unextractable instead of taking the app down. Fuzz the MIME dispatcher, not just the parsers. | lead | Open — P2 |
| RISK-005 | Solo project loses momentum | High | Phase gates chosen so each phase ends with something the owner uses daily. Dogfooding is the momentum plan. | lead | Open — ongoing |
| RISK-006 | Obsidian's plugin ecosystem (breadth is unwinnable) | Medium | Do not compete on breadth. Compete on the pillars plus vault compatibility, so leaving Obsidian is never all-or-nothing. | lead | Accepted |
| RISK-007 | Scope creep into "second Obsidian" | Medium | The "deliberately not" list in the charter is the contract. Anything on it needs an explicit decision to reopen. | lead | Open — ongoing |
| RISK-008 | Obsidian closes the gap (Bases keeps improving) | Medium | Lead with what is hardest to copy — writable views and a core AI layer — not query syntax. | lead | Open — watch |
| RISK-009 | Extractors are a long tail | Medium | One format per iteration behind a trait: PDF first, Office next, rest optional. A format with no extractor still indexes as a typed asset row; coverage gaps degrade capability rather than breaking the vault. | lead | Open — P2 |
| RISK-010 | Embedding model ships as a fat binary | Low | Download the model on first AI use, not at install. The app must be fully useful with the AI layer never enabled. | lead | Open — P4 |
| RISK-011 | Name collision with Serato (DJ software) if the product goes public | Low | Watch search results; revisit naming before any public launch. | lead | Open — watch |
