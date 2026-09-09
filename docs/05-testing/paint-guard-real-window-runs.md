---
project: Serat
artifact: Run mechanics for paint-guard mutation runs against a real Tauri window
phase: 05-testing
owner: tauri-app-dev (mechanics only — see "What this document does not decide")
serves: NFR-PRF-01, NFR-PRF-08, US-35 · F-377, F-392, F-393, F-406, F-461, F-464 · ADR-0014
---

# Running the Paint Guard Against a Real Window

Task #121 discharges F-392: the two paint-guard mutations have to be re-run
independently, against a real Tauri/WebView2 window rather than against `cargo test`.
F-393 deferred that run in its own text, #105 merged at `1894f20`, and F-464 found the
obligation still open while #101 cites F-392 as a gate-report input.

This document is the **mechanics only**. It exists so that an independent run costs an
hour rather than an afternoon of rediscovery.

## What this document does not decide

Deliberately split, so nobody has to infer it later:

| | Owner |
|---|---|
| How to build, launch, read and recognise a run | this document (tauri-app-dev) |
| **Which mutations to apply, what counts as red, and the verdict** | **code-reviewer** — F-392 names them |

The reason is F-445 inverted. F-445 is this project's best review outcome because
code-reviewer pre-registered the mutation targets *before* the instrument existed, so
its author aimed at them rather than being surprised by them. Here the code already
exists, so the same separation has to be enforced the other way round: an author who
writes both the procedure and the targets can produce a procedure that never reaches
the sharp one, without intending to. Nothing below names a mutation.

---

## Read this first: the step where a run silently does nothing

**The frontend is embedded into `serat.exe` at compile time. Editing `app/src/` or
`crates/serat-app/webview/` and then launching the binary changes nothing at all, and
the run comes back clean.**

`crates/serat-app/tauri.conf.json` sets `"frontendDist": "webview"` and has **no
`beforeBuildCommand`**. So `cargo build` never invokes vite, and the webview assets are
compiled into the executable rather than read from disk at launch.

This is not inferred from the config. It was measured, both ways:

| What was done | Result |
|---|---|
| `crates/serat-app/webview/boot.js` replaced by a 60-byte stub that mounts nothing and never calls `mark_first_paint`; **existing binary launched, no rebuild** | **exit 0**, complete trace written, `paint_evidence_ms` **352.0**, `t0_to_window_visible_ms` 1583.4 |
| Same stub, then `cargo build --release -p serat-app`, then launched | **exit 2**, no trace, stderr: `serat: no startup trace after 15s (timed out): no window_visible mark: the webview never reported a first painted frame. That is a shell that did not paint, not a shell that painted fast.` |

The first row is the hazard in one line: **a mutation that deletes the entire paint
mark produced a clean, plausible, complete trace carrying real paint evidence**, because
the mutation never reached the binary. That is F-461's failure mode — a run that could
not have failed — landing on the very mechanism F-461 came from.

**Every frontend mutation needs all three steps, in order.** A Rust-side mutation needs
only the third.

```
cd app && npm ci                 # once; node_modules is not checked in
cd app && npm run build          # vue-tsc --noEmit && vite build -> crates/serat-app/webview/
cargo build --release -p serat-app
```

Two things about step 2 worth knowing before it surprises you:

- `vite.config.ts` sets `emptyOutDir: true` and `entryFileNames: "boot.js"` (fixed
  names, no content hash). The build **wipes and regenerates**
  `crates/serat-app/webview/` — `boot.js`, `index.html` and `shell.css` are all
  **committed build artifacts**, so `git status` will show them as modified.
- Because they are committed, an innocent rebuild can produce a diff with no source
  change if the toolchain differs from whoever last committed them. Check `git status`
  before and after; restore with `git checkout crates/serat-app/webview/` and do not
  commit incidental regeneration.

**Positive control, and it is cheap enough to be non-optional.** Before trusting any
result, apply the stub above, rebuild, and confirm the run goes to exit 2 with the
`no window_visible mark` message. That proves your build pipeline reaches the binary.
Then restore and rebuild. A run whose pipeline was never proven is not evidence.

---

## Where the mark comes from

Worth having in front of you, because it decides what a mutation can and cannot hide.

1. `app/src/startup.ts` → `schedulePaintMark()` waits for **both** `whenDoubleRaf()` and
   `whenFirstContentfulPaint()`, then calls `markFirstPaint`. The FCP entry is read via
   `PerformanceObserver` with `buffered: true`, not synchronously inside the second rAF
   callback — a real WebView2 run showed the entry's *availability to script* lags its
   own `startTime`.
2. `crates/serat-app/src/commands.rs` → `mark_first_paint` is a one-line delegate to
   `AppState::mark_first_paint` (this shape is F-397's fix: the earlier version put the
   forwarding logic where no test could drive it).
3. `crates/serat-app/src/startup.rs` → `validate_paint_evidence` requires `raf1_ms` and
   `raf2_ms` finite and **strictly increasing**, and calls
   `frame_precedes_paint_diagnostic` (reports, never refuses — F-406/F-424).
4. `Startup::mark_first_paint` requires `first_contentful_paint_ms` **finite and
   positive**, and returns `PaintEvidenceRejected` rather than defaulting. A repeat
   call returns `Ok(false)` and is **ignored**.

**There is no other way to set `t0_to_window_visible_ms`.** No Rust path stamps it. A
shell whose evidence is rejected never marks, `Startup::trace()` fails with
`TraceIncomplete`, and `--exit-after-ready` exits non-zero.

---

## The vault

