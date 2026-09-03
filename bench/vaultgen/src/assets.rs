//! Binary asset bytes.
//!
//! P0 does not extract anything — extraction is P2 (ADR-0004, NFR-PRF-09) — so
//! the bar here is only that a walker sees a real file of a real type. Four of
//! the five formats clear a higher bar anyway, because it costs little and it
//! makes the same vault usable for P2:
//!
//! | Format | What is generated                                                     |
//! |--------|------------------------------------------------------------------------|
//! | PDF    | Structurally valid one-page PDF with a real xref table and extractable text |
//! | PNG    | Structurally valid 8-bit RGB PNG (zlib stored blocks, real CRCs)       |
//! | DOCX   | Structurally valid OOXML package: `[Content_Types].xml`, `_rels/.rels`, `word/document.xml` |
//! | XLSX   | Structurally valid OOXML package with one worksheet of numbers        |
//! | JPG    | **Placeholder.** Correct SOI/JFIF/EOI framing, no image data — a real baseline encoder is not worth writing for a file nothing decodes in P0. |
//!
//! Every text-bearing asset contains a unique marker, `serat-asset-NNNN-marker`,
//! so a P2 test can assert that a phrase inside a binary is findable from the
//! note search box without having to know what else the file says.

use crate::hash::{adler32, crc32};
use crate::plan::{AssetFormat, AssetMeta, TAG_ASSET};
use crate::rng::{Rng, stream_seed};
use crate::words::{DOMAIN_NOUNS, MODIFIERS, SENTENCE_MIDDLES, SENTENCE_OPENERS};

/// Render one asset's bytes.
pub fn render(seed: u64, asset: &AssetMeta) -> Vec<u8> {
    let mut r = Rng::new(stream_seed(
        seed,
        TAG_ASSET,
        500_000 + u64::from(asset.index),
    ));
    match asset.format {
        AssetFormat::Pdf => pdf(asset, &mut r),
        AssetFormat::Png => png(asset, &mut r),
        AssetFormat::Docx => docx(asset, &mut r),
        AssetFormat::Xlsx => xlsx(asset, &mut r),
        AssetFormat::Jpg => jpg(asset, &mut r),
    }
}

/// The token a P2 extraction test can search for.
pub fn marker(index: u32) -> String {
    format!("serat-asset-{index:04}-marker")
}

fn lines(asset: &AssetMeta, r: &mut Rng, count: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(count + 2);
    out.push(asset.title.clone());
    out.push(marker(asset.index));
    for _ in 0..count {
        out.push(format!(
            "{} {} {} {}.",
            r.pick(SENTENCE_OPENERS),
            r.pick(MODIFIERS),
            r.pick(DOMAIN_NOUNS),
            r.pick(SENTENCE_MIDDLES)
        ));
    }
    out
}

// ---------------------------------------------------------------------- PDF

fn pdf_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            c if c.is_ascii() && !c.is_ascii_control() => out.push(c),
            // PDF literal strings here are ASCII only; anything else would need
            // a font encoding this file does not carry.
            _ => out.push('?'),
        }
    }
    out
}

fn pdf(asset: &AssetMeta, r: &mut Rng) -> Vec<u8> {
    let count = r.range(6, 40) as usize;
    let body_lines = lines(asset, r, count);

    let mut content = String::from("BT\n/F1 11 Tf\n72 720 Td\n14 TL\n");
    for line in &body_lines {
        content.push('(');
        content.push_str(&pdf_escape(line));
        content.push_str(") Tj T*\n");
    }
    content.push_str("ET\n");

    let objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_string(),
        format!(
            "<< /Length {} >>\nstream\n{}endstream",
            content.len(),
            content
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];

    let mut out: Vec<u8> = Vec::with_capacity(4096);
    out.extend_from_slice(b"%PDF-1.7\n");
    // The binary comment that tells transfer agents this is not a text file.
    out.extend_from_slice(&[b'%', 0xE2, 0xE3, 0xCF, 0xD3, b'\n']);

    let mut offsets: Vec<usize> = Vec::with_capacity(objects.len());
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(obj.as_bytes());
        out.extend_from_slice(b"\nendobj\n");
    }

    let startxref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    // Each entry is exactly 20 bytes, which is what makes the table parseable.
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
            objects.len() + 1,
            startxref
        )
        .as_bytes(),
    );
    out
}

