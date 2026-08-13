//! HTTP intake and responses for the document service.
//!
//! Two render paths are exposed, and the difference between them is the single
//! question "does this have to be reproducible in ten years?"
//!
//!   POST /render                            ad hoc: source AND data in the
//!                                           request, nothing stored. Previews,
//!                                           live reload in an editor, trying
//!                                           things out. Arbitrarily dynamic.
//!
//!   POST /templates/{id}                    publish, gated by validation
//!   POST /templates/{id}/{version}/render   data only, against a published
//!                                           template. Everything archived
//!                                           goes here.
//!
//!   GET  /templates                         what is published
//!   GET  /templates/{id}/{version}          the manifest of one version
//!
//! Transport stays here. Rendering, validation and document-specific work live
//! behind the core facade.

use std::collections::BTreeMap;

use axum::{
    Json,
    body::Bytes,
    extract::{FromRequest, Multipart, Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::{
    core::{self, DocumentSpec, Files, MAX_FILE_BYTES, MAX_FILES},
    server::runtime::ServerState,
    storage::{Manifest, Version, VersionSummary},
};

// ---------------------------------------------------------------------------
// Multipart intake
//
//   curl -X POST localhost:8080/templates/example \
//     -F 'fixture=@assets/templates/example/fixture.json' \
//     -F 'file=@assets/templates/example/main.typ' \
//     -F 'file=@assets/templates/example/logo.png'
//
// Fixed names carry the metadata, `file` repeats for content. The virtual path
// comes from each part's filename, so `#import "brand.typ"` still resolves and
// subdirectories survive. `entrypoint` may be given as a field; it defaults to
// `main.typ`.
// ---------------------------------------------------------------------------

/// Part name that carries file content. Everything else is metadata.
const FILE_FIELD: &str = "file";

/// A parsed multipart body, split into metadata and file content.
struct Form {
    fields: BTreeMap<String, Vec<u8>>,
    files: BTreeMap<String, Vec<u8>>,
}

impl Form {
    /// Reads a multipart body into fields and files.
    ///
    /// The body limit layer caps the request as a whole, but parts arrive as a
    /// stream: one request could still carry ten thousand tiny files. The count
    /// and per-file ceilings from [`Files`] apply here, so an upload and a
    /// directory read accept the same thing.
    async fn read(request: Request) -> Result<Self, ErrorResponse> {
        let mut multipart = Multipart::from_request(request, &())
            .await
            .map_err(|rejection| ErrorResponse::BadRequest(rejection.to_string()))?;

        let mut fields = BTreeMap::new();
        let mut files = BTreeMap::new();

        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|error| ErrorResponse::BadRequest(error.to_string()))?
        {
            // Both borrow from `field`, which `bytes()` consumes.
            let name = field.name().unwrap_or_default().to_owned();
            let filename = field.file_name().map(ToOwned::to_owned);

            let bytes = field
                .bytes()
                .await
                .map_err(|error| ErrorResponse::BadRequest(error.to_string()))?;

            if name != FILE_FIELD {
                fields.insert(name, bytes.to_vec());
                continue;
            }

            // Client-controlled path. `Files::validate()` rejects absolute
            // paths and `..` in core -- that check is not optional.
            let path = filename.ok_or_else(|| {
                ErrorResponse::BadRequest("file part without a filename".to_owned())
            })?;

            if files.len() >= MAX_FILES {
                return Err(ErrorResponse::BadRequest(format!(
                    "too many files, limit is {MAX_FILES}"
                )));
            }

            if bytes.len() as u64 > MAX_FILE_BYTES {
                return Err(ErrorResponse::BadRequest(format!(
                    "`{path}` exceeds the per-file limit of {MAX_FILE_BYTES} bytes"
                )));
            }

            files.insert(path, bytes.to_vec());
        }

        Ok(Self { fields, files })
    }

    fn json(&self, name: &str) -> Result<serde_json::Value, ErrorResponse> {
        serde_json::from_slice(self.field(name)?)
            .map_err(|error| ErrorResponse::BadRequest(format!("field `{name}`: {error}")))
    }

    fn field(&self, name: &str) -> Result<&Vec<u8>, ErrorResponse> {
        self.fields
            .get(name)
            .ok_or_else(|| ErrorResponse::BadRequest(format!("missing field `{name}`")))
    }

    /// Consumes the form into the file set the core renders from.
    fn into_files(self, entrypoint: String) -> Files {
        Files {
            entrypoint,
            content: self.files,
        }
    }
}

// ---------------------------------------------------------------------------
// Ad hoc render
// ---------------------------------------------------------------------------

pub struct AdhocRequest {
    pub files: Files,
    pub data: serde_json::Value,
}

impl<S: Send + Sync> FromRequest<S> for AdhocRequest {
    type Rejection = ErrorResponse;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let form = Form::read(request).await?;
        let entrypoint = form_entrypoint(&form)?;
        let data = form.json("data")?;

        Ok(Self {
            data,
            files: form.into_files(entrypoint),
        })
    }
}

