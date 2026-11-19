//! Cleaning of incoming file names and safe placement under the download
//! folder (R8.6, design §9.2, security.md).
//!
//! A peer's `rel_path` is hostile. [`clean_rel_path`] turns it into a list of
//! plain name components or refuses it. [`ensure_dirs`] creates the folders
//! one component at a time and refuses anything that is not a real folder
//! inside the root, so a symbolic link or junction can never lead outside it.
//! [`unique_path`] picks `name (1).ext` instead of overwriting.

use std::io;
use std::path::{Path, PathBuf};

/// Longest `rel_path` accepted, in bytes.
pub const MAX_REL_PATH: usize = 4096;
/// Most folder levels in a `rel_path`.
pub const MAX_DEPTH: usize = 32;
/// Longest single name, in bytes, after cleaning.
pub const MAX_NAME: usize = 200;

/// Suffix of a file that is still arriving (R8.4).
pub const PART_SUFFIX: &str = ".urdevice-part";
/// Suffix of the resume sidecar next to a part file (R8.5).
pub const RESUME_SUFFIX: &str = ".urdevice-part.resume";

/// Why a name was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// Empty, or nothing left after cleaning.
    #[error("empty name")]
    Empty,
    /// Longer than the limits.
    #[error("name too long")]
    TooLong,
    /// `..`, an absolute path, a drive letter, or a UNC prefix.
    #[error("path leaves the folder")]
    Escapes,
    /// NUL or another control character.
    #[error("control character in name")]
    Control,
}

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³", "CONIN$", "CONOUT$",
];

/// True for a Windows reserved device name, with or without an extension
/// (`CON`, `nul.txt`, `COM1.tar.gz`).
pub fn is_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem))
}

/// Clean one name component. Refuses `..` and control characters; replaces
/// characters Windows cannot store with `_`; strips trailing dots and spaces
/// (Windows drops them); prefixes reserved names with `_`; and keeps the name
/// from looking like one of our own part files.
pub fn clean_name(raw: &str) -> Result<String, PathError> {
    if raw == ".." {
        return Err(PathError::Escapes);
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err(PathError::Control);
    }
    let mut s: String = raw
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '/' | '\\' => '_',
            c => c,
        })
        .collect();
    let trimmed = s.trim_end_matches(['.', ' ']).trim_start_matches(' ');
    if trimmed.is_empty() {
        return Err(PathError::Empty);
    }
    s = trimmed.to_owned();
    if is_reserved(&s) {
        s.insert(0, '_');
    }
    if s.ends_with(PART_SUFFIX) || s.ends_with(RESUME_SUFFIX) {
        s.push('_');
    }
    if s.len() > MAX_NAME {
        s = shorten(&s, MAX_NAME);
    }
    Ok(s)
}

/// Cut a name to at most `max` bytes, keeping a short extension.
fn shorten(name: &str, max: usize) -> String {
    let (stem, ext) = split_ext(name);
    let ext = if ext.len() <= 16 { ext } else { "" };
    let mut keep = max.saturating_sub(ext.len());
    while !stem.is_char_boundary(keep.min(stem.len())) {
        keep -= 1;
    }
    let mut out = stem[..keep.min(stem.len())]
        .trim_end_matches(['.', ' '])
        .to_owned();
    if out.is_empty() {
        out.push('_');
    }
    out.push_str(ext);
    out
}

/// Split `a.tar.gz` into `a.tar` and `.gz`; a leading dot is part of the stem.
pub fn split_ext(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    }
}

/// Clean a peer's relative path into name components (R8.6).
///
/// Both `/` and `\` separate folders. Refused: absolute paths, drive letters
/// (`C:`), UNC and device prefixes, `..` anywhere, NUL and other control
/// characters, and paths over the limits. Empty and `.` components are
/// skipped.
pub fn clean_rel_path(raw: &str) -> Result<Vec<String>, PathError> {
    if raw.len() > MAX_REL_PATH {
        return Err(PathError::TooLong);
    }
    if raw.contains('\0') {
        return Err(PathError::Control);
    }
    if raw.starts_with('/') || raw.starts_with('\\') {
        return Err(PathError::Escapes);
    }
    // `C:foo`, `C:\foo`: a drive letter, relative or not.
    let b = raw.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Err(PathError::Escapes);
    }
    let mut out = Vec::new();
    for part in raw.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err(PathError::Escapes);
        }
        out.push(clean_name(part)?);
        if out.len() > MAX_DEPTH {
            return Err(PathError::TooLong);
        }
    }
    if out.is_empty() {
        return Err(PathError::Empty);
    }
    Ok(out)
}

fn not_inside() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "outside the download folder",
    )
}