// ---------------------------------------------------------------------- PNG

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Wrap raw bytes in a zlib stream of *stored* deflate blocks. No compression,
/// but a valid stream: writing a deflate encoder to make a placeholder image
/// smaller would be effort spent in the wrong place.
fn zlib_stored(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() + 64);
    out.push(0x78); // CMF: deflate, 32K window
    out.push(0x01); // FLG: no dictionary, fastest — (0x78 << 8 | 0x01) % 31 == 0
    let mut i = 0usize;
    if raw.is_empty() {
        out.extend_from_slice(&[0x01, 0x00, 0x00, 0xFF, 0xFF]);
    }
    while i < raw.len() {
        let n = (raw.len() - i).min(65_535);
        let final_block = i + n >= raw.len();
        out.push(u8::from(final_block));
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&(!(n as u16)).to_le_bytes());
        out.extend_from_slice(&raw[i..i + n]);
        i += n;
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
    out
}

fn png(asset: &AssetMeta, r: &mut Rng) -> Vec<u8> {
    let w = 32 + 16 * r.below(6) as u32; // 32..=112
    let h = 32 + 16 * r.below(6) as u32;
    let base = (r.below(256) as u8, r.below(256) as u8, r.below(256) as u8);

    // Filter byte 0 (None) per scanline, then RGB triples.
    let mut raw = Vec::with_capacity((h * (1 + w * 3)) as usize);
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            raw.push(base.0.wrapping_add((x * 3) as u8));
            raw.push(base.1.wrapping_add((y * 3) as u8));
            raw.push(base.2);
        }
    }

    let mut out = Vec::with_capacity(raw.len() + 128);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour, no interlace
    png_chunk(&mut out, b"IHDR", &ihdr);

    // A text chunk so the marker is findable in a PNG too.
    let mut text = Vec::new();
    text.extend_from_slice(b"Description\0");
    text.extend_from_slice(marker(asset.index).as_bytes());
    png_chunk(&mut out, b"tEXt", &text);

    png_chunk(&mut out, b"IDAT", &zlib_stored(&raw));
    png_chunk(&mut out, b"IEND", &[]);
    out
}

// --------------------------------------------------------------------- ZIP

struct ZipEntry {
    name: &'static str,
    data: Vec<u8>,
}

/// A ZIP of stored (uncompressed) entries with real CRCs, which is all an OOXML
/// package needs to be readable. Timestamps are fixed at 1980-01-01, the ZIP
/// epoch, because a real timestamp would break byte-identical output.
fn zip(entries: &[ZipEntry]) -> Vec<u8> {
    const DOS_TIME: u16 = 0;
    const DOS_DATE: u16 = 0x0021; // 1980-01-01
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();

    for e in entries {
        let offset = out.len() as u32;
        let crc = crc32(&e.data);
        let size = e.data.len() as u32;

        out.extend_from_slice(b"PK\x03\x04");
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&DOS_TIME.to_le_bytes());
        out.extend_from_slice(&DOS_DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra len
        out.extend_from_slice(e.name.as_bytes());
        out.extend_from_slice(&e.data);

        central.extend_from_slice(b"PK\x01\x02");
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&DOS_TIME.to_le_bytes());
        central.extend_from_slice(&DOS_DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(e.name.as_bytes());
    }

    let cd_offset = out.len() as u32;
    let cd_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn docx(asset: &AssetMeta, r: &mut Rng) -> Vec<u8> {
    let count = r.range(5, 30) as usize;
    let body_lines = lines(asset, r, count);
    let mut doc = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>"#,
    );
    for line in &body_lines {
        doc.push_str("<w:p><w:r><w:t xml:space=\"preserve\">");
        doc.push_str(&xml_escape(line));
        doc.push_str("</w:t></w:r></w:p>");
    }
    doc.push_str("</w:body></w:document>");

    zip(&[
        ZipEntry {
            name: "[Content_Types].xml",
            data: br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#
                .to_vec(),
        },
        ZipEntry {
            name: "_rels/.rels",
            data: br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#
                .to_vec(),
        },
        ZipEntry {
            name: "word/document.xml",
            data: doc.into_bytes(),
        },
    ])
}

