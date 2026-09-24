# Screen: .serat/ gitignore offer

Serves: NFR-CMP-04 · US-01
Design basis: PRD NFR-CMP-04 ("shall offer to add one; it shall not create or edit a .gitignore the user did not ask it to touch")

**Dependency, not an assumption.** The current write path refuses any non-`.md` target (T-06), so `write_file`/the patch-save command cannot perform the "Add to .gitignore" write below. This screen depends on a **dedicated IPC command** — append-only, hard-scoped to `<vault root>/.gitignore` and nothing else, explicitly granted in the ACL, with its own T-06 amendment recording why this one path is allowed to touch a non-`.md` file — that the architect owns and is adding to the IPC contract (task #7). Nothing in this spec should be read as already having that command; **"Add to .gitignore" is designed against it and cannot ship before it lands.**

## Placement and shape

**Non-modal**, docked to the top of the file tree pane (the pane the offer is actually about), not centred over the window. This follows architecture.md §9.1's spirit even though §9.1 names the *write path* specifically: the requirement itself says "offer," and an offer that blocks the rest of the app until answered would misrepresent something optional as something urgent. It appears once, on first open of a vault that is a git repository with no `.serat/` ignore entry, and never again once answered (recorded per-vault, in `.serat/` itself — see *State persistence* below, which is safe under C-02 since losing the record only means the offer might be shown again after an index wipe, not that anything breaks).

```
┌ tree pane ───────────────────────────────────┐
│ ┌ .serat/ isn't ignored by git ─────────────┐ │
│ │ This vault is a git repo. Add .serat/ to  │ │
│ │ .gitignore so the index isn't committed?  │ │
│ │                                             │ │
│ │  [ Add to .gitignore ]  [ Not now ]  [ ⋯ ] │ │
│ └─────────────────────────────────────────────┘ │
│ ▾ Projects                                     │
│   …                                            │
└─────────────────────────────────────────────────┘
```

- Card: `elevation.overlay` (it floats above the tree content it's docked to, so it earns the one elevation exception), `accent-secondary-bg` (brass tint) rather than `bg-surface` — brass here reads as "worth a moment of attention," consistent with its role as the system's one secondary-signal colour (components.md `meta.principles`), and deliberately not `accent-error-bg` (rust), because an untracked index directory is not a problem, it's a housekeeping offer.
- Body copy: two short sentences, plain language, stating exactly what will happen and why, per the writing guidance — no "Recommended" badge, no exclamation.

## Actions

| Action | Result |
|---|---|
| **Add to .gitignore** | Calls the dedicated append-only IPC command named above, which appends `.serat/` (with a leading newline if the file doesn't already end in one) to the vault root's `.gitignore`, creating the file if none exists, and refuses to touch any other path. This is the one and only write this feature ever performs, and it happens only from this explicit click — never automatically, per the requirement's own wording. On success, the card is replaced in place by a one-line confirmation, "Added `.serat/` to .gitignore," which fades out after 3s (no persistent success banner needed — the tree itself is the lasting evidence, and a git-aware user can check). |
| **Not now** | Dismisses the card for this session. The offer reappears on the *next* cold open of the vault if the ignore entry still isn't present — it is not a permanent "seen it" dismissal, because the underlying condition (an untracked `.serat/`) hasn't changed. |
| **⋯ (overflow)** | One item: **Don't ask again for this vault** — a stronger, explicit opt-out, recorded per-vault. This is the affordance for someone who has `.serat/` ignored globally (a global gitignore, or deliberately wants it tracked) and doesn't want to be asked repeatedly; it is deliberately one extra click away from the two primary actions so it isn't mistaken for "Not now." |

## State persistence

Recorded in **`.serat/settings-local.json`** — one specific, per-vault, disposable file inside `.serat/` (not the vault's own settings TOML, since this is per-vault-copy state, not a user preference carried across vaults, and not a second file competing with it) — as one of: unset (offer eligible), `dismissed-session` (not persisted at all — session memory only), or `declined` (from "Don't ask again"). Because it lives inside `.serat/`, an index rebuild or a fresh clone of the vault naturally resets it — which is the correct behaviour: a fresh clone genuinely doesn't have the ignore entry yet either, so being asked again is accurate, not a bug (C-02 compliance, same reasoning architecture.md §9.2 uses for `.serat/history.json`).

## States

| State | Presentation |
|---|---|
| Eligible, not yet shown this session | Card shown once, on vault open, after the tree's first paint (never blocking `tree_first_screen` — it appears a beat after, so it never competes with the ADR-0015 usability marks). |
| Dismissed this session ("Not now") | Hidden; tree occupies the full pane height. |
| Declined permanently | Hidden; never recomputed unless `.serat/` is deleted/rebuilt. |
| `.gitignore` already contains an entry that would cover `.serat/` (e.g. a wildcard, or an entry added by another tool since last check) | Card never appears — the check that decides eligibility (does a matched ignore pattern already cover `.serat/`) runs before rendering, not "has Serat itself added it." |
| Vault is not a git repository | Card never appears — the requirement is conditioned on the vault being a git repo. |
| Write fails (permissions, read-only filesystem) | Card's confirmation is replaced with a plain-language failure line: "Couldn't write .gitignore — check the file isn't read-only," and the **Add to .gitignore** button remains so the user can retry after fixing the cause, rather than the card disappearing on failure (silently disappearing after a failed write would look like it succeeded). |

## Keyboard map

| Action | Binding |
|---|---|
| Focus the card when it appears | Not automatic — focus stays wherever the user was (opening a vault shouldn't steal focus for a non-urgent offer); the card is reachable via `Tab` from the tree pane's top, or via the palette command `Review .serat/ git status` |
| Accept | `↵` when the card has focus (default button is **Add to .gitignore**) |
| Dismiss | `Esc` (equivalent to "Not now") |

This table is a subset of the single authoritative binding set in `components.md` §7.

## Accessibility

- The card is announced via a polite live region when it appears (not assertive — it is not urgent), so a screen-reader user is informed without being interrupted mid-task.
- All three actions are real buttons with full labels, not icon-only controls, including the overflow menu's single item.
