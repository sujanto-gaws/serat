// Jump-to-location (search-panel.md, backlinks-pane.md; architecture.md §4.5):
// a `BlockLocation` is a byte range as indexed, plus the hash it was indexed
// from. Opening a result maps it into the editor's document. If the file's hash
// is still the indexed one the byte range is exact; if the note has changed
// since, the block is found by `ord` and the answer is marked approximate,
// "never a wrong line presented as exact".

import type { BlockLocation } from "@/ipc/generated/BlockLocation";

/** UTF-16 index in `text` of the UTF-8 byte offset `byte` (clamped to the text). */
export function byteToUtf16Index(text: string, byte: number): number {
  let bytes = 0;
  let index = 0;
  while (index < text.length && bytes < byte) {
    const code = text.codePointAt(index) as number;
    bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    index += code >= 0x10000 ? 2 : 1;
  }
  return index;
}

/** CodeMirror counts a CRLF as one offset; raw text counts two. */
export function rawIndexToDocOffset(text: string, index: number): number {
  let crlf = 0;
  for (let i = text.indexOf("\r\n"); i >= 0 && i + 1 < index; i = text.indexOf("\r\n", i + 2)) crlf += 1;
  return index - crlf;
}

/** Start offsets (raw UTF-16 indices) of blank-line-separated blocks. */
function blockStarts(text: string): number[] {
  const starts: number[] = [];
  let blank = true;
  let i = 0;
  while (i <= text.length) {
    const lineEnd = text.indexOf("\n", i);
    const end = lineEnd < 0 ? text.length : lineEnd;
    const isBlank = text.slice(i, end).trim() === "";
    if (!isBlank && blank) starts.push(i);
    blank = isBlank;
    if (lineEnd < 0) break;
    i = lineEnd + 1;
  }
  return starts;
}

export interface BlockPosition {
  /** Offset into the editor document (CodeMirror's model). */
  offset: number;
  /** True when located by `ord` because the note changed since it was indexed. */
  approximate: boolean;
}

/**
 * Where to put the cursor for `block` in `text` (as `read_file` returned it,
 * hash `currentHash`).
 */
export function locateBlock(text: string, currentHash: string | null, block: BlockLocation): BlockPosition {
  if (currentHash !== null && currentHash === block.note_hash) {
    const raw = byteToUtf16Index(text, block.byte_start);
    return { offset: rawIndexToDocOffset(text, raw), approximate: false };
  }
  const starts = blockStarts(text);
  const raw = starts.length === 0 ? 0 : starts[Math.min(block.ord, starts.length - 1)];
  return { offset: rawIndexToDocOffset(text, raw), approximate: true };
}
