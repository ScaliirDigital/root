//! Core engine -- Typst.
//!
//! The only place in the crate that touches `typst` or `typst_pdf`. Everything
//! else goes through [`Engine::render`].
//!
//! The compilation environment Typst asks an embedder to provide is
//! implemented here directly rather than through a wrapper crate: Typst is
//! pre-1.0 and its API moves between minor versions, so a layer in between is
//! one more thing that has to catch up before we can. For a service whose whole
//! promise is a pinned engine, owning that seam is worth what it costs.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Serialize;
use typst::{
    Library, LibraryExt,
    diag::{FileError, FileResult},
    foundations::{Bytes, Datetime, Duration},
    syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot},
    text::{Font, FontBook},
    utils::LazyHash,
};
use typst_layout::PagedDocument;
use typst_pdf::{PdfOptions, PdfStandard, PdfStandards, Timestamp};

use crate::core::files::{Files, INTERNAL_PREFIX};

/// Identifies the build that produced a document.
///
/// The whole binary is the unit that matters -- Typst, the embedded fonts and
/// the PDF options travel together, so one version string covers them. Typst
/// exposes no version constant of its own, and a bare "typst 0.15" would not
/// say which `document` build it was compiled into anyway.
pub const RENDERER: &str = concat!("document ", env!("CARGO_PKG_VERSION"));

/// The font set backing every render.
struct Fonts {
    book: LazyHash<FontBook>,
    faces: Vec<Font>,
}

/// Fonts are embedded in the binary on purpose.
///
/// System fonts would make output depend on the host: a different base image
/// silently changes glyph metrics, and a document from last year could no
/// longer be reproduced byte-for-byte.
///
/// Parsed once for the lifetime of the process and shared by every engine:
/// parsing is expensive and the set never changes.
static FONTS: LazyLock<Fonts> = LazyLock::new(|| {
    // Face 0 of each file -- these are single-face TTFs, not collections.
    let faces: Vec<Font> = [
        &include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/fonts/Roboto-Regular.ttf"
        ))[..],
        &include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/fonts/Roboto-Bold.ttf"
        ))[..],
    ]
    .into_iter()
    .map(|data| Font::new(Bytes::new(data), 0).expect("embedded font must parse"))
    .collect();

    Fonts {
        book: LazyHash::new(FontBook::from_fonts(&faces)),
        faces,
    }
});

const REQUEST_PATH: &str = "__data/request.json";
const XML_PATH: &str = "__data/factur-x.xml";

static REQUEST_FILE: LazyLock<FileId> =
    LazyLock::new(|| file_id(REQUEST_PATH).expect("internal request path must be valid"));

static XML_FILE: LazyLock<FileId> =
    LazyLock::new(|| file_id(XML_PATH).expect("internal xml path must be valid"));

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct RenderOptions {
    /// Fixed timestamp for deterministic output. `None` means "now", which
    /// makes two runs over identical data produce different bytes -- never use
    /// that on the archival path.
    pub timestamp: Option<i64>,
    pub standard: Pdf,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            timestamp: Some(0),
            standard: Pdf::Plain,
        }
    }
}

/// The PDF flavours this service commits to.
///
/// Deliberately narrower than what Typst can emit: every variant here is one we
/// have verified and are willing to promise. Adding one means checking the
/// output, not just adding a match arm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pdf {
    /// Ordinary PDF. Fine for previews and offers.
    Plain,
    /// Required for `ZUGFeRD` / Factur-X hybrid invoices: archival container
    /// that permits arbitrary embedded files.
    A3b,
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// A finished render.
///
/// Warnings belong here because the compile succeeded -- Typst had remarks, not
/// objections. Errors cannot occur alongside a PDF, so they live in
/// [`CompileError::Template`] instead of in a field that would always be empty.
pub struct Document {
    pub pdf: Vec<u8>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    /// The template itself is at fault -- the caller can fix this by editing
    /// Typst. Everything else in this enum is our problem, not theirs.
    #[error("file failed to compile")]
    Template {
        errors: Vec<String>,
        warnings: Vec<String>,
    },

