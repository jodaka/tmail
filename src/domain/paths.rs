//! Path helpers for attachment sources and destinations (plan §15).
//!
//! `~` is expanded inside Tmail — never through a shell — so paths with
//! spaces or special characters stay single argv entries end to end
//! (Phase 8 acceptance). Everything here is pure: the home directory is
//! injected, which keeps the helpers unit-testable without touching the
//! environment.

use std::path::{Path, PathBuf};

/// Largest file the composer accepts as an attachment (plan §15: "has an
/// acceptable size"). 25 MiB — deliberately conservative for mail.
pub const MAX_DRAFT_ATTACHMENT_BYTES: u64 = 25 * 1024 * 1024;

/// The user's home directory (`HOME`), when set.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Best-effort media type for a filename (send side, plan §15: the MIME
/// part must carry a type). A deliberately small, conventional map;
/// unknown extensions send as `application/octet-stream`, which every
/// client handles safely.
pub fn media_type_for(filename: &str) -> &'static str {
    let ext = Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "json" => "application/json",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "jpg" | "jpeg" => "image/jpeg",
        "txt" | "text" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        _ => "application/octet-stream",
    }
}

/// Expand a leading `~` or `~/…` against `home` (plan §15: expansion
/// happens in Tmail, not through a shell). `~user` forms are left literal —
/// resolving other users' homes is out of scope and would need a shell or
/// libc lookup. Without a home directory nothing is expanded.
pub fn expand_tilde(input: &Path, home: Option<&Path>) -> PathBuf {
    let Some(home) = home else {
        return input.to_path_buf();
    };
    let text = input.to_string_lossy();
    if text == "~" {
        return home.to_path_buf();
    }
    match text.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => input.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/home/ada")
    }

    #[test]
    fn tilde_forms_expand_against_the_injected_home() {
        assert_eq!(expand_tilde(Path::new("~"), Some(&home())), home());
        assert_eq!(
            expand_tilde(Path::new("~/docs/a b.pdf"), Some(&home())),
            PathBuf::from("/home/ada/docs/a b.pdf")
        );
    }

    #[test]
    fn other_paths_stay_untouched() {
        let raw = Path::new("/tmp/report final.pdf");
        assert_eq!(expand_tilde(raw, Some(&home())), raw);
        assert_eq!(
            expand_tilde(Path::new("rel/x"), Some(&home())),
            Path::new("rel/x")
        );
        // `~user` and mid-string tildes are literal.
        assert_eq!(
            expand_tilde(Path::new("~ada/x"), Some(&home())),
            Path::new("~ada/x")
        );
        assert_eq!(
            expand_tilde(Path::new("a/~b"), Some(&home())),
            Path::new("a/~b")
        );
    }

    #[test]
    fn without_home_nothing_expands() {
        let raw = Path::new("~/x");
        assert_eq!(expand_tilde(raw, None), raw);
    }

    #[test]
    fn media_types_map_by_extension() {
        assert_eq!(media_type_for("report.pdf"), "application/pdf");
        assert_eq!(
            media_type_for("photo.JPG"),
            "image/jpeg",
            "case-insensitive"
        );
        assert_eq!(media_type_for("notes.txt"), "text/plain");
        assert_eq!(media_type_for("archive.tar.gz"), "application/gzip");
        // Unknown and extension-less names fall back safely.
        assert_eq!(media_type_for("data.weird"), "application/octet-stream");
        assert_eq!(media_type_for("noext"), "application/octet-stream");
    }
}
