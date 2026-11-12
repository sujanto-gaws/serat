# Copy brief: the index state in the status strip

Serves: NFR-REL-02, NFR-REL-01, FR-VLT-04 · US-07
Design basis: architecture.md §3.4 *What the user sees* and *Why the index can wait at `Opening`* (task #115), `contracts/ipc.rs` `IndexState` and `OpeningReason`, components.md §1 (status strip)
Owner of the words: **frontend-designer** (this brief sets what each message must say, not the final wording). Renderer: vue-frontend-dev, from the field, in `describeIndexState` (`app/src/stores/vault.ts`). Implementing task: **#117 (to be filed)**.

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
| any other | As today (`Ready`, `Rebuilding`, `Unavailable`) | Unchanged by this brief | Unchanged |

**Tooltip / accessible description** for the waiting state (on hover and as the status item's `aria-description`): `This vault was closed while its index was still being built. Serat finishes that first so two builds never write the same index. You can keep editing.`

**Announcement.** The strip's live region announces the waiting state **once**, politely, when it is entered, and the following `Rebuilding` or `Ready` as usual. It does not re-announce while the state holds.

## What the front end must not do

- Treat an unknown `reason` as an error. `OpeningReason` may gain a member (a lock held by another Serat process is the expected one). Render an unknown reason as the plain `Opening index…`.
- Infer the reason from elapsed time ("still opening after 2 s, so it must be…"). The field is the only source.
- Offer a "Cancel" or "Rebuild now" action here. There is nothing safe for it to do: starting a second build is exactly what this wait prevents (ADR-0001).

## Acceptance (for #117's front-end half)

- `describeIndexState({ kind: "opening" })` and `describeIndexState({ kind: "opening", reason: "previous_index_still_closing" })` return different text, and the second names neither an internal term nor a number.
- An `IndexState` with an unrecognised `reason` string renders as the plain opening text and does not throw.
- The transition from the waiting state to `Rebuilding` replaces the text without a modal or a layout shift.
