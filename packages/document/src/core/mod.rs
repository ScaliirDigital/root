//! Document rendering and how it is executed.
//!
//! Everything Typst-specific lives inside this module and nowhere else. That is
//! deliberate: the Typst library API is pre-1.0 and breaks between minor
//! versions, so an upgrade should be a local change here, not a refactor across
//! the service.
//!
//! The module is split by what runs where:
//!   - [`files`]    what the compiler may see: an immutable, content-addressed
//!     set of virtual files
//!   - [`engine`]   turning those files plus data into a PDF
//!   - [`protocol`] the wire format between server and worker
//!   - [`process`]  the server and child side: worker pool plus one engine per worker
//!   - [`invoice`]  the canonical invoice and the Factur-X XML generated from it
//!   - [`profile`]  which Factur-X profile a document claims to be
//!   - [`zugferd`]  the XMP metadata Typst cannot write, added afterwards
//!
//! Callers use the functions and types exported from this module. The modules
//! underneath are implementation details: HTTP, CLI and storage code should not
//! assemble rendering jobs or document pipelines themselves.

use serde::Deserialize;

mod engine;
mod files;
mod invoice;
mod process;
mod profile;
mod protocol;
mod zugferd;

pub use engine::{CompileError, Document, Engine, Pdf, RENDERER, RenderOptions};
pub use files::{FIXTURE, Files, MAX_FILE_BYTES, MAX_FILES, REQUEST};
pub use invoice::{Invoice, Issuer, factur_x};
pub use process::{Limits, compile, initialize, run_worker, shutdown, start};
pub use profile::Profile;
pub use protocol::Job;

/// What kind of document the caller wants produced.
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentKind {
    Invoice,
}

/// Document semantics that affect validation and the generated PDF.
#[derive(Deserialize)]
pub struct DocumentSpec {
    #[serde(rename = "type")]
    pub kind: DocumentKind,
    pub profile: Profile,
}

/// Input for rendering against an already published template.
pub struct RenderRequest {
    pub files: Files,
    pub data: serde_json::Value,
    pub archival: bool,
    pub document: Option<DocumentSpec>,
}

/// A finished document plus the persistence guarantee it requires.
pub struct RenderedDocument {
    pub pdf: Vec<u8>,
    pub archival: bool,
}

