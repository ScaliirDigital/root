//! What the compiler is allowed to see.
//!
//! A template is never a single string: it is an entrypoint plus partials plus
//! assets. They travel and live together as one immutable unit, resolved
//! entirely in memory.
//!
//! Two properties carry the design:
//!   1. No file system. These files ARE the world -- anything not in here is
//!      unreachable, and that absence is the sandbox.
//!   2. Content addressing. The hash is the identity, so identical input always
//!      produces an identical document.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Virtual path, e.g. "main.typ" or "assets/logo.svg". Never touches the host.
pub type VirtualPath = String;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Files {
    /// Where compilation starts. Must exist in `content`.
    ///
    /// Defaults to `main.typ` — the overwhelmingly common case. Naming it
    /// explicitly stays possible, because a set with several `.typ` files at
    /// the root is otherwise ambiguous.
    #[serde(default = "default_entrypoint")]
    pub entrypoint: VirtualPath,
    /// `BTreeMap`, not `HashMap`: iteration order must be deterministic, or the
    /// hash would differ between runs over identical input.
    #[serde(with = "base64_map")]
    pub content: BTreeMap<VirtualPath, Vec<u8>>,
}

fn default_entrypoint() -> VirtualPath {
    "main.typ".to_owned()
}

/// Content address: identity derived from the bytes, not from a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentHash(pub [u8; 32]);

impl std::fmt::Display for ContentHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Files {
    /// Content address of the whole set.
    ///
    /// Doubles as the compiler cache key: comemo caches aggressively, so keying
    /// on this is what keeps a changed template from serving a stale layout.
    ///
    /// Lengths are hashed alongside the bytes so that concatenation cannot
    /// collide -- `["ab", "c"]` and `["a", "bc"]` must not share an address.
    #[must_use]
    pub fn hash(&self) -> ContentHash {
        let mut hasher = blake3::Hasher::new();

        hasher.update(self.entrypoint.as_bytes());
        hasher.update(&[0]);

        for (path, bytes) in &self.content {
            hasher.update(path.as_bytes());
            hasher.update(&(bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }

        ContentHash(*hasher.finalize().as_bytes())
    }

    /// Checks the two invariants that need no compiler: the entrypoint exists,
    /// and no path escapes the virtual root.
    ///
    /// The second is a security boundary, not a plausibility check. The
    /// resolver should already be incapable of escaping, but this runs first
    /// and costs nothing -- defence in depth.
    ///
    /// # Errors
    ///
    /// [`FilesError::MissingEntrypoint`] if the entrypoint is not part of the
    /// set, [`FilesError::IllegalPath`] for an absolute path or one containing
    /// `..`.
    pub fn validate(&self) -> Result<(), FilesError> {
        if !self.content.contains_key(&self.entrypoint) {
            return Err(FilesError::MissingEntrypoint(self.entrypoint.clone()));
        }

        for path in self.content.keys() {
            if path.starts_with('/') || path.split('/').any(|segment| segment == "..") {
                return Err(FilesError::IllegalPath(path.clone()));
            }
        }

        Ok(())
    }

    /// Reads a template directory, keyed by path relative to `root`.
    ///
    /// The counterpart to the multipart intake: both produce the same set from
    /// different transports, so what a CLI validates locally is what a server
    /// would receive. Nested paths are kept as they are, so
    /// `#import "components/header.typ"` resolves exactly as it does on disk.
    ///
    /// Everything under [`INTERNAL_PREFIX`] is excluded: those files are
    /// injected per request, and a published copy would shadow the real data.
    /// [`FIXTURE`] on the other hand is part of the template -- it carries the
    /// issuer's own details, so changing them has to change the address.
    ///
    /// # Errors
    ///
    /// [`FilesError::Unreadable`] if the directory or a file cannot be read,
    /// and [`FilesError::TooLarge`] or [`FilesError::TooManyFiles`] when a
    /// ceiling is exceeded.
    pub fn read_dir(root: &std::path::Path, entrypoint: &str) -> Result<Self, FilesError> {
        let mut content = BTreeMap::new();
        collect(root, root, &mut content)?;
        content.retain(|path, _| !path.starts_with(INTERNAL_PREFIX));

        Ok(Self {
            entrypoint: entrypoint.to_owned(),
            content,
        })
    }
}

/// Sample data, read separately: it validates a template but is not part of it.
pub const FIXTURE: &str = "fixture.json";

/// The request the host injects at render time.
///
/// A template directory carries an example copy so it compiles in an editor;
/// the bundle never does, because the real one arrives per document.
pub const REQUEST: &str = "__data/request.json";

/// Reserved for what the host injects at render time.
///
/// A template directory may carry these files so it compiles locally in an
/// editor, but they never enter the bundle: the engine supplies them per
/// request, and a published copy would shadow the real data.
pub const INTERNAL_PREFIX: &str = "__data/";

/// Ceilings that apply wherever a bundle enters the system -- an upload or a
/// directory read. Defined once so the two paths cannot drift apart and start
/// accepting different things.
pub const MAX_FILES: usize = 64;
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

fn directory_entry(
    entry: std::io::Result<std::fs::DirEntry>,
    directory: &std::path::Path,
) -> Result<std::fs::DirEntry, FilesError> {
    entry.map_err(|error| FilesError::Unreadable {
        path: directory.display().to_string(),
        source: error,
    })
}

fn entry_name<'a>(
    name: &'a std::ffi::OsStr,
    path: &std::path::Path,
) -> Result<&'a str, FilesError> {
    let Some(name) = name.to_str() else {
        return Err(FilesError::IllegalPath(path.display().to_string()));
    };

