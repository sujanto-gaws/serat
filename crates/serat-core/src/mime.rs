//! The one extension-to-MIME table (ADR-0017 step 5, `AssetInfo.mime`).
//!
//! Extension only, never content sniffing: a file's type is what its name says, and
//! the scheme handler answers `X-Content-Type-Options: nosniff` so the webview does
//! not second-guess it. Both the `serat-asset` scheme's allowlist and the index's
//! asset metadata read this one function, so the two cannot disagree about what a
//! `.png` is.

/// The MIME type for a lowercase file extension (no dot), or
/// `application/octet-stream` if the table does not know it.
#[must_use]
pub fn mime_for_extension(extension: &str) -> &'static str {
    match extension {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "md" | "markdown" => "text/markdown",
        "txt" => "text/plain",
        "json" => "application/json",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// Whether the `serat-asset` scheme may serve this MIME type: PDF and the raster
/// and SVG image types, nothing else (ADR-0017 step 5).
#[must_use]
pub fn is_viewable(mime: &str) -> bool {
    mime == "application/pdf" || is_image(mime)
}

/// Whether the type is a viewable image, which the scheme serves whole (an `<img>`
/// cannot ask for spans).
#[must_use]
pub fn is_image(mime: &str) -> bool {
    matches!(
        mime,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "image/bmp"
            | "image/svg+xml"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_and_images_are_viewable_and_everything_else_is_not() {
        for ext in [
            "pdf", "png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "svg",
        ] {
            assert!(is_viewable(mime_for_extension(ext)), "{ext}");
        }
        for ext in ["md", "txt", "html", "zip", "mp4", "json", "", "exe"] {
            assert!(!is_viewable(mime_for_extension(ext)), "{ext}");
        }
    }

    #[test]
    fn only_viewable_images_are_images() {
        assert!(is_image("image/png"));
        assert!(!is_image("application/pdf"));
        assert!(!is_image("image/tiff"));
    }
}