fn xlsx(asset: &AssetMeta, r: &mut Rng) -> Vec<u8> {
    let rows = r.range(5, 60) as usize;
    let mut sheet = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>"#,
    );
    // Row 1 carries the marker as an inline string; the rest are numbers, which
    // is what a "sheets as data" test (FR-BIN, P2) wants to read back.
    sheet.push_str(&format!(
        r#"<row r="1"><c r="A1" t="inlineStr"><is><t>{}</t></is></c></row>"#,
        xml_escape(&marker(asset.index))
    ));
    for i in 0..rows {
        let rn = i + 2;
        sheet.push_str(&format!(
            r#"<row r="{rn}"><c r="A{rn}"><v>{}</v></c><c r="B{rn}"><v>{}</v></c></row>"#,
            r.below(10_000),
            r.below(10_000)
        ));
    }
    sheet.push_str("</sheetData></worksheet>");

    zip(&[
        ZipEntry {
            name: "[Content_Types].xml",
            data: br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#
                .to_vec(),
        },
        ZipEntry {
            name: "_rels/.rels",
            data: br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
                .to_vec(),
        },
        ZipEntry {
            name: "xl/workbook.xml",
            data: br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Data" sheetId="1" r:id="rId1"/></sheets></workbook>"#
                .to_vec(),
        },
        ZipEntry {
            name: "xl/_rels/workbook.xml.rels",
            data: br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#
                .to_vec(),
        },
        ZipEntry {
            name: "xl/worksheets/sheet1.xml",
            data: sheet.into_bytes(),
        },
    ])
}

/// Placeholder, and labelled as one. Correct SOI + JFIF APP0 + comment + EOI,
/// no SOF/SOS, so it will not decode. Writing a baseline JPEG encoder to make a
/// file that nothing in P0 or P2 opens is not a good use of anyone's time; the
/// README says so out loud rather than letting it look like an oversight.
fn jpg(asset: &AssetMeta, r: &mut Rng) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(512);
    out.extend_from_slice(&[0xFF, 0xD8]); // SOI
    out.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]); // APP0, length 16
    out.extend_from_slice(b"JFIF\0");
    out.extend_from_slice(&[0x01, 0x02, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00]);

    let comment = format!(
        "{} placeholder {}",
        marker(asset.index),
        r.pick(DOMAIN_NOUNS)
    );
    let len = comment.len() + 2;
    out.extend_from_slice(&[0xFF, 0xFE]); // COM
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(comment.as_bytes());
    out.extend_from_slice(&[0xFF, 0xD9]); // EOI
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Plan;

    fn assets() -> Vec<AssetMeta> {
        Plan::build(7, 400, 40).assets
    }

    #[test]
    fn every_format_has_its_magic_bytes() {
        for a in assets() {
            let bytes = render(7, &a);
            let head: &[u8] = &bytes[..8.min(bytes.len())];
            match a.format {
                AssetFormat::Pdf => {
                    assert_eq!(&head[..5], b"%PDF-");
                    assert!(bytes.ends_with(b"%%EOF\n"), "{}", a.file_name);
                }
                AssetFormat::Png => {
                    assert_eq!(head, &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
                }
                AssetFormat::Docx | AssetFormat::Xlsx => {
                    assert_eq!(&head[..4], b"PK\x03\x04");
                    // The end-of-central-directory record must be findable.
                    assert!(
                        bytes.windows(4).any(|w| w == b"PK\x05\x06"),
                        "{} has no EOCD",
                        a.file_name
                    );
                }
                AssetFormat::Jpg => {
                    assert_eq!(&head[..2], &[0xFF, 0xD8]);
                    assert_eq!(&bytes[bytes.len() - 2..], &[0xFF, 0xD9]);
                }
            }
        }
    }

    #[test]
    fn text_bearing_assets_carry_their_marker() {
        for a in assets() {
            let bytes = render(7, &a);
            let needle = marker(a.index);
            assert!(
                bytes.windows(needle.len()).any(|w| w == needle.as_bytes()),
                "{} does not contain {needle}",
                a.file_name
            );
        }
    }

    #[test]
    fn zlib_header_check_bits_are_valid() {
        // (CMF << 8 | FLG) must be a multiple of 31 or no decoder will touch it.
        let z = zlib_stored(b"some bytes to store");
        let header = (u16::from(z[0]) << 8) | u16::from(z[1]);
        assert_eq!(header % 31, 0);
        assert_eq!(
            &z[z.len() - 4..],
            &adler32(b"some bytes to store").to_be_bytes()
        );
    }
}