/// Create (or find) the folders `dirs` under `root`, one level at a time.
/// Every level must be a real folder, not a link. Returns the folder, checked
/// once more with `canonicalize` to be inside `root` (design §9.2).
pub fn ensure_dirs(root: &Path, dirs: &[String]) -> io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let canon_root = root.canonicalize()?;
    let mut cur = canon_root.clone();
    for d in dirs {
        cur.push(d);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_dir() => {}
            Ok(_) => return Err(not_inside()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => match std::fs::create_dir(&cur) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if !std::fs::symlink_metadata(&cur)?.file_type().is_dir() {
                        return Err(not_inside());
                    }
                }
                Err(e) => return Err(e),
            },
            Err(e) => return Err(e),
        }
    }
    let canon = cur.canonicalize()?;
    if !canon.starts_with(&canon_root) {
        return Err(not_inside());
    }
    Ok(canon)
}

/// True when `p` exists in any form, including a dangling link.
pub fn taken(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

/// `name`, or `name (1).ext`, `name (2).ext`, ... : the first for which
/// `free` holds.
pub fn unique_name(name: &str, free: impl Fn(&str) -> bool) -> io::Result<String> {
    if free(name) {
        return Ok(name.to_owned());
    }
    let (stem, ext) = split_ext(name);
    for n in 1..10_000 {
        let candidate = format!("{stem} ({n}){ext}");
        if free(&candidate) {
            return Ok(candidate);
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name"))
}

/// A free path for `name` in `dir`: never an existing file (R8.6).
pub fn unique_path(dir: &Path, name: &str) -> io::Result<PathBuf> {
    unique_name(name, |n| !taken(&dir.join(n))).map(|n| dir.join(n))
}

/// Move a verified part file to its final name without ever replacing an
/// existing file. Tries `name`, then `name (1).ext`, ... Returns the path
/// used.
pub fn place(part: &Path, dir: &Path, name: &str) -> io::Result<PathBuf> {
    for _ in 0..10_000 {
        let dest = unique_path(dir, name)?;
        // A hard link fails if `dest` exists, so it never replaces a file that
        // appeared since `unique_path` looked.
        match std::fs::hard_link(part, &dest) {
            Ok(()) => {
                std::fs::remove_file(part)?;
                return Ok(dest);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            // File systems without hard links (FAT, some FUSE): check, then rename.
            Err(_) => {
                if taken(&dest) {
                    continue;
                }
                std::fs::rename(part, &dest)?;
                return Ok(dest);
            }
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free name"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "urdevice-path-{}-{}",
            std::process::id(),
            crate::crypto::random_u64().unwrap()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn plain_paths_pass() {
        assert_eq!(clean_rel_path("a.txt").unwrap(), ["a.txt"]);
        assert_eq!(
            clean_rel_path("Photos/2024/x.jpg").unwrap(),
            ["Photos", "2024", "x.jpg"]
        );
        assert_eq!(clean_rel_path("a\\b").unwrap(), ["a", "b"]);
        assert_eq!(clean_rel_path("./a//b/.").unwrap(), ["a", "b"]);
        assert_eq!(clean_rel_path(".hidden").unwrap(), [".hidden"]);
    }

    #[test]
    fn escapes_are_refused() {
        for p in [
            "../x",
            "a/../../x",
            "a\\..\\x",
            "/etc/passwd",
            "\\\\server\\share\\x",
            "C:\\Windows\\x",
            "c:x",
            "..",
        ] {
            assert_eq!(clean_rel_path(p), Err(PathError::Escapes), "{p}");
        }
        assert_eq!(clean_rel_path("a\0b"), Err(PathError::Control));
        assert_eq!(clean_rel_path("a\nb"), Err(PathError::Control));
        assert_eq!(clean_rel_path(""), Err(PathError::Empty));
        assert_eq!(clean_rel_path("./"), Err(PathError::Empty));
        assert_eq!(clean_rel_path("..."), Err(PathError::Empty));
        assert_eq!(clean_rel_path(&"a/".repeat(40)), Err(PathError::TooLong));
    }

    #[test]
    fn windows_names_are_made_safe() {
        assert_eq!(clean_name("CON").unwrap(), "_CON");
        assert_eq!(clean_name("nul.txt").unwrap(), "_nul.txt");
        assert_eq!(clean_name("com1.tar.gz").unwrap(), "_com1.tar.gz");
        assert_eq!(clean_name("console.txt").unwrap(), "console.txt");
        assert_eq!(clean_name("a:b*c?.txt").unwrap(), "a_b_c_.txt");
        assert_eq!(clean_name("trailing. . ").unwrap(), "trailing");
        assert_eq!(clean_name("x.urdevice-part").unwrap(), "x.urdevice-part_");
        let long = format!("{}.jpg", "é".repeat(300));
        let s = clean_name(&long).unwrap();
        assert!(s.len() <= MAX_NAME && s.ends_with(".jpg"));
    }

    #[test]
    fn collisions_get_numbers() {
        let d = temp();
        std::fs::write(d.join("a.txt"), b"1").unwrap();
        std::fs::write(d.join("a (1).txt"), b"1").unwrap();
        assert_eq!(unique_path(&d, "a.txt").unwrap(), d.join("a (2).txt"));
        assert_eq!(unique_path(&d, "b").unwrap(), d.join("b"));
        assert_eq!(
            unique_name(".bashrc", |n| n != ".bashrc").unwrap(),
            ".bashrc (1)"
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn place_never_overwrites() {
        let d = temp();
        std::fs::write(d.join("a.txt"), b"old").unwrap();
        let part = d.join("a.txt.urdevice-part");
        std::fs::write(&part, b"new").unwrap();
        let dest = place(&part, &d, "a.txt").unwrap();
        assert_eq!(dest, d.join("a (1).txt"));
        assert_eq!(std::fs::read(d.join("a.txt")).unwrap(), b"old");
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert!(!part.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn dirs_stay_inside() {
        let d = temp();
        let made = ensure_dirs(&d, &["x".into(), "y".into()]).unwrap();
        assert!(made.ends_with("x/y") || made.ends_with("x\\y"));
        std::fs::write(d.join("file"), b"").unwrap();
        assert!(ensure_dirs(&d, &["file".into(), "z".into()]).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_refused() {
        let d = temp();
        let outside = temp();
        std::os::unix::fs::symlink(&outside, d.join("link")).unwrap();
        assert!(ensure_dirs(&d, &["link".into()]).is_err());
        assert!(ensure_dirs(&d, &["link".into(), "deeper".into()]).is_err());
        assert!(!outside.join("deeper").exists());
        // A link in the way of the final name is skipped, never followed.
        std::os::unix::fs::symlink(outside.join("t.txt"), d.join("t.txt")).unwrap();
        assert_eq!(unique_path(&d, "t.txt").unwrap(), d.join("t (1).txt"));
        std::fs::remove_dir_all(&d).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }
}

#[cfg(test)]
mod props {
    #![allow(clippy::unwrap_used)]

    use std::path::Component;

    use proptest::prelude::*;

    use super::*;

    /// Pieces that hostile names are made of.
    fn piece() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("..".to_owned()),
            Just(".".to_owned()),
            Just("/".to_owned()),
            Just("\\".to_owned()),
            Just("C:".to_owned()),
            Just("CON".to_owned()),
            Just("nul.txt".to_owned()),
            Just("\0".to_owned()),
            Just(" .".to_owned()),
            Just(".urdevice-part".to_owned()),
            "[a-zA-Z0-9 ._-]{1,12}",
            "\\PC{1,6}",
        ]
    }

    fn hostile() -> impl Strategy<Value = String> {
        proptest::collection::vec(piece(), 1..12).prop_map(|v| v.concat())
    }

    proptest! {
        /// A cleaned path never leaves the folder it is joined to (design §16).
        #[test]
        fn cleaned_paths_stay_inside(raw in hostile()) {
            if let Ok(parts) = clean_rel_path(&raw) {
                prop_assert!(!parts.is_empty() && parts.len() <= MAX_DEPTH);
                let root = Path::new("/root/downloads");
                let mut joined = root.to_path_buf();
                for p in &parts {
                    prop_assert!(!p.is_empty() && p.len() <= MAX_NAME);
                    prop_assert!(p != "." && p != "..");
                    prop_assert!(!p.contains(['/', '\\', ':', '\0']));
                    prop_assert!(!p.chars().any(char::is_control));
                    prop_assert!(!is_reserved(p), "{p}");
                    prop_assert!(!p.ends_with(PART_SUFFIX) && !p.ends_with(RESUME_SUFFIX));
                    prop_assert!(!p.ends_with('.') && !p.ends_with(' '));
                    joined.push(p);
                }
                prop_assert!(joined.starts_with(root));
                let extra: Vec<_> = joined.strip_prefix(root).unwrap().components().collect();
                prop_assert_eq!(extra.len(), parts.len());
                prop_assert!(extra.iter().all(|c| matches!(c, Component::Normal(_))));
            }
        }

        /// Cleaning is stable: a clean path cleans to itself.
        #[test]
        fn cleaning_is_idempotent(raw in hostile()) {
            if let Ok(parts) = clean_rel_path(&raw) {
                prop_assert_eq!(clean_rel_path(&parts.join("/")).unwrap(), parts);
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// Folders made for a hostile path are inside the root on the real
        /// file system.
        #[test]
        fn folders_are_made_inside_the_root(raw in hostile()) {
            if let Ok(parts) = clean_rel_path(&raw) {
                let root = std::env::temp_dir().join(format!(
                    "urdevice-prop-{}-{}",
                    std::process::id(),
                    crate::crypto::random_u64().unwrap()
                ));
                let (_, dirs) = parts.split_last().unwrap();
                let made = ensure_dirs(&root, dirs);
                let canon = root.canonicalize().unwrap();
                if let Ok(d) = made {
                    prop_assert!(d.starts_with(&canon));
                }
                std::fs::remove_dir_all(&root).unwrap();
            }
        }
    }
}
