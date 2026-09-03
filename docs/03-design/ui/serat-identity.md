---
project: Serat
artifact: Brand & product identity
phase: 03-design/ui
source: https://claude.ai/code/artifact/b6a46b84-e8ee-438d-bb28-68c6dd2a5726
source_html: docs/_source/serat-identity.html
imported: 2026-09-02
---

*(embedded image)*

Identity exploration · three directions · August 2026

# Serat Identity

Three marks built from the two things the name already means — a written work, and a fibre. Each is drawn as flat geometry so it survives at 16 pixels and needs no gradient, no bevel, and no second version for dark mode.

## The brief the mark has to satisfy

An app icon has a harder job than a logo. It lives at 16px in a browser tab, 32px in a taskbar, and 512px in a store listing, and it has to be findable in a dock next to thirty other squares. That rules out most of what makes a pretty logo.

Legible at 16px  
One dominant shape, high contrast, no detail that dissolves

Not Obsidian  
**No violet gem.** Indigo reads as navy, not purple

Not a brain-graph  
Nodes-and-edges is the most tired cliché in this category

Both meanings  
Manuscript *and* thread — the name's own double sense

One drawing  
Flat fills only, so it inverts cleanly instead of needing a variant

## Three directions

*(embedded image)*

Direction A

### Lontar — the bound manuscript

Javanese *serat* were written on lontar: strips of palm leaf, stacked and held by a single cord threaded through them. That is the literal object the name refers to, drawn literally — three leaves, one thread, a loose end at the bottom so the cord reads as fibre rather than a rule.

It carries a second reading for anyone who has never heard of a lontar: three lines of text with something running through them. A notes app whose icon looks like lines of text is not a bad accident.

**Risk:** at a glance it can read as a list or hamburger icon. The staggered widths and the thread are what break that — which is why the strips must never be equal length.

*(embedded image)*

App icon

*(embedded image)*

Ink variant

*(embedded image)*

32px

*(embedded image)*

16px opt.

*(embedded image)*

Mono

*(embedded image)*serat

Lockup

*(embedded image)*

Direction B

### Tenun — the plain weave

A true plain weave: three strands each way, with the over-under alternation actually drawn rather than implied. It is the other meaning of *serat*, and it is the closer fit to what the product does — individual threads becoming a structure, which is the whole argument of the blueprint.

The two-tone interlace is what makes it work. A single-colour weave collapses into a solid block; warp in ink and weft in brass keeps every crossing legible, and gives the icon a texture no competitor in this category has.

**Risk:** the densest of the three. It holds its silhouette when blurred, but individual strands merge below about 20px — it wants a two-by-two reduction for the favicon.

*(embedded image)*

App icon

*(embedded image)*

Ink variant

*(embedded image)*

32px

*(embedded image)*

16px

*(embedded image)*

Mono

*(embedded image)*serat

Lockup

*(embedded image)*

Direction C

### Simpul — one thread, one node

A monogram, but drawn as a single unbroken fibre rather than a letterform: one stroke, round-capped at both ends so it reads as cut thread, with one bead sitting on it where the curve crosses its own centre. The bead is the note on the thread.

This is the flexible one. It scales further down than the other two, works as a loading spinner and a cursor badge, and stamps cleanly on anything. It is also the least specific — an S with a dot could belong to a dozen products.

**Risk:** safe to the point of forgettable. Choose it if you expect the name to do the identifying work and want the mark to stay out of the way.

*(embedded image)*

App icon

*(embedded image)*

Ink variant

*(embedded image)*

32px

*(embedded image)*

16px

*(embedded image)*

Mono

*(embedded image)*serat

Lockup

## Which one

### A. Lontar recommended

The only one of the three that could not belong to another product. It is specific to the name, it has the cleanest small-size silhouette, and its accidental second reading — lines of text — is exactly right for what the app is. Ship the simplified two-strip version below 20px.

### B. Tenun strong alternative

The better mark if you want the icon to argue for the *structure* story rather than the manuscript one. Most distinctive texture of the three and the most confident as a large icon; needs a reduced version for the favicon.

### C. Simpul fallback

Pick this only if A and B both feel too illustrative. It will never be wrong and it will never be memorable. Its better job is as a secondary device — spinner, badge, watermark — under whichever mark wins.

On the indigo

Obsidian's icon is a saturated violet gem. The brand indigo here is `#24457E` — a dark navy-leaning blue drawn from batik indigo dye, roughly 30 points darker and considerably less magenta. Side by side in a dock they do not read as related, which is deliberate: the whole positioning is "not another purple PKM app".

## Palette and specs

| Token  | Hex      | Role                                                                                |
|--------|----------|-------------------------------------------------------------------------------------|
| Indigo | \#24457E | Icon ground, primary brand colour, accents in the UI                                |
| Brass  | \#C08A47 | The thread, the weft, the bead — always the secondary element, never the whole mark |
| Ink    | \#16191B | The mark on light grounds; alternate icon ground                                    |
| Paper  | \#F3F4F1 | Light ground; the mark on indigo and ink grounds                                    |

#### Rules

- **Clear space** equal to the height of one strip (A), one strand (B), or the stroke width (C) on all four sides.
- **Minimum size** 16px with the optimised variant, 24px with the full drawing. Below that, use the wordmark alone.
- **Icon padding** is 62% mark to 38% ground on a squircle — the macOS convention. A full-bleed square is fine for Windows; keep the same optical size.
- **Wordmark** is lowercase, Bricolage Grotesque 700 at −0.035em tracking. Lowercase reads as a personal tool; uppercase reads as enterprise software, which this is not.
- **Never** add a gradient, an outline, or a drop shadow; never recolour the accent to anything but brass; never place the mark on a photograph.

All three marks are flat SVG on a 64-unit grid with no gradients, strokes converted at export, and a single accent colour driven by one variable — so the same file serves the app icon, the favicon, the About panel and the installer. Companion to the Serat product blueprint.