    Ok(name)
}

fn collect_entries(
    root: &std::path::Path,
    directory: &std::path::Path,
    content: &mut BTreeMap<VirtualPath, Vec<u8>>,
    entries: &mut dyn Iterator<Item = std::io::Result<std::fs::DirEntry>>,
) -> Result<(), FilesError> {
    for entry in entries {
        let entry = directory_entry(entry, directory)?;
        let path = entry.path();

        let name = entry.file_name();
        let name = entry_name(&name, &path)?;

        if name.starts_with('.') {
            continue;
        }

        let metadata = entry.metadata().map_err(|error| FilesError::Unreadable {
            path: path.display().to_string(),
            source: error,
        })?;

        if metadata.is_symlink() {
            return Err(FilesError::IllegalPath(path.display().to_string()));
        }

        if metadata.is_dir() {
            collect(root, &path, content)?;
            continue;
        }

        if metadata.len() > MAX_FILE_BYTES {
            return Err(FilesError::TooLarge(path.display().to_string()));
        }

        if content.len() >= MAX_FILES {
            return Err(FilesError::TooManyFiles);
        }

        let relative = path
            .strip_prefix(root)
            .expect("collected paths sit under the root");

        let key = relative
            .to_str()
            .expect("every segment was checked for utf-8 above");

        let bytes = std::fs::read(&path).map_err(|error| FilesError::Unreadable {
            path: path.display().to_string(),
            source: error,
        })?;

        content.insert(key.replace('\\', "/"), bytes);
    }

    Ok(())
}

fn collect(
    root: &std::path::Path,
    directory: &std::path::Path,
    content: &mut BTreeMap<VirtualPath, Vec<u8>>,
) -> Result<(), FilesError> {
    let unreadable = |path: &std::path::Path, error: std::io::Error| FilesError::Unreadable {
        path: path.display().to_string(),
        source: error,
    };

    let mut entries = std::fs::read_dir(directory).map_err(|e| unreadable(directory, e))?;

    collect_entries(root, directory, content, &mut entries)
}