/// Preview path. No storage, no version, no guarantees beyond "here is a PDF".
pub async fn render_adhoc(
    State(state): State<ServerState>,
    request: AdhocRequest,
) -> Result<Response, ErrorResponse> {
    let pdf = core::render_adhoc(request.files, request.data, state.limits).await?;
    Ok(pdf_response(Bytes::from(pdf)))
}

// ---------------------------------------------------------------------------
// Publish
// ---------------------------------------------------------------------------

/// A template is published together with the fixture it is validated against.
///
/// No fixture, no publish -- otherwise "it compiles" is the only guarantee you
/// have, and that is not much of one.
pub struct PublishRequest {
    pub files: Files,
    /// Representative sample data. Should exercise the awkward cases: long
    /// names, many line items, zero-rated items, reverse charge.
    pub fixture: serde_json::Value,
}

impl<S: Send + Sync> FromRequest<S> for PublishRequest {
    type Rejection = ErrorResponse;

    async fn from_request(request: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let form = Form::read(request).await?;
        let entrypoint = form_entrypoint(&form)?;
        let fixture = form.json("fixture")?;

        Ok(Self {
            fixture,
            files: form.into_files(entrypoint),
        })
    }
}

#[derive(Serialize)]
pub struct PublishResponse {
    pub template_id: String,
    pub content_hash: String,
    pub accepted: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<Version>,

    /// Omitted when empty: a successful publish should not carry two empty
    /// arrays that a reader has to check before ignoring them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// Validate first, store only on success. Diagnostics come back either way, so
/// the caller can fix the template without guessing.
///
/// A rejected template is the caller's input, not a server fault, so it answers
/// 422 -- the status alone is enough to branch on, without parsing the body.
pub async fn publish_template(
    State(state): State<ServerState>,
    Path(template_id): Path<String>,
    request: PublishRequest,
) -> (StatusCode, Json<PublishResponse>) {
    let validation = core::validate_template(&request.files, &request.fixture, state.limits).await;

    let mut response = PublishResponse {
        template_id: template_id.clone(),
        content_hash: validation.content_hash,
        accepted: validation.accepted,
        version: None,
        errors: validation.errors,
        warnings: validation.warnings,
    };

    if !response.accepted {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(response));
    }

    // Storing can fail after the template already passed validation. That is a
    // server fault, not a bad template, so it must not come back as a rejection
    // -- `accepted: false` would tell the caller to go fix their Typst.
    match state
        .storage
        .publish(&template_id, &request.files, request.fixture)
        .await
    {
        Ok(version) => {
            response.version = Some(version);
            (StatusCode::CREATED, Json(response))
        }
        Err(error) => {
            tracing::error!(%error, template_id, "publish failed after validation");
            response.accepted = false;
            response.errors = vec![error.to_string()];
            (StatusCode::INTERNAL_SERVER_ERROR, Json(response))
        }
    }
}

// ---------------------------------------------------------------------------
// Published render
//
// JSON, because nothing here is a file: the template is already stored, only
// the data of this one document comes in.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PublishedRenderRequest {
    pub data: serde_json::Value,
    /// Set for anything that will be archived. Fixed timestamp + PDF/A-3.
    #[serde(default)]
    pub archival: bool,
    /// What kind of document this is meant to be.
    ///
    /// Absent means "just render the template": no schema, no XML, no claims
    /// about the result beyond it being a PDF.
    #[serde(default)]
    pub document: Option<DocumentSpec>,
}

pub async fn render(
    State(state): State<ServerState>,
    Path((template_id, version)): Path<(String, Version)>,
    Json(request): Json<PublishedRenderRequest>,
) -> Result<Response, ErrorResponse> {
    let (_manifest, files) = state
        .storage
        .get(&template_id, version)
        .await
        .map_err(|error| ErrorResponse::NotFound(error.to_string()))?;

    let document = core::render(
        core::RenderRequest {
            files: (*files).clone(),
            data: request.data,
            archival: request.archival,
            document: request.document,
        },
        state.limits,
    )
    .await?;

    if document.archival {
        state
            .storage
            .put_artifact(&template_id, version, &document.pdf)
            .await
            .map_err(|error| ErrorResponse::Internal(error.to_string()))?;
    }

    Ok(pdf_response(Bytes::from(document.pdf)))

    // What the archival path still owes you, and what a renderer alone will
    // never give you:
    //   - the invoice number, drawn from a gapless sequence
    //   - idempotency, so a retry does not mint a second document
    //   - a receipt stored next to the PDF: data + template version + renderer
    //     build, so the document can be reproduced and defended later
    //   - for ZUGFeRD: the CII XML embedded as an attachment
}

