# Icons — provisional

`icon.ico` is a placeholder: a 32×32 flat mark in the brand palette (indigo ground,
one brass thread, a paper bead), drawn to the rules in
`docs/03-design/ui/serat-identity.md` §Palette but **not** to any of the three
directions the identity document actually proposes.

It exists because `tauri-build` cannot produce a Windows binary without an `.ico`, so
without it the crate does not compile on Windows at all and nobody can measure
NFR-PRF-01.

Owners: frontend-designer chooses and exports the mark; release-engineer owns the icon
set, the signed manifest and the installer (docs/06-release, architecture §10.1).
Replacing this file is expected and needs no coordination with tauri-app-dev.
