# Copy brief: the index state in the status strip

Serves: NFR-REL-02, NFR-REL-01, FR-VLT-04 · US-07
Design basis: architecture.md §3.4 *What the user sees* and *Why the index can wait at `Opening`* (task #115), `contracts/ipc.rs` `IndexState` and `OpeningReason`, components.md §1 (status strip)
Owner of the words: **frontend-designer** (this brief sets what each message must say, not the final wording). Renderer: vue-frontend-dev, from the field, in `describeIndexState` (`app/src/stores/vault.ts`). Implementing task: **P1 #117**.

## Why this brief exists

Until #96, `Opening` lasted only the open check, 5–8 ms on a warm index, so "index: opening" was a flicker nobody read. Since #96, a vault re-opened while the previous session's index actor is still finishing a rebuild **stays at `Opening` until that actor exits**. On the 20,000-note vault that can be tens of seconds, and there is no progress count, because the old actor no longer reports. A bare "opening" for that long reads as hung. The contract now carries the reason, `IndexState::Opening { reason: "previous_index_still_closing" }`, and this brief sets what the user is told.

## Rules every message follows

- **Status strip only.** Not a modal, not a toast, not a spinner over content, and not a banner over the editor (architecture §9.1). The same slot the other index states use.
- **Editing is never in question.** The user can type, save and open notes throughout. The message must not suggest otherwise, and must not ask them to wait before working.
- **Say what is unavailable and why**, the same pattern as `Rebuilding`'s "search and queries are incomplete until this finishes".
- **No fake progress.** There is no count and no percentage for this state, so none is shown, and no animated bar that implies one. A static or subtly pulsing dot is the most that may move.
- **Neutral colour** (`text-secondary`). This is not a warning: nothing is wrong, and nothing is asked of the user.
- **No internal words** in the visible text: not "actor", "writer", "detached", "index.db", "lock".

## The states

| Wire value | When | Must convey | Suggested copy (frontend-designer may revise) |
|---|---|---|---|
| `{ kind: "opening" }` (no reason) | The open check is running. Normally milliseconds. | Nothing beyond "starting". It usually disappears before it is read. | `Opening index…` |
| `{ kind: "opening", reason: "previous_index_still_closing" }` | A re-open of a vault whose earlier session is still finishing its index (#96). Can last a whole rebuild. | (1) Search, backlinks and views are waiting. (2) Why: the previous session of this vault is still finishing. (3) It will continue by itself. (4) Editing works. | `Finishing the previous session's index — search will start when it's done` |
| `{ kind: "opening", reason: "index_held_by_another_process" }` | Another Serat session holds this vault's index lock (#154): a second window, a benchmark run still finishing, another open of the same vault in this process, or a lock a crashed process left that Windows has not yet released. The open retries every 2 s for up to 60 s (#155); after that the state becomes `Unavailable`. | (1) Search, backlinks and views are waiting. (2) Why: another Serat session is using this vault's index. (3) It will continue by itself when that session is done. (4) Editing works. | `Another Serat session is using this vault's index — search will start when it's free` |
| any other | As today (`Ready`, `Rebuilding`, `Unavailable`) | Unchanged by this brief | Unchanged |

**Tooltip / accessible description** for the waiting state (on hover and as the status item's `aria-description`): `This vault was closed while its index was still being built. Serat finishes that first so two builds never write the same index. You can keep editing.`

**Tooltip / accessible description for `index_held_by_another_process`:** `Only one Serat session can update a vault's index at a time. Serat keeps trying for a while and tells you if it cannot get it. You can keep editing.` After the retry bound the state becomes `Unavailable`, whose message is the Rust side's and is shown as written.

**Announcement.** The strip's live region announces each waiting state once, politely, when it is entered. It also announces the following `Rebuilding`, `Ready` or `Unavailable` as usual. It does not re-announce while a state holds, and it does not announce the plain `Opening` that follows a cleared reason: that state lasts only until the open check's first state and would be noise.

## Index check found problems (ratified, #157)

`IndexStatus.index_check_problems` is present only while a rebuild that the post-unclean-open integrity check started (#69, reason `corrupt`) is running, and is never zero. The strip shows **one quiet fact beside the `Rebuilding` text** while it is present and removes it when the field goes absent.

- **Visible text** (no number, no second "rebuilding": the `Rebuilding` line beside it already says so): `Index check found problems`
- **Tooltip / accessible description** (the count appears only here; `problem` when N is 1): `When Serat last closed unexpectedly, its check of the search index found N problems, so it is rebuilding the index from your notes. Your notes are not affected. You can keep editing.`
- **Announcement:** once, politely, in the same update that moves the strip to `Rebuilding`, as **one** utterance that replaces the `Rebuilding` announcement that update would otherwise queue: `Index check found problems. Rebuilding the index from your notes. You can keep editing.` Not repeated on progress events; nothing when the fact clears; the `Ready` or `Unavailable` that ends the rebuild is announced once, with its visible text.
- Neutral colour (`text-secondary`), no icon alarm, no modal, no action: the rebuild is already running and there is nothing for the user to do.

## What the front end must not do

- Treat an unknown `reason` as an error. `OpeningReason` may gain members. Render an unknown reason as the plain `Opening index…`.
- Infer the reason from elapsed time ("still opening after 2 s, so it must be…"). The field is the only source.
- Offer a "Cancel" or "Rebuild now" action here. There is nothing safe for it to do: starting a second build is exactly what this wait prevents (ADR-0001).

## Acceptance (for P1 #117's front-end half)

- `describeIndexState({ kind: "opening" })` and `describeIndexState({ kind: "opening", reason: "previous_index_still_closing" })` return different text, and the second names neither an internal term nor a number.
- An `IndexState` with an unrecognised `reason` string renders as the plain opening text and does not throw.
- The transition from the waiting state to `Rebuilding` replaces the text without a modal or a layout shift.
- `describeIndexState({ kind: "opening", reason: "index_held_by_another_process" })` returns text different from both other opening states, and names neither an internal term nor a number.
- `index_check_problems` present renders `Index check found problems` beside the `Rebuilding` text, with no digit and no second "rebuilding", and the count only in the tooltip; absent (or 0) renders nothing (#157). The update that brings the fact makes exactly one live-region announcement.