    #[error("files is unusable: {0}")]
    Files(String),

    #[error("data could not be encoded")]
    Encoding(#[from] serde_json::Error),

    /// Wrong before anything is rendered: an operator has to fix the
    /// environment, not the caller their request.
    #[error("invalid configuration: {0}")]
    Configuration(String),

    #[error("pdf export failed: {0}")]
    Export(String),

    #[error("compile exceeded its limits")]
    ResourceLimit,
}

// ---------------------------------------------------------------------------
// Compilation
// ---------------------------------------------------------------------------

/// The immutable part of a template compilation.
///
/// File identifiers, parsed Typst sources and binary assets depend only on the
/// template bundle. An [`Engine`] keeps the most recently used preparation so
/// repeated renders of the same template do not rebuild it for every job.
struct Template {
    main: FileId,
    sources: HashMap<FileId, Source>,
    binaries: HashMap<FileId, Bytes>,
}

impl Template {
    fn new(files: &Files) -> Result<Self, CompileError> {
        let mut sources = HashMap::new();
        let mut binaries = HashMap::new();

        for (path, bytes) in &files.content {
            if path.trim_start_matches('/').starts_with(INTERNAL_PREFIX) {
                return Err(CompileError::Files(format!(
                    "`{path}` uses reserved internal path `{INTERNAL_PREFIX}`"
                )));
            }
            let id = file_id(path)?;

            if std::path::Path::new(path)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("typ"))
            {
                let text = String::from_utf8(bytes.clone())
                    .map_err(|_| CompileError::Files(format!("`{path}` is not valid utf-8")))?;

                sources.insert(id, Source::new(id, text));
            } else {
                binaries.insert(id, Bytes::new(bytes.clone()));
            }
        }

        let main = file_id(&files.entrypoint)?;

        if !sources.contains_key(&main) {
            return Err(CompileError::Files(format!(
                "entrypoint `{}` is not a source file in the set",
                files.entrypoint
            )));
        }

        Ok(Self {
            main,
            sources,
            binaries,
        })
    }
}

#[derive(Serialize)]
struct Request<'a> {
    data: Option<&'a serde_json::Value>,
    has_xml: bool,
}

/// One Typst compilation.
///
/// The prepared template and Typst library are shared between renders.
/// Only request-specific data, attachments and the visible date live in this
/// compilation.
struct Compilation<'a> {
    library: &'a LazyHash<Library>,
    template: &'a Template,
    request: Bytes,
    xml: Option<Bytes>,
    today: Option<Datetime>,
}

