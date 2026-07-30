//! Bringing the service up and keeping it up.
//!
//! Startup order is deliberate: render engine, then storage, then the
//! listener. Each stage can abort the start, and the port only opens once
//! everything behind it is known to work -- a broken font or an unreachable
//! bucket fails here, not on the first request someone cares about.

use std::{process::ExitCode, sync::Arc};

use axum::{
    Router,
    extract::{DefaultBodyLimit, Request, State},
    http::header,
    middleware::{Next, from_fn_with_state},
    response::Response,
    routing::{get, post},
};

use crate::{
    core::{self, Limits},
    server::events::{self, ErrorResponse},
    storage::{Storage, StorageError, s3},
};

/// Everything the handlers need. Cheap to clone: the expensive parts are behind
/// `Arc`.
#[derive(Clone)]
pub struct ServerState {
    pub storage: Arc<Storage>,
    pub limits: Limits,
}

/// Bearer token from the environment. Unset means the service stays open --
/// fine behind a private network or a proxy that already authenticates, and a
/// footgun anywhere else, which the README says out loud.
///
/// An empty value counts as unset: otherwise `DOCUMENT_TOKEN=` would accept
/// every request carrying an empty bearer token.
fn auth_token() -> Option<String> {
    std::env::var("DOCUMENT_TOKEN")
        .ok()
        .filter(|token| !token.is_empty())
}

async fn require_token(
    State(expected): State<blake3::Hash>,
    request: Request,
    next: Next,
) -> Result<Response, ErrorResponse> {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or(ErrorResponse::Unauthorized)?;

    // Compared as hashes, not as strings: a byte-wise comparison stops at the
    // first difference, and the timing of that could in principle be used to
    // walk the token out one character at a time. The token itself exists only
    // until startup is done.
    if blake3::hash(presented.as_bytes()) != expected {
        return Err(ErrorResponse::Unauthorized);
    }

    Ok(next.run(request).await)
}

pub fn router(state: ServerState) -> Router {
    // Probes stay open: an orchestrator should not need a credential to find
    // out whether the process is alive.
    let probes = Router::new()
        .route("/health", get(events::health))
        .route("/readiness", get(events::health))
        .route("/liveness", get(events::health));

    let mut api = Router::new()
        .route("/render", post(events::render_adhoc))
        .route("/templates", get(events::list_templates))
        .route("/templates/{id}", post(events::publish_template))
        .route("/templates/{id}/{version}", get(events::get_template))
        .route("/templates/{id}/{version}/render", post(events::render));

    if let Some(token) = auth_token() {
        api = api.layer(from_fn_with_state(
            blake3::hash(token.as_bytes()),
            require_token,
        ));
    }

    probes.merge(api).with_state(state)
}

#[derive(Debug, thiserror::Error)]
enum RuntimeError {
    #[error("render engine unavailable, refusing to start: {0}")]
    Warmup(#[from] core::CompileError),

    #[error("{0}")]
    Server(String),
}

/// Starts the HTTP service and blocks until it stops.
pub fn start(listen: &str) -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter("document=debug,tower_http=debug")
        .init();

    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(serve(listen));

    match result {
        Ok(()) => {
            tracing::info!("document server stopped");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "document server stopped");
            ExitCode::FAILURE
        }
    }
}

/// Runs everything after render-engine warmup.
///
/// Once this function is entered, [`serve`] owns the worker cleanup and every
/// exit from this stage passes through [`core::shutdown`].
async fn bootstrap(listen: &str) -> Result<(), String> {
    let storage = open_storage()
        .await
        .map_err(|error| format!("storage unavailable: {error}"))?;

    // The index has to exist before the port opens: a request arriving against
    // an empty index would answer 404 for a template that is actually there.
    let templates = storage
        .load()
        .await
        .map_err(|error| format!("template index unreadable: {error}"))?;

    tracing::info!(templates, "template index loaded");

    let state = ServerState {
        storage: Arc::new(storage),
        limits: Limits::default(),
    };

    let service = router(state)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        // Bundles arrive as JSON with embedded assets, so the default body
        // limit is too small -- but leaving it unbounded is a denial-of-service
        // knob.
        .layer(DefaultBodyLimit::disable())
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            16 * 1024 * 1024,
        ));

    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|error| format!("failed to bind `{listen}`: {error}"))?;

    // Logging the configured address is sufficient here. Asking the listener
    // for its local address solely for logging would introduce another fallible
    // operation into an otherwise completed startup.
    tracing::info!(
        address = %listen,
        "document server listening"
    );

    axum::serve(listener, service)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("axum server cannot fail");

    Ok(())
}

async fn serve(listen: &str) -> Result<(), RuntimeError> {
    let workers = core::start()?;

    tracing::info!(workers, "render engine ready");

    if auth_token().is_some() {
        tracing::info!("bearer authentication enabled");
    } else {
        tracing::warn!("DOCUMENT_TOKEN unset -- all endpoints are open");
    }

    let result = bootstrap(listen).await.map_err(RuntimeError::Server);

    core::shutdown().await;

    result
}

/// Where templates and archived documents live.
///
/// A bucket wins when configured, then a data directory, otherwise memory. A
/// bucket that *is* configured but unreachable is a hard failure: starting up
/// anyway and only finding out when someone wants an invoice archived is the
/// worse outcome.
async fn open_storage() -> Result<Storage, StorageError> {
    if let Some(config) = s3::Config::from_env() {
        let bucket = config.bucket.clone();
        let objects = s3::connect(config).await?;

        tracing::info!(%bucket, "object storage enabled");
        return Ok(Storage::s3(objects));
    }

    if let Ok(directory) = std::env::var("DOCUMENT_DATA_DIR") {
        let storage = Storage::local(directory.as_ref())?;

        tracing::info!(directory, "local storage enabled");
        return Ok(storage);
    }

    // Louder than the others: `archival: true` will now appear to succeed and
    // then vanish on restart.
    tracing::warn!("no storage configured -- nothing survives a restart");

    Ok(Storage::memory())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");

        "SIGINT"
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;

        "SIGTERM"
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<&'static str>();

    let signal = tokio::select! {
        signal = ctrl_c => {
            eprintln!();
            signal
        }
        signal = terminate => signal,
    };

    tracing::info!(signal, "shutdown signal received");
}