/// Result of validating a template before it is published.
pub struct TemplateValidation {
    pub content_hash: String,
    pub accepted: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Failures from the core document pipeline, independent of any transport.
#[derive(Debug)]
pub enum Error {
    InvalidFiles(String),
    TemplateFailed(Vec<String>),
    InvalidData(Vec<String>),
    Internal(String),
}

/// Preview path. No storage, no version and no reproducibility guarantee.
pub async fn render_adhoc(
    files: Files,
    data: serde_json::Value,
    limits: Limits,
) -> Result<Vec<u8>, Error> {
    files
        .validate()
        .map_err(|error| Error::InvalidFiles(error.to_string()))?;

    let job = Job {
        files,
        data: Some(data),
        xml: None,
        // Live timestamp is fine here: nothing is being archived.
        options: RenderOptions {
            timestamp: None,
            standard: Pdf::Plain,
        },
    };

    produce(&job, limits).await
}

/// Validates a template against its own fixture before anything is persisted.
///
/// Validation itself never fails: rejection is represented by `accepted: false`
/// together with all diagnostics the caller needs to fix the template.
pub async fn validate_template(
    files: &Files,
    fixture: &serde_json::Value,
    limits: Limits,
) -> TemplateValidation {
    let content_hash = files.hash().to_string();

    let rejected = |errors: Vec<String>, warnings: Vec<String>| TemplateValidation {
        content_hash: content_hash.clone(),
        accepted: false,
        errors,
        warnings,
    };

    // Structure first: no compiler needed, so it is the cheapest gate.
    if let Err(error) = files.validate() {
        return rejected(vec![error.to_string()], Vec::new());
    }

    // Determinism is forced here, not left to the caller: a template that only
    // renders correctly with a live timestamp is a defect we want to see at
    // publish time.
    let job = Job {
        files: files.clone(),
        data: fixture.get("data").cloned(),
        xml: None,
        options: RenderOptions {
            timestamp: Some(0),
            ..RenderOptions::default()
        },
    };

    match compile(&job, limits).await {
        Ok(document) => TemplateValidation {
            content_hash: content_hash.clone(),
            accepted: true,
            errors: Vec::new(),
            warnings: document.warnings,
        },
        Err(CompileError::Template { errors, warnings }) => rejected(errors, warnings),
        Err(error) => rejected(vec![error.to_string()], Vec::new()),
    }
}

/// Renders data against a published template and returns the finished PDF.
///
/// Document-specific work belongs here: schema validation, Factur-X generation,
/// PDF/A selection and `ZUGFeRD` metadata are one pipeline and are deliberately
/// hidden from the HTTP layer.
pub async fn render(request: RenderRequest, limits: Limits) -> Result<RenderedDocument, Error> {
    let document = request.document;

    // An invoice is not just a rendered template: the data has to hold up as an
    // invoice before anything is produced from it, and the XML is generated
    // from the same data the page is drawn from, so the two cannot disagree.
    let xml = match document.as_ref() {
        None => None,
        Some(spec) => {
            let DocumentKind::Invoice = spec.kind;

            let issuer: Issuer = match request.files.content.get(FIXTURE) {
                Some(bytes) => serde_json::from_slice(bytes).map_err(|error| {
                    // The template's own file, so this is our problem to fix,
                    // not something the caller can do anything about.
                    Error::Internal(format!("`{FIXTURE}` in the template: {error}"))
                })?,
                None => {
                    return Err(Error::Internal(format!(
                        "the template carries no `{FIXTURE}`"
                    )));
                }
            };

            let invoice: Invoice = serde_json::from_value(request.data.clone())
                .map_err(|error| Error::InvalidData(vec![error.to_string()]))?;

            invoice
                .validate(&issuer, spec.profile)
                .map_err(|error| Error::InvalidData(error.problems))?;

            Some(factur_x::generate(&issuer, &invoice, spec.profile))
        }
    };

    // An invoice is always archival: a hybrid invoice has to be PDF/A-3 to
    // carry its attachment, and a document that has to hold up later cannot be
    // stamped with the current time.
    let archival = request.archival || document.is_some();

    let options = if archival {
        RenderOptions {
            timestamp: Some(0),
            standard: Pdf::A3b,
        }
    } else {
        RenderOptions {
            timestamp: None,
            standard: Pdf::Plain,
        }
    };

    let job = Job {
        files: request.files,
        data: Some(request.data),
        xml,
        options,
    };

    let pdf = produce(&job, limits).await?;

    // Keep metadata finalization as its own boundary. Besides being the natural
    // separation between renderer output and post-processing, it lets core unit
    // tests exercise metadata failures without going through HTTP.
    finalize_pdf(pdf, document.as_ref()).map(|pdf| RenderedDocument { pdf, archival })
}

/// Runs a job through the sandbox and normalizes engine failures for callers of
/// the core facade.
async fn produce(job: &Job, limits: Limits) -> Result<Vec<u8>, Error> {
    match compile(job, limits).await {
        Ok(document) => Ok(document.pdf),
        Err(CompileError::Template { errors, .. }) => Err(Error::TemplateFailed(errors)),
        Err(error) => Err(Error::Internal(error.to_string())),
    }
}

/// Applies document-specific post-processing to an already rendered PDF.
fn finalize_pdf(pdf: Vec<u8>, document: Option<&DocumentSpec>) -> Result<Vec<u8>, Error> {
    match document {
        Some(spec) => zugferd::add_metadata(&pdf, &zugferd::Zugferd::new(spec.profile))
            .map_err(|error| Error::Internal(error.to_string())),
        None => Ok(pdf),
    }
}

#[cfg(test)]
mod tests;
