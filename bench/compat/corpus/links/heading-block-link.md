---
type: note
---

# Heading Block Link

Links into specific parts of another note:

- [[heading-and-block-target#Some Heading]] — subpath onto a heading.
- [[heading-and-block-target#^block1]] — subpath onto a block anchor
  (Obsidian writes `#^id`; there is no bare `^id` form).
- [[heading-and-block-target#Some Heading#Another Heading]] — the nested form targets
  the second heading under the first (architecture §9.2's subpath rule). Here the two
  headings are siblings, so the case pins what the *index* does with the
  nesting: it records the note part and resolves the note; whether the second heading is found
  under the first is the heading lookup's question (`subpath_found`), not the
  index's.
- [[heading-and-block-target]] — the bare link, the control for the three above.
