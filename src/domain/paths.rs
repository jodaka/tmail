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

/// The user's home directory: `$HOME`, or `$USERPROFILE` on platforms
/// where GitHub runners and stock shells never set `HOME` (Windows port,
/// issue y90w). Empty values are treated as unset.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
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

/// Whether `path` resolves to an existing, executable file. Unix checks
/// the execution permission bits (so a non-executable file cannot pass the
/// startup check and fail later on first use); other platforms keep the
/// is-a-file check, as there is no portable exec bit.
fn executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.is_file()
            && path
                .metadata()
                .map(|meta| meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Whether `program` resolves as an executable: a direct path (anything
/// with a separator) must exist as an executable file (issue 5ab7: a
/// non-executable file is rejected here, not on first use); otherwise each
/// `PATH` entry is searched. Shared by the config and backend layers, which
/// both report a missing external program up front instead of on first use.
pub fn program_on_path_exists(program: &str) -> bool {
    // Any separator (both conventions: Unix `/` and Windows `\`) means a
    // direct path, not a bare program name to search on `PATH`.
    if program.contains('/') || program.contains('\\') {
        return executable_file(Path::new(program));
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| executable_file(&dir.join(program))))
        .unwrap_or(false)
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
    fn direct_paths_require_an_executable_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fake-program");
        std::fs::File::create(&path).expect("touch");
        // Unix: 0644, no execution bit — the start-up check must reject
        // the file here instead of letting the first operation fail
        // later. Windows has no exec bit, so a plain existing file is
        // accepted there and only the positive check can hold.
        #[cfg(unix)]
        assert!(!program_on_path_exists(path.to_str().unwrap()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("chmod +x");
        }
        assert!(program_on_path_exists(path.to_str().unwrap()));
        // Directories and missing paths never pass the direct-path check.
        assert!(!program_on_path_exists(dir.path().to_str().unwrap()));
        assert!(!program_on_path_exists("./no-such-tmail-program"));
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
