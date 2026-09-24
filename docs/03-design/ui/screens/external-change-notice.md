# Screen: External-change conflict notice

Serves: FR-VLT-04, FR-VLT-05, NFR-REL-01, C-04 · US-06, US-07
Design basis: architecture.md §3.3 and §9.3, ADR-0016 §4 "The front end applies one of three outcomes"

## The three outcomes, and what the UI does for each

ADR-0016 defines three buffer states an external change can meet. Only one of them produces anything visible — the other two are correct precisely because they are silent, and this spec is explicit about that so a reviewer doesn't add UI to the two states that must stay quiet.

**A save is never in flight while an `ExternalChange` is being applied, and there is no retry loop anywhere in this flow.** The sequence is always: a save is sent with `base_hash`; if the disk hash has since moved, the core refuses it (`StaleHash`) *and* — because the watcher already saw the same disk change — emits the corresponding `ExternalChange` for that note. The front end does not poll or wait blindly: it waits specifically for that `ExternalChange`, applies exactly one of the three outcomes below, adopts the new base, and only then re-issues the save against the new `base_hash`. For the clean and dirty-disjoint outcomes this whole sequence is invisible and effectively immediate (no user-facing "retry" ever appears); for the overlapping outcome, the resend is *replaced* by the notice below — nothing auto-saves until the user resolves it.

