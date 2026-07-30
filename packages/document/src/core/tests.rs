//! Engine tests.
//!
//! In their own file because they carry template fixtures and would otherwise
//! double the length of the module they exercise. Still a child module, not an
//! integration test: `Compilation` is deliberately not exported from `core`, so
//! nothing outside this tree can reach it.

use std::path::PathBuf;

use super::{
    DocumentKind, DocumentSpec, Error, Profile,
    engine::{CompileError, Engine, RenderOptions},
    files::Files,
    finalize_pdf,
};

fn template_root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("templates")
        .join(name)
}

/// Loads a template the same way the CLI and the server do.
///
/// Deliberately not a bespoke walker: reading the bundle through the real code
/// path means these tests also cover which files are excluded from it.
fn template(name: &str) -> Files {
    Files::read_dir(&template_root(name), "main.typ")
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

/// The render data a template ships for local work, as the host would inject
/// it. Only the `data` half is passed to the engine -- the rest of the request
/// is context the engine derives itself.
fn request_data(name: &str) -> serde_json::Value {
    let path = template_root(name).join("__data/request.json");

    let request: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));

    request["data"].clone()
}

/// The starter template, the one `template init` writes and the container
/// serves. Testing it here means a broken example cannot ship unnoticed.
#[test]
fn renders_the_example() {
    let mut engine = Engine::new();

    match engine.render(
        &template("minimal"),
        Some(&request_data("minimal")),
        None,
        &RenderOptions::default(),
    ) {
        Ok(document) => {
            assert!(document.pdf.starts_with(b"%PDF"), "not a pdf");

            for warning in &document.warnings {
                println!("warning: {warning}");
            }
        }
        Err(error) => panic!("{error:#?}"),
    }
}

#[test]
fn renders_the_invoice() {
    let mut engine = Engine::new();

    match engine.render(
        &template("invoice"),
        Some(&request_data("invoice")),
        None,
        &RenderOptions::default(),
    ) {
        Ok(document) => {
            assert!(document.pdf.starts_with(b"%PDF"), "not a pdf");
            std::fs::write("target/test-invoice.pdf", &document.pdf).unwrap();
        }
        Err(error) => panic!("{error:#?}"),
    }
}

/// Missing data is a compile error, not a silently empty document.
#[test]
fn rejects_missing_data() {
    let mut engine = Engine::new();

    let result = engine.render(
        &template("invoice"),
        None::<&serde_json::Value>,
        None,
        &RenderOptions::default(),
    );

    assert!(
        matches!(result, Err(CompileError::Template { .. })),
        "a template without data must fail to compile"
    );
}

/// The guard for the archival path: embedded fonts plus a fixed timestamp must
/// produce byte-identical output. If this ever fails, something host-dependent
/// crept back in.
#[test]
fn output_is_deterministic() {
    let mut engine = Engine::new();
    let options = RenderOptions::default();
    let data = request_data("invoice");
    let files = template("invoice");

    let first = engine
        .render(&files, Some(&data), None, &options)
        .unwrap()
        .pdf;

    let second = engine
        .render(&files, Some(&data), None, &options)
        .unwrap()
        .pdf;

    assert_eq!(
        first, second,
        "identical input must produce identical bytes"
    );
}

/// A published bundle must not be able to shadow what the host injects.
#[test]
fn rejects_reserved_internal_paths() {
    let mut files = template("minimal");

    files
        .content
        .insert("__data/request.json".to_owned(), b"{}".to_vec());

    let mut engine = Engine::new();

    let result = engine.render(
        &files,
        Some(&request_data("minimal")),
        None,
        &RenderOptions::default(),
    );

    assert!(
        matches!(result, Err(CompileError::Files(_))),
        "templates must not be able to shadow internal runtime files"
    );
}

/// The example directory carries its request file for local editing, but it is
/// not part of what gets published.
#[test]
fn request_file_stays_out_of_the_bundle() {
    let files = template("invoice");

    assert!(
        !files.content.contains_key("__data/request.json"),
        "the injected request must not travel in the bundle"
    );

    assert!(
        files.content.contains_key("fixture.json"),
        "the issuer's own details are part of the template"
    );
}

#[test]
fn finalize_pdf_maps_metadata_error() {
    let spec = DocumentSpec {
        kind: DocumentKind::Invoice,
        profile: Profile::Minimum,
    };

    let result = finalize_pdf(b"not a pdf".to_vec(), Some(&spec));

    assert!(matches!(result, Err(Error::Internal(_))));
}

#[test]
fn profile_display_uses_conformance_level() {
    assert_eq!(Profile::Minimum.to_string(), "MINIMUM");
    assert_eq!(Profile::Basic.to_string(), "BASIC");
    assert_eq!(Profile::En16931.to_string(), "EN 16931");
}