#[derive(Debug, thiserror::Error)]
pub enum FilesError {
    #[error("entrypoint `{0}` is not part of the file set")]
    MissingEntrypoint(VirtualPath),
    #[error("illegal virtual path: `{0}`")]
    IllegalPath(VirtualPath),
    #[error("`{path}` could not be read")]
    Unreadable {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("`{0}` exceeds the per-file limit")]
    TooLarge(String),
    #[error("too many files, limit is {MAX_FILES}")]
    TooManyFiles,
}

/// File contents travel as base64 over the wire.
///
/// `serde_json` has no byte type, so a plain `Vec<u8>` would serialize as an
/// array of numbers -- unusable from curl, and four times the size. The content
/// hash is unaffected: it is computed over the raw bytes, so the address stays
/// stable regardless of transport encoding.
mod base64_map {
    use std::collections::BTreeMap;

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        value: &BTreeMap<String, Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .iter()
            .map(|(path, bytes)| (path.clone(), STANDARD.encode(bytes)))
            .collect::<BTreeMap<_, _>>()
            .serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<String, Vec<u8>>, D::Error> {
        BTreeMap::<String, String>::deserialize(deserializer)?
            .into_iter()
            .map(|(path, encoded)| {
                STANDARD
                    .decode(encoded)
                    .map(|bytes| (path, bytes))
                    .map_err(serde::de::Error::custom)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entrypoint: &str, paths: &[&str]) -> Files {
        Files {
            entrypoint: entrypoint.to_owned(),
            content: paths
                .iter()
                .map(|path| ((*path).to_owned(), b"= ok".to_vec()))
                .collect(),
        }
    }

    fn write(root: &std::path::Path, path: &str, bytes: &[u8]) {
        let target = root.join(path);

        std::fs::create_dir_all(target.parent().expect("a file has a parent directory"))
            .expect("create parent directory");

        std::fs::write(target, bytes).expect("write file");
    }

    // -----------------------------------------------------------------------
    // Addressing
    // -----------------------------------------------------------------------

    #[test]
    fn identical_content_hashes_identically() {
        assert_eq!(
            files("main.typ", &["main.typ", "brand.typ"]).hash(),
            files("main.typ", &["main.typ", "brand.typ"]).hash()
        );
    }

    #[test]
    fn entrypoint_is_part_of_the_address() {
        assert_ne!(
            files("main.typ", &["main.typ", "other.typ"]).hash(),
            files("other.typ", &["main.typ", "other.typ"]).hash()
        );
    }

    /// The length prefix is what keeps concatenation from colliding.
    #[test]
    fn concatenation_does_not_collide() {
        let mut left = files("main.typ", &["main.typ"]);
        left.content.insert("a".to_owned(), b"xy".to_vec());
        left.content.insert("b".to_owned(), b"z".to_vec());

        let mut right = files("main.typ", &["main.typ"]);
        right.content.insert("a".to_owned(), b"x".to_vec());
        right.content.insert("b".to_owned(), b"yz".to_vec());

        assert_ne!(left.hash(), right.hash());
    }

    #[test]
    fn renders_the_address_as_hex() {
        assert_eq!(ContentHash([0xab; 32]).to_string(), "ab".repeat(32));
    }

    // -----------------------------------------------------------------------
    // Validation
    // -----------------------------------------------------------------------

    #[test]
    fn accepts_a_well_formed_set() {
        files("main.typ", &["main.typ", "components/header.typ"])
            .validate()
            .expect("nested paths are fine");
    }

    #[test]
    fn rejects_a_missing_entrypoint() {
        let error = files("main.typ", &["brand.typ"])
            .validate()
            .expect_err("entrypoint is not in the set");

        assert!(error.to_string().contains("is not part of the file set"));
    }

    #[test]
    fn rejects_traversal() {
        for path in ["../etc/passwd", "a/../../etc/passwd", "/etc/passwd"] {
            let error = files("main.typ", &["main.typ", path])
                .validate()
                .expect_err("path escapes the root");

            assert!(error.to_string().contains("illegal virtual path"));
        }
    }

    // -----------------------------------------------------------------------
    // Reading a directory
    // -----------------------------------------------------------------------

    #[test]
    fn reads_a_template_directory() {
        let root = tempfile::tempdir().expect("temporary directory");

        write(root.path(), "main.typ", b"= hello");
        write(root.path(), "components/header.typ", b"= header");
        write(root.path(), FIXTURE, b"{}");
        write(root.path(), REQUEST, b"{}");
        write(root.path(), ".hidden", b"ignored");
        write(root.path(), ".git/config", b"ignored");

        let files = Files::read_dir(root.path(), "main.typ").expect("read template directory");

        // Nested paths keep their shape so imports resolve as they do on disk.
        assert!(files.content.contains_key("components/header.typ"));

        // The fixture belongs to the template; the injected request does not.
        assert!(files.content.contains_key(FIXTURE));
        assert!(!files.content.contains_key(REQUEST));

        // Hidden entries are skipped, directories and all.
        assert!(!files.content.contains_key(".hidden"));
        assert_eq!(files.content.len(), 3);

        files.validate().expect("a read directory is valid");
    }

    #[test]
    fn reports_a_missing_directory() {
        let root = tempfile::tempdir().expect("temporary directory");

        let error = Files::read_dir(&root.path().join("gone"), "main.typ")
            .expect_err("directory does not exist");

        assert!(error.to_string().contains("could not be read"));
    }

    #[test]
    fn rejects_a_file_over_the_size_limit() {
        let root = tempfile::tempdir().expect("temporary directory");

        write(
            root.path(),
            "big.bin",
            &vec![0u8; usize::try_from(MAX_FILE_BYTES).expect("limit fits in usize") + 1],
        );

        let error = Files::read_dir(root.path(), "main.typ").expect_err("file is too large");

        assert!(error.to_string().contains("exceeds the per-file limit"));
    }

    #[test]
    fn rejects_a_set_over_the_file_limit() {
        let root = tempfile::tempdir().expect("temporary directory");

        for index in 0..=MAX_FILES {
            write(root.path(), &format!("file{index}.typ"), b"= ok");
        }

        let error = Files::read_dir(root.path(), "main.typ").expect_err("too many files");

        assert!(error.to_string().contains("too many files"));
    }

    /// A link could point outside the directory the caller meant to publish.
    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink() {
        let root = tempfile::tempdir().expect("temporary directory");

        write(root.path(), "main.typ", b"= ok");
        std::os::unix::fs::symlink("/etc/passwd", root.path().join("link.typ"))
            .expect("create symlink");

        let error = Files::read_dir(root.path(), "main.typ").expect_err("symlinks are refused");

        assert!(error.to_string().contains("illegal virtual path"));
    }

    #[cfg(unix)]
    #[test]
    fn entry_name_rejects_non_utf8() {
        use std::os::unix::ffi::OsStrExt;

        let name = std::ffi::OsStr::from_bytes(&[b'a', 0xff, b'.', b't', b'y', b'p']);
        let path = std::path::Path::new(name);

        let error = entry_name(name, path).expect_err("name is not utf-8");

        assert!(error.to_string().contains("illegal virtual path"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn read_dir_rejects_non_utf8_entry() {
        use std::os::unix::ffi::OsStrExt;

        let root = tempfile::tempdir().expect("temporary directory");
        let name = std::ffi::OsStr::from_bytes(&[b'a', 0xff, b'.', b't', b'y', b'p']);

        std::fs::write(root.path().join(name), b"= ok").expect("write file");

        let error = Files::read_dir(root.path(), "main.typ").expect_err("name is not utf-8");

        assert!(error.to_string().contains("illegal virtual path"));
    }

    // -----------------------------------------------------------------------
    // Wire format
    // -----------------------------------------------------------------------

    /// Contents survive the base64 round trip byte for byte, including bytes
    /// that are not valid utf-8.
    #[test]
    fn survives_the_wire_format() {
        let mut original = files("main.typ", &["main.typ"]);
        original
            .content
            .insert("logo.png".to_owned(), vec![0x89, 0x50, 0x4e, 0x47, 0xff]);

        let encoded = serde_json::to_string(&original).expect("serialize");

        // Readable from curl rather than an array of numbers.
        assert!(encoded.contains("\"logo.png\":\""));

        let restored: Files = serde_json::from_str(&encoded).expect("deserialize");

        assert_eq!(restored.content, original.content);
        assert_eq!(restored.hash(), original.hash());
    }

    #[test]
    fn rejects_content_that_is_not_base64() {
        let error = serde_json::from_str::<Files>(r#"{"content":{"main.typ":"not base64!"}}"#)
            .expect_err("invalid base64");

        assert!(!error.to_string().is_empty());
    }

    /// The entrypoint may be left out; `main.typ` is the common case.
    #[test]
    fn defaults_the_entrypoint() {
        let files: Files =
            serde_json::from_str(r#"{"content":{"main.typ":"PSBvaw=="}}"#).expect("deserialize");

        assert_eq!(files.entrypoint, "main.typ");
        files.validate().expect("the default entrypoint is present");
    }

    /// A file the process may see but not read.
    #[cfg(unix)]
    #[test]
    fn reports_an_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temporary directory");
        write(root.path(), "secret.typ", b"= ok");

        std::fs::set_permissions(
            root.path().join("secret.typ"),
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("drop permissions");

        let error = Files::read_dir(root.path(), "main.typ").expect_err("file is unreadable");

        assert!(error.to_string().contains("could not be read"));
    }

    /// A directory that may be listed but not traversed: names come back, but
    /// nothing can be stat'ed inside it.
    #[cfg(unix)]
    #[test]
    fn reports_an_unstattable_entry() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temporary directory");
        write(root.path(), "locked/main.typ", b"= ok");

        let locked = root.path().join("locked");
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444))
            .expect("drop the execute bit");

        let result = Files::read_dir(root.path(), "main.typ");

        // Restore before the assertion so the tempdir can clean itself up.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
            .expect("restore permissions");

        assert!(
            result
                .expect_err("entry cannot be stat'ed")
                .to_string()
                .contains("could not be read")
        );
    }

    #[test]
    fn rejects_content_that_is_not_a_string_map() {
        assert!(serde_json::from_str::<Files>(r#"{"content":{"main.typ":123}}"#).is_err());
        assert!(serde_json::from_str::<Files>(r#"{"content":[]}"#).is_err());
    }

    /// `to_string` never fails, but `Display` has to propagate a sink error.
    #[test]
    fn propagates_a_formatting_error() {
        use std::fmt::Write;

        struct FailingSink;

        impl Write for FailingSink {
            fn write_str(&mut self, _: &str) -> std::fmt::Result {
                Err(std::fmt::Error)
            }
        }

        assert!(
            std::fmt::write(&mut FailingSink, format_args!("{}", ContentHash([0; 32]))).is_err()
        );
    }

    #[test]
    fn reports_an_unreadable_directory_entry() {
        let root = tempfile::tempdir().expect("temporary directory");

        let error = directory_entry(
            Err(std::io::Error::other("directory entry unavailable")),
            root.path(),
        )
        .expect_err("directory entry should be unreadable");

        assert!(error.to_string().contains("could not be read"));
    }

    #[test]
    fn collect_reports_an_unreadable_directory_entry() {
        let root = tempfile::tempdir().expect("temporary directory");
        let mut content = BTreeMap::new();

        let mut entries =
            std::iter::once(Err(std::io::Error::other("directory entry unavailable")));

        let result = collect_entries(root.path(), root.path(), &mut content, &mut entries);

        assert!(result.is_err());
    }
}
