---
type: note
---

# Alias Link

Links to its target by alias, not by real name or path: [[Ali]] and
[[Target Alias]].

Both stay unresolved by design: `aliases` are quick-switcher candidates only
(architecture §9.2), and the link resolver matches a path or a file name
(`crates/serat-index/src/resolve.rs`).

It also links by file name, [[aliases-target]], which does resolve.
