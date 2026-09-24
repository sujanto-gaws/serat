# bench/editor-latency

The NFR-PRF-02 budget harness: keystroke to paint shall not exceed 16ms at p95 in a 5,000-line note. Task #11 (P1/test). Owned by test-engineer.

## Status — scaffolding only, not wired into CI

This harness exists ahead of its design on purpose (coordinator instruction, 2026-09-24): the measurement technique and the statistic don't depend on the P1 editor-integration design (architect task #5), so there was no reason to wait to write them. What *is* blocked on #5 and its implementation (#21, vue-frontend-dev) is the "subject" — how to reach a running Serat window from a test driver and how to open a specific note deterministically. `tests/subject.ts` names exactly that gap and throws loudly with the blocking task numbers rather than silently doing nothing, the same convention `bench/budget/src/subject.rs` uses for NFR-PRF-01/03/04.

**Green today:** `tests/measure.unit.spec.ts` — the p95 statistic, matching `bench/budget`'s TC-027 fixture shape (one slow run in twenty passes, two do not).

**Red today, on purpose:** `tests/nfr-prf-02.spec.ts` — fails at `openNoteForLatencyTest`, not at a timeout, so a CI run failing here says why rather than making someone rediscover it.

## Run it

```
npm install
npx playwright install-deps chromium   # if system deps are missing; the browser itself is already at /opt/pw-browsers/chromium
npm test
```

Chromium is not downloaded by Playwright's own installer here — `playwright.config.ts` points `executablePath` at the pre-provisioned `/opt/pw-browsers/chromium`, overridable with `PW_CHROMIUM_PATH`.

## What lands this for real

1. Architect task #5 says how the app under test is reached (dev-server stand-in for the Tauri webview, or CDP attach to a real window) and how a note is opened deterministically.
2. vue-frontend-dev task #21 lands the hybrid live-preview editor `tests/subject.ts` needs a locator for.
3. A 5,000-line fixture note, generated deterministically (NFR-MNT-02) rather than hand-authored, ideally beside `bench/vaultgen`'s existing generator rather than duplicating it.
4. Once 1–3 land, `subject.ts`'s `openNoteForLatencyTest` gets a real body, the spec goes from throwing to asserting, and this harness is wired into `.github/workflows/budgets.yml` beside the Rust jobs — following NFR-PRF-08 and RISK-013's rule that a CI job is only claimed once it can actually run, not once it is written.

## Serves

NFR-PRF-02 · NFR-PRF-08 (once wired) · US-35 · C-06.