| Buffer state at the moment of the external change | Applied silently? | UI |
|---|---|---|
| **Clean** (no unsaved changes) | Yes — the edit script is applied as one transaction, cursor and undo mapped through it | **None.** No toast, no flash, nothing. The requirement (FR-VLT-05) is that the cursor stays at line 40 while line 5 changes underneath it — the correct UI for that is the absence of any interruption. A subtle, non-modal indicator is not warranted here because nothing was lost or requires a decision. |
| **Dirty, disjoint** (unsaved local edits that don't touch the same ranges) | Yes — external edits are mapped through the local ones and applied; buffer stays dirty | **A quiet status-strip mention only**: the save-state text in the status strip (`components.md` §1) briefly reads "Updated externally" for 3s before returning to "Unsaved," replacing nothing about the editing experience itself. This is deliberately far quieter than the conflict notice below — it is informational, not a decision point. |
| **Dirty, overlapping** (unsaved local edits whose ranges touch or intersect the external ones) | **No — this is the conflict.** Nothing is applied. | **The notice specified below.** |

## Layout — the conflict notice

Non-modal, per architecture.md §9.1's own rule ("no modal dialogs on the write path") — this notice exists *because* a write was about to be lost or ambiguous, which is exactly the routine-event-not-a-fault case that rule is written for. Docked to the top of the affected editor tab's pane (not window-centred, not a toast that could appear while the user is looking elsewhere), `elevation.overlay`, `accent-secondary-bg` (brass) — brass, not rust, because this is an expected event in a vault other tools also write to (§3.3: "a concurrent external edit is a normal event"), not a failure of the application.

```
┌ editor pane, note.md ─────────────────────────────────────┐
│ ┌ This note changed outside Serat ─────────────────────┐  │
│ │ Your unsaved edits overlap the external change.        │  │
│ │ Nothing has been applied yet — pick which wins.        │  │
│ │                                                          │  │
│ │  [ Keep mine ]   [ Take theirs ]   [ Compare ]          │  │
│ └────────────────────────────────────────────────────────┘  │
│                                                                │
│         (the buffer beneath, exactly as the user left it,    │
│          fully editable, untouched by the notice)             │
└────────────────────────────────────────────────────────────┘
```

The buffer under the notice is never dimmed, disabled or covered — the user can keep typing while the notice sits there, because the conflict doesn't need to be resolved before they can keep working; it needs to be resolved before their *next save* (which the app already guards with `StaleHash`, so nothing is at risk in the meantime).

## Actions (verbatim from ADR-0016 §4, this spec designs their presentation only)

| Action | What it does | Presentation |
|---|---|---|
| **Keep mine** | Rebases the local `ChangeSet` onto the disk `Text` and adopts the disk hash as the new base. This is **not** a blanket overwrite: only the ranges that actually overlapped are decided in the local edit's favour: the next save patches just those local ranges, so every part of the external change that didn't overlap the user's edit is still applied and survives. | Primary-weight button (`accent-interactive` fill) — the default action for someone who trusts their own in-progress edit over an external tool's *at the point they actually collide*. Confirmed inline: notice collapses to a 3s fading confirmation, "Your overlapping edits win — the rest of the external change is kept," then disappears. No further confirmation dialog (a second modal to confirm a non-destructive-until-save choice would violate the same no-modal-on-write-path rule this whole notice exists to honour). |
| **Take theirs** | Replaces the buffer in one **history-recorded** transaction — undo brings back what was typed, per the ADR's explicit guarantee that "nothing is discarded silently in any branch." This one *is* a full replacement: the user's overlapping edit is discarded (recoverably, via undo), not merged. | Secondary-weight button (hairline border, not filled) — visually less default than "Keep mine," a judgement call reflecting that discarding the user's own in-progress typing is the more surprising of the two outcomes, but it is one click, not buried, because it's frequently the right choice (e.g. Claude Code made a substantive edit the user wants). Confirmed inline the same way, and the status strip's undo affordance (already always present) is the stated way back — no extra "Undo this" button duplicating it. |
| **Compare** | Opens a diff. | Opens a read-only, non-modal diff view **in place of the editor pane's content** (not a new window, not a third overlay stacked on the notice) — old (disk) on the left, current buffer on the right, both scrollable independently with the overlapping range highlighted in both. The notice's three actions remain visible/accessible from the diff view's own header, so choosing after comparing doesn't require navigating back first. `Esc` from the diff view returns to the editor with the notice still present (Compare is inspection, not a fourth outcome). |

## Persistence and dismissal

- The notice does **not** auto-dismiss and does **not** go away on `Esc` in the sense of discarding the conflict — `Esc` here means "hide the notice, keep the conflict pending," equivalent to minimising it into a small persistent indicator (a rust-free brass dot on the tab, distinct from the unsaved brass dot by its position — top-right corner of the tab vs. left of the note name — and by tooltip: "External change pending — click to review"). This exists so the notice never becomes an obstacle the user is forced to deal with mid-thought, while guaranteeing (per NFR-REL-01, "nothing is lost silently") that the pending state is never fully invisible.
- Switching away from the note and back re-shows the full notice (not just the minimised dot), so returning to the note surfaces the decision again rather than leaving it perpetually minimised and easy to forget.
- Closing the tab while a conflict is pending prompts (inline, not a native OS dialog) with the same three actions **plus a fourth, explicit button: "Discard my edits."** There is no default/implicit choice on close — closing the tab never silently resolves the conflict one way or the other. "Discard my edits" is its own labelled action (equivalent to Take theirs, stated in the vocabulary of "I'm closing this and don't need my version") precisely so a user who just wants to close the tab doesn't have to parse "Take theirs" as an answer to "what happens to my typing," and so the choice is never made *for* them by the act of closing.

## States

| State | Presentation |
|---|---|
| Conflict just detected | Full notice as drawn, `accent-secondary-bg`, focus **not** stolen (the user may be mid-keystroke elsewhere; the notice appears without moving the text cursor). |
| Minimised (via `Esc`) | Small brass dot on the tab, tooltip on hover, click reopens the full notice. |
| Resolved via Keep mine / Take theirs | 3s fading confirmation, then gone; tab dot (unsaved, if any) returns to its normal meaning. |
| Comparing | Diff view replaces editor content; notice actions pinned in the diff header. |
| Multiple notes with pending conflicts simultaneously (e.g. Claude Code batch-edits several open notes) | Each note's tab carries its own minimised dot independently; only the currently active tab's full notice is shown at a time — switching tabs surfaces that tab's notice, consistent with "returning to the note surfaces the decision again" above. No stacked/global "3 conflicts pending" summary is designed for P1; each is handled where it lives. |

## Keyboard map

| Action | Binding |
|---|---|
| Keep mine | `⌘` `⇧` `K` while the notice is visible, or click |
| Take theirs | `⌘` `⇧` `O` while the notice is visible, or click — **not** `⌘⇧T`, which is reserved app-wide for "Reopen closed tab" (see the master keybinding table in `components.md` §7) |
| Open compare | `⌘` `⇧` `D` while the notice is visible, or click |
| Minimise notice | `Esc` |
| Reopen minimised notice | `↵` on the focused tab's minimised indicator, or click |

This table is a subset of the single authoritative binding set in `components.md` §7.

## Accessibility

- The notice announces via an assertive live region once (not repeatedly) when it first appears — it is a genuine decision point, unlike the quiet disjoint-merge status-strip mention, which is polite/non-interrupting.
- All three actions are real labelled buttons, reachable by `Tab` without leaving the keyboard, and the diff view is fully keyboard-navigable (`↓`/`↑` between changed ranges, matching the backlinks/search convention of arrow-key list navigation).
- The minimised indicator has an accessible name distinct from the unsaved dot ("External change pending, note.md") so assistive tech doesn't conflate the two brass signals despite their shared colour (components.md's `unsaved-dot` vs. this notice's own token — see tokens.json comment on why they're named separately).
