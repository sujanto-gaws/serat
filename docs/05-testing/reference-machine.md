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

**1. Defender real-time protection is off.** Every file this machine writes and reads skips the on-access scanner. A full rebuild of 20,000 notes reads 20,000 files, and on a stock Windows machine with Defender enabled that is materially slower — this is the single largest environmental difference between RM-1 and a default Windows installation, and between RM-1 and a GitHub-hosted Windows runner (where Defender *is* enabled). **Any NFR-PRF-04 number taken on RM-1 is a best case for Windows.** The report for T2 must say so, and the CI budget job (B2) must carry a separate, looser CI threshold or an explicit exclusion, decided when the first CI numbers land — not by weakening the product budget.

**2. The active power plan is "Energy Star", not "High performance".** Boost behaviour and therefore single-thread throughput vary with it, and on a laptop they also vary with thermals. Cold-start (NFR-PRF-01) is largely single-threaded and is the measurement most exposed to this.

**Benchmark protocol on RM-1**, to keep runs comparable:

- Mains power connected, lid open, no other build running.
- The same power plan for every run in a series; the plan in force is recorded in the report.
- Discard the first run of a series (page cache is cold in a way the budget does not describe — NFR-PRF-01 specifies a *warm* index) and report p95 over the remaining runs.
- Record the vault's `tree_digest_sha256` from its manifest in every report, so the number and the vault it was taken against travel together.

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

If this drifts by more than ~30% the machine, its power plan or its scanner configuration has changed, and previously recorded budget numbers are no longer directly comparable.

## Machines that are not the reference machine

CI runners are not RM-1 and never will be. NFR-PRF-08 requires every budget to be asserted in CI and a regression to fail the build; that is a *regression gate*, and its thresholds are a property of the runner. The product budgets in NFR-PRF-01/03/04 are the numbers RM-1 must meet. Both are real, they are not the same number, and B2 keeps them in separate columns.

## Traceability

| ID | How this file serves it |
|---|---|
| NFR-PRF-01 | Names the "reference machine" the acceptance criterion refers to. |
| NFR-PRF-03 | Same, for save→index p95. |
| NFR-PRF-04 | Same, for full rebuild wall clock; records the Defender caveat that bounds it. |
| NFR-PRF-08 | Separates product budgets on RM-1 from regression thresholds in CI. |
| US-35 | Makes "stay fast as the vault grows" measurable against a fixed baseline. |
| C-06 | Budgets are enforced against the synthetic vault *on a named machine*. |
