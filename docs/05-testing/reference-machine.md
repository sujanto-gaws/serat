---
project: Serat
artifact: Reference machine for the performance budgets
phase: 05-testing
owner: test-engineer
serves: NFR-PRF-01, NFR-PRF-03, NFR-PRF-04, NFR-PRF-08, US-35, C-06
---

# The Reference Machine

NFR-PRF-01 says the cold-start budget is "measured on the reference machine over 20 runs; p95 within budget". Until this document existed there was no reference machine, which made every P0 number unattributable: 30 seconds for a full rebuild means one thing on a laptop with a slow disk and something else entirely on a workstation. This file names the machine, so that a P0 measurement and a P5 measurement can be compared, and so a number recorded in `reports/` can be reproduced.

**Ownership:** test-engineer. **Rule:** if the reference machine changes, every recorded number is re-taken before it is compared with a new one. A budget is never re-baselined against a different machine without saying so in the report.

## RM-1 — the P0 reference machine

Measured 2026-09-03, the machine this repository is developed on.

| Property | Value |
|---|---|
| Model | Lenovo 82B1 (Legion 5 15ARH05), laptop |
| CPU | AMD Ryzen 7 4800H with Radeon Graphics |
| Cores / threads | 8 physical, 16 logical |
| Base clock | 2.90 GHz (boost governed by the active power plan — see below) |
| RAM | 31.4 GiB |
| Page file | 28 GiB, system-managed |
| Storage, repository | WD_BLACK SN770 1 TB, NVMe SSD (drive `D:`, NTFS, 931.5 GB, 138.5 GB free) |
| Storage, system | Samsung SSD 970 EVO Plus 1 TB, NVMe SSD (drive `C:`, NTFS) |
| OS | Windows 11 Home Single Language, 10.0.26200 (build 26200) |
| Power source | AC (mains), battery present |
| Active power plan | **"Energy Star"** (`fe798efb-b6ce-42b1-ba46-3235b1658649`) |
| Windows Defender real-time protection | **Disabled** |
| Toolchain | cargo 1.92.0, rustc 1.92.0, node 24.15.0, sqlite3 3.42.0 |
| Repository path | `D:\Projects\gaws\AIResearch` (on the SN770) |

### The two facts that will bite

Both are recorded here rather than quietly enjoyed, because a number is only useful with its conditions attached.

