//! Classify a filename / mime type into a short uppercase display badge.

/// A short type badge (e.g. "PDF", "IMG") for a transfer row.
/// Tries the filename extension first, then the mime type, then "FILE".
pub fn file_kind(name: &str, mime: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "PDF",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "heic" => "IMG",
        "mp4" | "mov" | "mkv" | "webm" | "avi" => "VID",
        "mp3" | "wav" | "flac" | "ogg" | "m4a" => "AUD",
        "zip" | "tar" | "gz" | "rar" | "7z" => "ZIP",
        "doc" | "docx" | "txt" | "md" | "rtf" | "pages" => "DOC",
        _ => mime_kind(mime),
    }
}

fn mime_kind(mime: &str) -> &'static str {
    if mime.starts_with("image/") {
        "IMG"
    } else if mime.starts_with("video/") {
        "VID"
    } else if mime.starts_with("audio/") {
        "AUD"
    } else {
        "FILE"
    }
}

#[cfg(test)]
mod tests {
    use super::file_kind;

    #[test]
    fn classifies_by_extension() {
        assert_eq!(file_kind("report.pdf", ""), "PDF");
        assert_eq!(file_kind("photo.JPG", ""), "IMG");
        assert_eq!(file_kind("clip.mp4", ""), "VID");
        assert_eq!(file_kind("song.flac", ""), "AUD");
        assert_eq!(file_kind("archive.tar.gz", ""), "ZIP");
        assert_eq!(file_kind("notes.md", ""), "DOC");
    }

    #[test]
    fn falls_back_to_mime_then_file() {
        assert_eq!(file_kind("noext", "image/png"), "IMG");
        assert_eq!(file_kind("noext", "video/mp4"), "VID");
        assert_eq!(file_kind("mystery", "application/x-thing"), "FILE");
        assert_eq!(file_kind("mystery", ""), "FILE");
    }
}
