//! The one extension-to-MIME table (architecture §7.4; ADR-0017; FR-BIN-01).
//!
//! MIME comes from the file name alone, never from sniffing content: sniffing would
//! read every binary on every rebuild (a per-file I/O cost on NFR-PRF-04's path) for a
//! distinction P1 displays and does not act on. The index (`assets.mime`), the
//! `asset_info` command, the `serat-asset` handler and R3's asset hits all call this, so
//! they cannot disagree. Pure data, no I/O.

/// What an extension we do not know becomes.
pub const OCTET_STREAM: &str = "application/octet-stream";

/// The table, lowercase extension to MIME type.
const TABLE: &[(&str, &str)] = &[
    ("pdf", "application/pdf"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("svg", "image/svg+xml"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("bmp", "image/bmp"),
    ("ico", "image/x-icon"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    ("doc", "application/msword"),
    ("xls", "application/vnd.ms-excel"),
    ("ppt", "application/vnd.ms-powerpoint"),
    ("odt", "application/vnd.oasis.opendocument.text"),
    ("ods", "application/vnd.oasis.opendocument.spreadsheet"),
    ("odp", "application/vnd.oasis.opendocument.presentation"),
    ("csv", "text/csv"),
    ("tsv", "text/tab-separated-values"),
    ("txt", "text/plain"),
    ("json", "application/json"),
    ("xml", "application/xml"),
    ("html", "text/html"),
    ("htm", "text/html"),
    ("rtf", "application/rtf"),
    ("epub", "application/epub+zip"),
    ("eml", "message/rfc822"),
    ("zip", "application/zip"),
    ("gz", "application/gzip"),
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("ogg", "audio/ogg"),
    ("m4a", "audio/mp4"),
    ("flac", "audio/flac"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("mov", "video/quicktime"),
];

/// The MIME type for an extension (no dot, any case), if the table has it.
#[must_use]
pub fn mime_for_extension(extension: &str) -> Option<&'static str> {
    TABLE
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(extension))
        .map(|&(_, mime)| mime)
}

/// The MIME type for a file name or vault-relative path, by its last extension.
/// [`OCTET_STREAM`] when the extension is absent or unknown.
#[must_use]
pub fn mime_for_path(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .and_then(|(_, extension)| mime_for_extension(extension))
        .unwrap_or(OCTET_STREAM)
}

/// The types the `serat-asset` scheme serves, exactly (ADR-0017 Decision step 5, the
/// T-10 boundary). Everything else gets 404. Not a prefix match: `image/tiff` and
/// `image/x-icon` are in the table and are not served.
pub const VIEWABLE: [&str; 8] = [
    "application/pdf",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/bmp",
    "image/svg+xml",
];

/// Whether the `serat-asset` scheme serves this type (ADR-0017).
#[must_use]
pub fn is_viewable(mime: &str) -> bool {
    VIEWABLE.contains(&mime)
}

/// The link that inserts an attachment, from the text that resolves to it (§9.3):
/// `![[text]]` for an image or PDF, `[[text]]` for anything else (FR-VLT-08).
#[must_use]
pub fn attachment_link(link_text: &str, mime: &str) -> String {
    if is_viewable(mime) {
        format!("![[{link_text}]]")
    } else {
        format!("[[{link_text}]]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_extensions_map_case_insensitively_by_the_last_extension() {
        assert_eq!(mime_for_path("a/b/Report.PDF"), "application/pdf");
        assert_eq!(mime_for_path("x.tar.gz"), "application/gzip");
        assert_eq!(mime_for_path("photo.JPeG"), "image/jpeg");
    }

    #[test]
    fn unknown_missing_and_directory_dots_are_octet_stream() {
        assert_eq!(mime_for_path("data.weird"), OCTET_STREAM);
        assert_eq!(mime_for_path("Makefile"), OCTET_STREAM);
        assert_eq!(mime_for_path("v1.2/blob"), OCTET_STREAM);
        assert_eq!(mime_for_path("trailing."), OCTET_STREAM);
    }

    #[test]
    fn exactly_the_eight_adr_0017_types_are_viewable_across_the_whole_table() {
        let mut viewable: Vec<&str> = TABLE
            .iter()
            .map(|&(_, mime)| mime)
            .filter(|mime| is_viewable(mime))
            .collect();
        viewable.sort_unstable();
        viewable.dedup();
        let mut expected = VIEWABLE.to_vec();
        expected.sort_unstable();
        assert_eq!(viewable, expected);
        assert!(!is_viewable("image/tiff") && !is_viewable("image/x-icon"));
        assert!(!is_viewable(OCTET_STREAM));
    }

    #[test]
    fn only_pdf_and_images_are_viewable_and_embedded() {
        assert!(is_viewable(mime_for_path("a.pdf")));
        assert!(is_viewable(mime_for_path("a.svg")));
        assert!(!is_viewable(mime_for_path("a.docx")));
        assert_eq!(attachment_link("a.png", "image/png"), "![[a.png]]");
        assert_eq!(attachment_link("a.pdf", "application/pdf"), "![[a.pdf]]");
        assert_eq!(
            attachment_link("a.xlsx", mime_for_path("a.xlsx")),
            "[[a.xlsx]]"
        );
    }
}