impl typst::World for Compilation<'_> {
    fn library(&self) -> &LazyHash<Library> {
        self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &FONTS.book
    }

    fn main(&self) -> FileId {
        self.template.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        self.template
            .sources
            .get(&id)
            .cloned()
            .ok_or_else(|| FileError::NotFound(id.vpath().get_without_slash().into()))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == *REQUEST_FILE {
            return Ok(self.request.clone());
        }

        if id == *XML_FILE {
            return self
                .xml
                .clone()
                .ok_or_else(|| FileError::NotFound(id.vpath().get_without_slash().into()));
        }

        if let Some(bytes) = self.template.binaries.get(&id) {
            return Ok(bytes.clone());
        }

        self.template
            .sources
            .get(&id)
            .map(|source| Bytes::from_string(source.text().to_owned()))
            .ok_or_else(|| FileError::NotFound(id.vpath().get_without_slash().into()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        FONTS.faces.get(index).cloned()
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        self.today
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Persistent Typst rendering engine.
///
/// Workers keep one engine alive across jobs. The Typst library is built once
/// per worker, while immutable template state is retained across repeated
/// renders of the same template.
pub struct Engine {
    library: LazyHash<Library>,
    prepared: Option<(blake3::Hash, Template)>,
}

impl Default for Engine {
    fn default() -> Self {
        Self {
            library: LazyHash::new(Library::builder().build()),
            prepared: None,
        }
    }
}

impl Engine {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Renders `files` with request data exposed as virtual in-memory files.
    ///
    /// The request data never becomes Typst syntax and never touches disk.
    ///
    /// # Errors
    ///
    /// [`CompileError::Template`] if the template is at fault,
    /// [`CompileError::Files`] for a malformed file set,
    /// [`CompileError::Encoding`] if `data` cannot be serialized, and
    /// [`CompileError::Export`] if PDF writing fails.
    pub fn render(
        &mut self,
        files: &Files,
        data: Option<&serde_json::Value>,
        xml: Option<&str>,
        options: &RenderOptions,
    ) -> Result<Document, CompileError> {
        let hash = template_hash(files);

        let template = match &self.prepared {
            Some((cached, template)) if cached == &hash => template,
            // Prepared once per template and kept: parsing sources and building
            // the file map is the expensive part, and a worker usually sees the
            // same template many times in a row.
            _ => {
                let (_, template) = self.prepared.insert((hash, Template::new(files)?));
                template
            }
        };

        let request = Bytes::new(
            serde_json::to_vec(&Request {
                data,
                has_xml: xml.is_some(),
            })
            .expect("a json value serializes"),
        );

        let xml = xml.map(|xml| Bytes::from_string(xml.to_owned()));

        let compilation = Compilation {
            library: &self.library,
            template,
            request,
            xml,
            today: today(options),
        };

        let compiled = typst::compile::<PagedDocument>(&compilation);

        let document = match compiled.output {
            Ok(document) => document,
            Err(errors) => {
                return Err(CompileError::Template {
                    errors: messages(&errors),
                    warnings: messages(&compiled.warnings),
                });
            }
        };

        let warnings = messages(&compiled.warnings);

        let pdf = typst_pdf::pdf(&document, &pdf_options(options))
            .map_err(|errors| CompileError::Export(format!("{errors:#?}")))?;

        Ok(Document { pdf, warnings })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Fingerprints the immutable template bundle for the engine-local cache.
///
/// Length prefixes make the representation unambiguous even when arbitrary
/// binary assets contain separator bytes. The hash never leaves the process;
/// it only decides whether the existing preparation may be reused.
fn template_hash(files: &Files) -> blake3::Hash {
    fn update(hasher: &mut blake3::Hasher, bytes: &[u8]) {
        hasher.update(&bytes.len().to_le_bytes());
        hasher.update(bytes);
    }

    let mut hasher = blake3::Hasher::new();

    update(&mut hasher, files.entrypoint.as_bytes());

    for (path, bytes) in &files.content {
        update(&mut hasher, path.as_bytes());
        update(&mut hasher, bytes);
    }

    hasher.finalize()
}

/// The date a template sees through `datetime.today()`.
///
/// Derived from the same fixed timestamp that stamps the PDF, so a template
/// that prints today's date stays reproducible on the archival path.
fn today(options: &RenderOptions) -> Option<Datetime> {
    let seconds = options
        .timestamp
        .unwrap_or_else(|| time::OffsetDateTime::now_utc().unix_timestamp());

    let moment = time::OffsetDateTime::from_unix_timestamp(seconds).ok()?;

    Datetime::from_ymd(moment.year(), u8::from(moment.month()), moment.day())
}

/// Turns a bundle path into the identifier Typst uses internally.
///
/// Everything sits in the project root: there are no packages, because there is
/// no package resolver -- which is the sandbox.
fn file_id(path: &str) -> Result<FileId, CompileError> {
    let vpath = VirtualPath::new(path)
        .map_err(|error| CompileError::Files(format!("`{path}`: {error}")))?;

    Ok(RootedPath::new(VirtualRoot::Project, vpath).intern())
}

/// Translates our narrow [`Pdf`] choice into what Typst expects.
///
/// Infallible by construction: every variant of [`Pdf`] maps to a standard set
/// Typst accepts, which is the point of keeping the enum narrower than what
/// Typst can emit.
fn pdf_options(options: &RenderOptions) -> PdfOptions {
    let standards = match options.standard {
        Pdf::Plain => PdfStandards::default(),
        Pdf::A3b => PdfStandards::new(&[PdfStandard::A_3b]).expect("A-3b is a valid standard set"),
    };

    PdfOptions {
        standards,
        timestamp: options.timestamp.and_then(utc_timestamp),
        ..PdfOptions::default()
    }
}

/// Converts Unix seconds into the timestamp Typst stamps into the PDF.
fn utc_timestamp(seconds: i64) -> Option<Timestamp> {
    let moment = time::OffsetDateTime::from_unix_timestamp(seconds).ok()?;

    let datetime = Datetime::from_ymd_hms(
        moment.year(),
        u8::from(moment.month()),
        moment.day(),
        moment.hour(),
        moment.minute(),
        moment.second(),
    )
    .expect("a representable instant has valid calendar fields");

    Some(Timestamp::new_utc(datetime))
}

/// Typst diagnostics carry spans and traces; only the message is useful to a caller.
fn messages(diagnostics: &[typst::diag::SourceDiagnostic]) -> Vec<String> {
    diagnostics.iter().map(|d| d.message.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entrypoint: &str, content: &[(&str, &str)]) -> Files {
        Files {
            entrypoint: entrypoint.to_owned(),
            content: content
                .iter()
                .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
                .collect(),
        }
    }

    fn render(files: &Files) -> Result<Document, CompileError> {
        Engine::new().render(files, None, None, &RenderOptions::default())
    }

    /// The whole failure as text, diagnostics included.
    fn failure(files: &Files) -> String {
        format!("{:?}", render(files).err().expect("expected a failure"))
    }

    // -----------------------------------------------------------------------
    // Missing files
    // -----------------------------------------------------------------------

    /// Importing a source file that is not in the bundle.
    #[test]
    fn reports_a_missing_import() {
        let files = files("main.typ", &[("main.typ", r#"#import "gone.typ": *"#)]);

        assert!(failure(&files).contains("file not found"));
    }

    /// Reading a binary asset that is not in the bundle.
    #[test]
    fn reports_a_missing_asset() {
        let files = files("main.typ", &[("main.typ", r#"#read("gone.png")"#)]);

        assert!(failure(&files).contains("Template"));
    }

    /// A template that expects an attachment, rendered without one.
    #[test]
    fn reports_a_missing_attachment() {
        let files = files(
            "main.typ",
            &[("main.typ", r#"#read("__data/factur-x.xml")"#)],
        );

        assert!(failure(&files).contains("Template"));
    }

    /// The same template with an attachment supplied.
    #[test]
    fn reads_the_supplied_attachment() {
        let files = files(
            "main.typ",
            &[("main.typ", r#"#read("__data/factur-x.xml").len()"#)],
        );

        Engine::new()
            .render(&files, None, Some("<invoice/>"), &RenderOptions::default())
            .expect("attachment is available to the template");
    }

    /// A source file read as bytes rather than imported.
    #[test]
    fn reads_a_source_file_as_bytes() {
        let files = files(
            "main.typ",
            &[
                ("main.typ", r#"#str(read("brand.typ")).len()"#),
                ("brand.typ", "#let accent = red"),
            ],
        );

        render(&files).expect("a source file is readable as bytes");
    }

    // -----------------------------------------------------------------------
    // Determinism
    // -----------------------------------------------------------------------

    /// The date a template sees comes from the fixed timestamp, not the clock --
    /// that is what makes an archived document reproducible.
    #[test]
    fn derives_the_visible_date_from_the_timestamp() {
        let files = files("main.typ", &[("main.typ", "#datetime.today().display()")]);

        let options = RenderOptions {
            timestamp: Some(1_754_870_400),
            standard: Pdf::Plain,
        };

        let first = Engine::new()
            .render(&files, None, None, &options)
            .expect("render");

        let second = Engine::new()
            .render(&files, None, None, &options)
            .expect("render");

        assert_eq!(first.pdf, second.pdf);
    }

    /// Without a fixed timestamp the date is the real one, so the same input
    /// does not have to produce the same bytes.
    #[test]
    fn falls_back_to_the_current_date() {
        let options = RenderOptions {
            timestamp: None,
            standard: Pdf::Plain,
        };

        assert!(today(&options).is_some());
    }

    #[test]
    fn rejects_a_timestamp_outside_the_representable_range() {
        let options = RenderOptions {
            timestamp: Some(i64::MAX),
            standard: Pdf::Plain,
        };

        assert!(today(&options).is_none());
        assert!(utc_timestamp(i64::MAX).is_none());
    }

    // -----------------------------------------------------------------------
    // Paths
    // -----------------------------------------------------------------------

    #[test]
    fn rejects_a_path_typst_cannot_represent() {
        let error = file_id("../escape.typ").expect_err("a path must stay inside the project");
        assert!(error.to_string().starts_with("files is unusable:"));
    }

    /// A bundle may not shadow the paths the engine injects.
    #[test]
    fn rejects_a_reserved_internal_path() {
        let files = files(
            "main.typ",
            &[("main.typ", "ok"), ("__data/request.json", "{}")],
        );

        assert!(failure(&files).contains("reserved internal path"));
    }

    #[test]
    fn rejects_a_source_file_that_is_not_utf8() {
        let mut files = files("main.typ", &[("main.typ", "ok")]);
        files
            .content
            .insert("broken.typ".to_owned(), vec![0xff, 0xfe]);

        assert!(failure(&files).contains("not valid utf-8"));
    }

    #[test]
    fn rejects_an_entrypoint_typst_cannot_represent() {
        let files = files("../main.typ", &[("main.typ", "ok")]);

        assert!(failure(&files).contains("../main.typ"));
    }

    #[test]
    fn rejects_an_entrypoint_that_is_not_a_source_file() {
        let files = files("logo.png", &[("logo.png", "not typst")]);

        assert!(failure(&files).contains("is not a source file in the set"));
    }

    /// The second render of the same bundle reuses the prepared template --
    /// that reuse is what keeps a worker fast across jobs.
    #[test]
    fn reuses_the_prepared_template() {
        let bundle = files("main.typ", &[("main.typ", "ok")]);
        let mut engine = Engine::new();

        let first = engine
            .render(&bundle, None, None, &RenderOptions::default())
            .expect("first render");

        let second = engine
            .render(&bundle, None, None, &RenderOptions::default())
            .expect("second render");

        assert_eq!(first.pdf, second.pdf);

        // A different bundle has to rebuild rather than reuse.
        let other = files("main.typ", &[("main.typ", "different")]);

        let third = engine
            .render(&other, None, None, &RenderOptions::default())
            .expect("third render");

        assert_ne!(second.pdf, third.pdf);
    }

    #[test]
    fn rejects_a_bundle_path_typst_cannot_represent() {
        let bundle = files("main.typ", &[("main.typ", "ok"), ("../escape.typ", "ok")]);

        assert!(failure(&bundle).contains("../escape.typ"));
    }

    /// PDF/A-3b rejects an attachment without a mime type and description, and
    /// that rejection has to reach the caller as an error, not a panic.
    #[test]
    fn reports_a_failed_pdf_export() {
        let bundle = files(
            "main.typ",
            &[("main.typ", r#"#pdf.attach("x.bin", bytes((1,2,3)))"#)],
        );

        let options = RenderOptions {
            timestamp: Some(0),
            standard: Pdf::A3b,
        };

        let error = Engine::new()
            .render(&bundle, None, None, &options)
            .err()
            .expect("export must fail");

        assert!(error.to_string().starts_with("pdf export failed:"));
        assert!(format!("{error:?}").contains("mime type is missing"));
    }
}