Use a scratch vault you generate. **Never the canonical 20k benchmark vault** — the app
writes a real `.serat/` into whatever it opens, which is correct production behaviour
and has already broken the shared vault digest once (F-401, and again as
`F-NEXT/cold-start-poisons-the-shared-vault-digest`).

Small is fine. The paint marks are not a function of vault size; a few hundred notes is
enough, and a cold vault (no `.serat/`) only lengthens `t0_to_vault_usable_ms`, not the
paint marks.

## Launching

Two modes, and the choice matters.

**Mode A — verdict.** The process exits by itself and leaves a trace file.

```
SERAT_STARTUP_TRACE=<file> \
SERAT_STARTUP_T0_UNIX_NANOS=<ns since epoch> \
target/release/serat.exe --vault <dir> --open <vault-relative.md> \
                         --exit-after-ready --ready-timeout-ms 15000
```

- `--open` is not optional in practice. Without it the shell falls back to the settings
  TOML in the platform config dir and restores whatever the last run opened, so two runs
  are not comparable.
- `SERAT_STARTUP_T0_UNIX_NANOS` is the harness's spawn instant. Omit it and the trace
  records `t0_source: in_process`, which undercounts by whatever the loader cost.
- `--ready-timeout-ms` bounds a hang. 15 s is ample; the default is 60 s.
- **Exit 0** = trace written. **Exit 2** = failed, with a named reason on stderr. There
  is no third outcome and no silent success.

**Mode B — inspection.** Drop `--exit-after-ready`. The window stays open and the
status bar carries the front end's own report: `painted <n> ms` on success, or
`paint mark failed: <error>` when the guard refused (`store.setMark`, `app/src/startup.ts`).
That string is the fastest way to tell *the guard fired* from *the call never happened*,
and it needs no debugger. Mode A cannot show it — the process is gone.

**Attaching a debugger (Mode B only).** F-393 and F-423 both read in-page state over the
Chrome DevTools Protocol by launching with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>` and evaluating in
the live page. **I have not run this myself** — it is recorded here because it is the
technique those two findings used and its author offered it forward; treat the exact
invocation as theirs, not as verified by me.

## Reading the result

The trace is one JSON line. Three fields matter here:

| Field | Meaning |
|---|---|
| `t0_to_window_visible_ms` | the mark itself, on **this process's** clock, taken when the call arrived |
| `paint_evidence_ms` | the raw `first-contentful-paint` `startTime` the webview sent |
| `raf2_ms` | the second `requestAnimationFrame` timestamp sent alongside it |

**`paint_evidence_ms` and `raf2_ms` are on the page's navigation clock, not on `t0`.**
They are kept for audit and are never compared against any other mark. Do not read
`t0_to_window_visible_ms - paint_evidence_ms` as anything.

### What a clean run looks like

From **48 traces** taken on this machine during task #119, spanning cold and warm vaults:

| | observed |
|---|---|
| `paint_evidence_ms` | 144.0 – 392.0 |
| `raf2_ms` | 100.1 – 313.8 |
| `paint_evidence_ms - raf2_ms` | 35.2 – 79.7, median **50.2** |

The gap band is the useful one, and it matches F-406's independently measured 51.4 /
56.7 / 63.4 ms. In WebView2 the second rAF callback consistently *precedes* the paint
entry by ~50 ms: rAF fires on the renderer's schedule and Paint Timing measures the
compositor's.

`frame_precedes_paint_diagnostic`'s threshold is **200 ms**, so with a maximum observed
gap of 79.7 ms **it did not fire in any of the 48 runs**. Its absence from stderr is the
expected state, not a signal.

### Known noise

Every one of the 48 runs printed exactly this on stderr, at teardown, and it means
nothing:

```
[ERROR:ui\gfx\win\window_impl.cc:172] Failed to unregister class Chrome_WidgetWin_0. Error = 1412
```

Filter it out (`grep -v window_impl.cc`) before reading a run's output, or it will be
the only line you see.

---

## What I expect to be hard

The one section that is judgement rather than mechanics, offered because I have run
this shell against a real window more than anyone else on the team.

1. **The build pipeline, and it is not close.** See the top of this document. A frontend
   mutation without `npm run build && cargo build --release` produces a clean green run
   with real-looking paint evidence. If a run comes back clean, the first question is
   not "is the guard sound" but "did my mutation reach the binary" — and the positive
   control answers it in one rebuild.

2. **Exit code alone cannot see a mutation that *adds* a call.** `mark_first_paint`
   returns `Ok(false)` on a repeat, so a fabricated call that arrives *first* takes the
   mark and the legitimate call is silently ignored. The run then exits 0 with a
   complete trace. What distinguishes it is `paint_evidence_ms`: a fabricated constant
   sits outside the 144–392 ms band above, while `0` or a negative would be rejected
   outright. **Read the evidence value, not just the exit code.** F-397's mutation was
   exactly this shape (`mark_first_paint(1.0)`), and F-392's was the front-end analogue
   (a second `invoke` hoisted to script top level).

3. **It needs a real interactive desktop session.** WebView2 must be present and a
   window must actually be creatable. Over SSH, in a service context, or on a machine
   with no logged-in session, expect a hang bounded only by `--ready-timeout-ms` rather
   than a clean failure.

4. **Timing-sensitive mutations may not reproduce on one run.** The band above is wide
   (paint evidence varies by 2.7× across runs on one machine) and the first run after a
   rebuild is consistently the slowest. Take several runs before concluding anything
   from a single number; the shape of the failure, not its magnitude, is the signal.

5. **Do not add `--bench-save-list` to a paint-guard run.** It implies
   `--exit-after-ready` and adds a save workload that has nothing to do with this guard.
   It also releases on `wait_for_usable`, whose binding term is `buffer_rendered` — the
   thing F-461 was about.