// ---------------------------------------------------------------------------
// Browsing
//
// Both answer from the index, so neither costs an object access. Enough to see
// what is published and to compare a version against a local bundle; the file
// contents themselves are only reachable by rendering.
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct TemplateSummary {
    pub template_id: String,
    pub versions: Vec<VersionSummary>,
}

pub async fn list_templates(State(state): State<ServerState>) -> Json<Vec<TemplateSummary>> {
    Json(
        state
            .storage
            .list()
            .into_iter()
            .map(|(template_id, versions)| TemplateSummary {
                template_id,
                versions,
            })
            .collect(),
    )
}

/// The manifest for one version: what it contains, what produced it, when.
/// File contents are not included -- the manifest carries their hashes.
pub async fn get_template(
    State(state): State<ServerState>,
    Path((template_id, version)): Path<(String, Version)>,
) -> Result<Json<Manifest>, ErrorResponse> {
    state
        .storage
        .manifest(&template_id, version)
        .map(Json)
        .map_err(|error| ErrorResponse::NotFound(error.to_string()))
}

// ---------------------------------------------------------------------------
// Shared
// ---------------------------------------------------------------------------

/// Liveness, readiness and health all answer the same way: the process is up.
/// Nothing behind it can be half-ready, because the port only opens once the
/// engine and the index are known to work.
pub async fn health() -> StatusCode {
    StatusCode::NO_CONTENT
}

/// Default when no entrypoint is named. The overwhelmingly common case, and
/// naming it explicitly stays possible for file sets with several roots.
const DEFAULT_ENTRYPOINT: &str = "main.typ";

/// Resolves the entrypoint and checks it against the uploaded files, so a
/// missing root fails here with a clear message instead of surfacing as a
/// confusing compiler error further down.
fn form_entrypoint(form: &Form) -> Result<String, ErrorResponse> {
    let (name, named) = match form.fields.get("entrypoint") {
        Some(bytes) => (
            String::from_utf8(bytes.clone()).map_err(|_| {
                ErrorResponse::BadRequest("field `entrypoint` is not valid utf-8".to_owned())
            })?,
            true,
        ),
        None => (DEFAULT_ENTRYPOINT.to_owned(), false),
    };

    if form.files.contains_key(&name) {
        Ok(name)
    } else if named {
        Err(ErrorResponse::BadRequest(format!(
            "entrypoint `{name}` is not among the uploaded files"
        )))
    } else {
        Err(ErrorResponse::BadRequest(format!(
            "no entrypoint given and no `{DEFAULT_ENTRYPOINT}` among the uploaded files"
        )))
    }
}

fn pdf_response(pdf: Bytes) -> Response {
    ([(header::CONTENT_TYPE, "application/pdf")], pdf).into_response()
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ErrorResponse {
    BadRequest(String),
    Unauthorized,
    NotFound(String),
    TemplateFailed(Vec<String>),
    Internal(String),
    /// The data is not what this document type requires. Every problem at once,
    /// because fixing an integration one field per request is a guessing game.
    InvalidData(Vec<String>),
}

impl From<core::Error> for ErrorResponse {
    fn from(error: core::Error) -> Self {
        match error {
            core::Error::InvalidFiles(message) => Self::BadRequest(message),
            core::Error::TemplateFailed(errors) => Self::TemplateFailed(errors),
            core::Error::InvalidData(problems) => Self::InvalidData(problems),
            core::Error::Internal(message) => Self::Internal(message),
        }
    }
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, vec![message]),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, vec![message]),
            // The template is the caller's input, so a compile failure is a 422,
            // not a 500.
            Self::TemplateFailed(errors) => (StatusCode::UNPROCESSABLE_ENTITY, errors),
            Self::Internal(message) => (StatusCode::INTERNAL_SERVER_ERROR, vec![message]),
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                vec!["missing or invalid token".to_owned()],
            ),
            Self::InvalidData(problems) => (StatusCode::UNPROCESSABLE_ENTITY, problems),
        };

        (status, Json(serde_json::json!({ "errors": body }))).into_response()
    }
}
