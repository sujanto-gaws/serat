# AIResearch — Serat

Serat, a local-first Obsidian-alternative knowledge app (Tauri v2 + Rust core, Vue 3 frontend), built by a Claude Code agent team. See CLAUDE.md for constraints, roles and working agreement; docs/SERAT-INDEX.md for the artifacts.

## Layout
```
.claude/
  settings.json        # enables agent teams + quality-gate hooks
  agents/              product-analyst, architect, frontend-designer, rust-core-dev, tauri-app-dev, vue-frontend-dev, test-engineer, code-reviewer, release-engineer
  hooks/               # TaskCompleted / TeammateIdle gates
  workflows/           # review-branch, traceability-audit
  skills/              # project-specific skills (empty for now)
docs/01..07-*/         # SDLC artifacts by phase, with templates
src/                   # application code
tests/                 # test suites (cite REQ ids)
```

## Starting a team session
1. `claude` in this folder (agent teams are enabled via .claude/settings.json).
2. Ask the lead: "Form a team: product-analyst, architect, backend-dev, frontend-dev, test-engineer, code-reviewer. Start with <feature>."
3. Watch the shared task list; the hooks reject task completions that lack an artifact link or have failing tests.
