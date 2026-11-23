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

## Rebuilding: one line per reason (#158)

`IndexState::Rebuilding` carries a `RebuildReason` (`contracts/ipc.rs`), and the strip has rendered it raw: `Rebuilding index (corrupt)`. That puts an internal word in visible text, against the rules above. "Corrupt" also alarms a user whose notes are untouched. The reason phrase below replaces the `Rebuilding index (<reason>)` prefix. The rest of the line stays as it is, ` — search and queries are incomplete until this finishes · <done>/<total>`: the progress count is the one number `Rebuilding` shows, and architecture §3.4 *What the user sees* calls for it. The table covers every member of `RebuildReason`. A member added to the contract gets a row here before it ships.

| Wire value (`reason`) | When (contract) | Visible reason phrase | Tooltip / accessible description |
|---|---|---|---|
| `no_index` | No index yet: a first open, or the user deleted `.serat/` (US-07) | `Building the index` | `This vault has no search index yet, so Serat is building one from your notes. Your notes are not changed. You can keep editing.` |
| `index_version` | The index was written by a different version of Serat (ADR-0002) | `Updating the index for this version of Serat` | `This version of Serat reads notes differently from the one that built the search index, so it is rebuilding the index from your notes. Your notes are not changed. You can keep editing.` |
| `interrupted` | A previous rebuild did not finish | `Completing an unfinished index rebuild` | `The last rebuild of the search index did not finish, so Serat is building it again from your notes. Your notes are not changed. You can keep editing.` |
| `vault_moved` | The index names a different vault: the folder moved, or `.serat/` was copied from another vault | `Rebuilding the index for this vault` | `The search index was made for this vault at another location, or copied from a different vault, so Serat is rebuilding it from your notes. Your notes are not changed. You can keep editing.` |
| `corrupt` | The index failed a check, at open or during the session (§3.4) | `Repairing the index` | `Serat found a problem in its search index, so it is rebuilding it from your notes. Your notes are not affected. You can keep editing.` |
| `user_requested` | The user ran the rebuild command from the palette | `Rebuilding the index, as you asked` | `Serat is rebuilding the search index from your notes, as you asked. Your notes are not changed. You can keep editing.` |
| any other (unknown to this build) | A member added after this build | `Rebuilding index` | `Serat is rebuilding the search index from your notes. Your notes are not changed. You can keep editing.` |

**Wording rules for this table.**
- **Each phrase says what is happening, not what went wrong.** No phrase uses "error", "failed" or "corrupt".
- **No phrase or tooltip contains a wire value**, including the ordinary English word "interrupted", so the acceptance check below can be literal.
- **Every tooltip ends with the same two sentences** about notes and editing (design goal 1, and the rule that editing is never in question).
- **`corrupt` says "not affected"** rather than "not changed", because the user's worry there is damage, not modification.

**With #157's fact.** When `index_check_problems` is present (a `corrupt` rebuild started by the post-unclean-open check, #69), the line reads `Repairing the index — search and queries are incomplete until this finishes · <done>/<total>`. The fact `Index check found problems` sits beside it with its own tooltip, and the line does not repeat it.

**Announcement.**
- **Entering `Rebuilding`** announces once, politely: `<reason phrase>. Search and queries are incomplete until it finishes. You can keep editing.` For example, `Repairing the index. Search and queries are incomplete until it finishes. You can keep editing.`
- **Progress events** (`done`/`total` moving) are silent.
- **A change of reason within `Rebuilding`** (one rebuild ends and another starts) is announced once, as an entry.
- **When #157's fact arrives in the same update**, the fact's utterance replaces this one, as #157's section rules, so that update makes exactly one announcement.
- **The `Ready` or `Unavailable` that follows** is announced as usual.

## What the front end must not do

- Treat an unknown `reason` as an error. `OpeningReason` may gain members. Render an unknown reason as the plain `Opening index…`.
- Infer the reason from elapsed time ("still opening after 2 s, so it must be…"). The field is the only source.
- Offer a "Cancel" or "Rebuild now" action here. There is nothing safe for it to do: starting a second build is exactly what this wait prevents (ADR-0001).

## Acceptance (for P1 #117's front-end half)

- `describeIndexState({ kind: "opening" })` and `describeIndexState({ kind: "opening", reason: "previous_index_still_closing" })` return different text, and the second names neither an internal term nor a number.
- An `IndexState` with an unrecognised `reason` string renders as the plain opening text and does not throw.
- The transition from the waiting state to `Rebuilding` replaces the text without a modal or a layout shift.
- `describeIndexState({ kind: "opening", reason: "index_held_by_another_process" })` returns text different from both other opening states, and names neither an internal term nor a number.

## Acceptance (for #158)

- For every `RebuildReason` member in `contracts/ipc.rs`, `describeIndexState({ kind: "rebuilding", reason, done, total, walk_complete })` starts with that row's reason phrase. **No visible string, and no tooltip, contains any raw `RebuildReason` wire value** (`no_index`, `index_version`, `interrupted`, `vault_moved`, `corrupt`, `user_requested`). The test iterates the members, so a member added to the contract without a row fails it.
- An unrecognised `reason` renders `Rebuilding index` with the generic tooltip and does not throw.
- Entering `Rebuilding` makes one announcement with the reason phrase; progress events make none; a change of reason makes one.