**1. Defender real-time protection is off.** Every file this machine writes and reads skips the on-access scanner. A full rebuild of 20,000 notes reads 20,000 files, and on a stock Windows machine with Defender enabled that is materially slower — this is the single largest environmental difference between RM-1 and a default Windows installation, and, like RM-1, a GitHub-hosted Windows runner (all 36 runs on record report the scanner OFF; this sentence said Defender was enabled there until task #166). **Any NFR-PRF-04 number taken on RM-1 with the scanner off is a best case for Windows.**

**2. The active power plan is "Energy Star", not "High performance".** Boost behaviour and therefore single-thread throughput vary with it, and on a laptop they also vary with thermals. Cold-start (NFR-PRF-01) is largely single-threaded and is the measurement most exposed to this.

## The gate number is the scanner-enabled number

Instruction from the lead, task B2, and it is a rule rather than a caveat:

- **Every filesystem-bound budget is measured on RM-1 both ways** — real-time protection off (the comparable best case, stable across runs) and on (what a user actually gets).
- **The scanner-enabled figure is the one the charter's "< 30 s" gate is judged against.** A number taken with the scanner off is a diagnostic, not a verdict.
- **Real-time protection is never disabled to make a budget pass.** If the honest number breaches 30 s, the P0 gate is NO-GO and the architecture changes — architecture §1.2 says P0 may invalidate the parse-pool assumption, and I/O rather than parsing turning out to be the bottleneck is exactly such a result, not an environment footnote. Carried in the risk register.

This does not depend on anyone remembering it. The harness detects the scanner state itself, stamps it on every result, and marks a scanner-disabled run `"gate_eligible": false` with a `NOT A GATE NUMBER` line in its report — so a best-case figure cannot be filed as the gate figure by omission. The harness never changes the scanner state: toggling real-time protection is an administrator action with consequences outside this repository, and a benchmark tool that disables your antivirus to look good is not a thing this project should contain. Toggling is a manual step in the protocol below.

**Benchmark protocol on RM-1**, to keep runs comparable:

1. Mains power connected, lid open, no other build running.
2. Record the environment before the series and after it:
   ```
   cargo run --release -p budget --bin environment
   ```
   It prints the scanner state, the power plan and the core count, and exits non-zero when the environment cannot produce a gate number.
3. Run the series with real-time protection **enabled** — this is the gate number.
4. Disable real-time protection, re-run, and record that figure beside it as the comparable best case. Re-enable it afterwards.
5. The same power plan for every run in a series; the plan in force is recorded automatically in each result and quoted in the report.
6. Discard the first run of a series (page cache is cold in a way the budget does not describe — NFR-PRF-01 specifies a *warm* index) and report p95 over the remaining runs.
7. Record the vault's `tree_digest_sha256` in every report, so the number and the vault it was taken against travel together. The harness does this for you and refuses to run against a vault that disagrees with its manifest.

A gate report therefore carries two rows per filesystem-bound budget, and says which is which:

| Budget | Scanner on (gate) | Scanner off (best case) | Power plan |
|---|---|---|---|
| NFR-PRF-04 full rebuild | *pending I4* | *pending I4* | *recorded per run* |

### Machine fingerprint

A coarse check that RM-1 is still RM-1, cheap enough to run before a benchmark series. Generating the 20k vault is CPU-bound on content generation and IO-bound on 20,205 file creations, which is the same mix the rebuild budget cares about.

```
cargo build --release -p vaultgen
target/release/vaultgen --preset 20k --force
```

| Measurement | RM-1, 2026-09-03 |
|---|---|
| vaultgen 20k wall clock | 4.71 s, 5.19 s, 5.46 s (three consecutive runs) |
| vaultgen 50k wall clock | 6.84 s |
| Vault size, 20k | 39.4 MiB across 20,205 files |
| Environment at the time | `windows · 16 cores · realtime scanner OFF · power plan Energy Star` |

Those generation figures were taken with the scanner **off**, and are recorded as such. They are a machine fingerprint, not a budget, so they are not subject to the gate rule above — but they will move when the scanner is on, and a fingerprint compared across different scanner states means nothing.

If this drifts by more than ~30% the machine, its power plan or its scanner configuration has changed, and previously recorded budget numbers are no longer directly comparable.

## No cold-start figure on this machine is a cold-*vault* figure

A bound on what P0 can establish, not a footnote, and it belongs beside `gate_eligible: false` in every cold-start report.

Every cold-start measurement spawns the application repeatedly against one vault. After the first iteration the Windows page cache holds it, so the walk is CPU-bound rather than I/O-bound for every sample that follows. **A user's cold start reads a cold vault.** The figure NFR-PRF-01 is asserted against is therefore a warm-vault figure, and the gap between it and the thing the requirement describes is unmeasured.

It is unmeasured because it could not be measured here, and the attempt is worth recording rather than the conclusion alone. A freshly generated vault is *write*-cached rather than cold: measuring against fresh 20k vaults gave +42.9 ms (n = 12 per arm) against a **null control of +15.9 ms with the same binary on both arms** — the control was run precisely because the first result looked too clean, and most of the apparent effect was the harness measuring itself. Dropping the Windows page cache outright needs privileges the measuring account does not have.

**What would fix it**, in order of preference:

1. Drop the standby list between spawns (`RAMMap -Et`, or an equivalent that can be scripted) and re-take the series. If RM-1 can be given that privilege, this is a materially better measurement than anything taken so far and it is worth the setup.
2. Failing that, measure the *first* spawn against a freshly written vault and report it separately with its own null control, so the number is at least labelled as what it is.
3. Failing both, the gate report says the budget was validated against a warm vault and that the cold-vault case is untested. That is the current state.

The same caveat applies to the proportionality comparison, and the scale-check report line carries it.

## RM-2 — gaws-dev-05, a Linux desktop (2026-11-24)

The repository owner ran the budget bins with `--profile reference` on a Linux desktop named gaws-dev-05 on 2026-11-24 and committed the raw JSON (`docs/05-testing/reports/reference-runs/gaws-dev-05-2026-11/`, commit `4244c14`). It is a second named machine, not a replacement for RM-1. The analysis is `docs/05-testing/reports/reference-run-gaws-dev-05-2026-11.md`.

**The rule.** A budget is never compared across machines. RM-2's figures are RM-2's: they are not compared with RM-1's, with the Linux container's, or with a hosted runner's, and none of them re-baselines a number taken elsewhere. When the harness's own rule or a report sets two figures side by side, the report says they come from different machines and that the comparison is context, not a verdict.

The table is filled only from what the five result files record. The `environment` bin was not run, before or after the series, so every property the result files do not carry is **owed (environment bin not run)**.

| Property | Value |
|---|---|
| Name | gaws-dev-05 |
| OS | linux (recorded in every result file) |
| Cores | 4 (recorded in every result file) |
| Real-time scanner | `null` (not applicable to Linux; the harness records no scanner state) |
| Power plan | `unknown` (as recorded; no plan is read on Linux) |
| Vault | 20,000 notes, format version 1, `vault_digest_sha256` `35fcb867759df4b8737af8914f798d81d77396dfe051f38140139d3779bd393e` (identical in all five files) |
| `gate_eligible` | true in all five files (`Environment::gate_eligible()` is true on Linux) |
| Toolchain | owed (environment bin not run); the result files carry no toolchain field |
| Model, CPU, clock | owed (environment bin not run) |
| RAM | owed (environment bin not run) |
| Storage and filesystem | owed (environment bin not run) |
| Distribution and kernel | owed (environment bin not run) |
| Webview (webkit2gtk) version | owed (environment bin not run); matters for NFR-PRF-01 |
| Build profile of the app under test | release (enforced by `trace.rs`; NFR-PRF-01 only) |
| Concurrent load before and after (`attested`) | owed (environment bin not run); the run is therefore not attested in the sense `p1-gate-report.md` uses |
| Series order and first-sample handling | owed; the files give samples in run order and do not say whether a warm-up run was discarded |

**What gate-eligible means here.** Under ADR-0019 and the harness rule, a Linux run is gate eligible by construction, so RM-2's figures are asserting figures for the Linux reference: a breach prints FAIL and a pass prints PASS, and the harness says `asserted: true` where the budget is asserted. The product ships on Windows. ADR-0019 §8 says a figure from a machine other than RM-1 is supporting evidence and not the gate figure, so **the RM-1 scanner-on figures remain owed** for NFR-PRF-01, NFR-PRF-03, NFR-PRF-04 and FR-VLT-04, and #167 (the Windows re-runs) stays open. "Gate eligible" is a statement about the scanner question the harness was written for. It is not a statement that the machine is the reference machine of the charter's gate.

**Not run on RM-2:** `environment` (before and after), `p1-external-change` (FR-VLT-04), `p1-save-tail`.

## Machines that are not the reference machine

CI runners are not RM-1 and never will be. NFR-PRF-08 requires every budget to be asserted in CI and a regression to fail the build; that is a *regression gate*, and its thresholds are a property of the runner. The product budgets in NFR-PRF-01/03/04 are the numbers RM-1 must meet. Both are real, they are not the same number, and B2 keeps them in separate columns.

A GitHub-hosted Windows runner does **not** have real-time protection enabled: all 36 `budgets.yml` runs on record (279-316) report `realtime scanner OFF` on 2 cores, so a hosted number is never gate eligible in the sense this document means (this paragraph said the opposite until task #166 read the logs). Where a budget has no regression ceiling it is report-only on that runner and asserted here, on the reference machine; see `nfr-prf-04-gate-quality.md`, "Task #166".

## Traceability

| ID | How this file serves it |
|---|---|
| NFR-PRF-01 | Names the "reference machine" the acceptance criterion refers to, and records that its cold-start figures are warm-vault figures. |
| NFR-PRF-03 | Same, for save→index p95. |
| NFR-PRF-04 | Same, for full rebuild wall clock; sets the rule that the gate figure is the scanner-enabled one. |
| NFR-PRF-08 | Separates product budgets on RM-1 from regression thresholds in CI. |
| US-35 | Makes "stay fast as the vault grows" measurable against a fixed baseline. |
| C-06 | Budgets are enforced against the synthetic vault *on a named machine*. |
| ADR-0019 | RM-2 is a gate-eligible Linux machine whose figures are supporting evidence for the gate, never the RM-1 gate figure. |
