//! S3 backend wiring.
//!
//! Works against AWS S3 and compatible implementations (`MinIO`, Hetzner,
//! Garage) through a configurable endpoint and addressing style.

use object_store::{
    ObjectStore, ObjectStoreExt,
    aws::{AmazonS3, AmazonS3Builder},
    path::Path,
};
use std::{fmt, sync::Arc};

/// Environment variable that decides whether object storage is enabled at all.
/// Everything else has a default or is optional.
const BUCKET_VAR: &str = "DOCUMENT_S3_BUCKET";

#[derive(Clone)]
pub struct Config {
    pub bucket: String,
    pub region: String,
    pub endpoint: Option<String>,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub session_token: Option<String>,
    pub virtual_hosted_style: bool,
    /// Only for local `MinIO`. Never in production.
    pub allow_http: bool,
}

/// Hand-written so credentials never reach a log line. A derived `Debug` would
/// print the secret key verbatim the first time someone traces the startup path.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field(
                "access_key_id",
                &self.access_key_id.as_ref().map(|_| "<set>"),
            )
            .field(
                "secret_access_key",
                &self.secret_access_key.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .field("virtual_hosted_style", &self.virtual_hosted_style)
            .field("allow_http", &self.allow_http)
            .finish()
    }
}

impl Config {
    /// Reads the configuration from the environment.
    ///
    /// Returns `None` when no bucket is set. That is not an error -- it is the
    /// signal that this run has no object storage, which is the normal case for
    /// the CLI and for local previews.
    ///
    /// Anything left unset here still falls back to the standard `AWS_*`
    /// variables via [`AmazonS3Builder::from_env`].
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let bucket = std::env::var(BUCKET_VAR).ok()?;

        Some(Self {
            bucket,
            region: std::env::var("DOCUMENT_S3_REGION").unwrap_or_else(|_| "us-east-1".to_owned()),
            endpoint: std::env::var("DOCUMENT_S3_ENDPOINT").ok(),
            access_key_id: std::env::var("DOCUMENT_S3_ACCESS_KEY_ID").ok(),
            secret_access_key: std::env::var("DOCUMENT_S3_SECRET_ACCESS_KEY").ok(),
            session_token: std::env::var("DOCUMENT_S3_SESSION_TOKEN").ok(),
            virtual_hosted_style: std::env::var("DOCUMENT_S3_VIRTUAL_HOSTED_STYLE").is_ok(),
            allow_http: std::env::var("DOCUMENT_S3_ALLOW_HTTP").is_ok(),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid S3 configuration: access key and secret key must be provided together")]
    IncompleteCredentials,

    #[error("invalid S3 configuration: {0}")]
    Configuration(#[from] object_store::Error),

    #[error("bucket `{bucket}` is not reachable: {source}")]
    Unreachable {
        bucket: String,
        source: object_store::Error,
    },
}

/// Builds an S3 store from `config`.
///
/// This contacts nothing -- credentials and endpoint are only exercised on the
/// first real request. Use [`connect`] if you want to find out at startup.
///
/// # Errors
///
/// Returns [`Error::Configuration`] if the resulting configuration is
/// incomplete or inconsistent.
pub fn build(config: Config) -> Result<Arc<dyn ObjectStore>, Error> {
    if config.access_key_id.is_some() != config.secret_access_key.is_some() {
        return Err(Error::IncompleteCredentials);
    }

    store(config)
        .map(|store| Arc::new(store) as Arc<dyn ObjectStore>)
        .map_err(Error::from)
}

/// Builds the store and verifies the bucket actually answers.
///
/// Without the probe, a typo in the endpoint or a wrong key stays invisible
/// until the first request -- which is the moment someone wants an invoice.
/// This moves that failure to startup, where it is cheap.
///
/// # Errors
///
/// Returns [`Error::Configuration`] on incomplete config, or
/// [`Error::Unreachable`] if the bucket cannot be queried.
pub async fn connect(config: Config) -> Result<Arc<dyn ObjectStore>, Error> {
    let bucket = config.bucket.clone();
    let store = build(config)?;

    probe(store, bucket).await
}

async fn probe(store: Arc<dyn ObjectStore>, bucket: String) -> Result<Arc<dyn ObjectStore>, Error> {
    match store.head(&Path::from(".probe")).await {
        Ok(_) | Err(object_store::Error::NotFound { .. }) => Ok(store),
        Err(source) => Err(Error::Unreachable { bucket, source }),
    }
}

fn store(config: Config) -> Result<AmazonS3, object_store::Error> {
    let mut builder = AmazonS3Builder::from_env()
        .with_bucket_name(config.bucket)
        .with_region(config.region)
        .with_virtual_hosted_style_request(config.virtual_hosted_style)
        .with_allow_http(config.allow_http);

    if let Some(endpoint) = config.endpoint {
        builder = builder.with_endpoint(endpoint);
    }

    if let Some(access_key_id) = config.access_key_id {
        builder = builder.with_access_key_id(access_key_id);
    }

    if let Some(secret_access_key) = config.secret_access_key {
        builder = builder.with_secret_access_key(secret_access_key);
    }

    if let Some(session_token) = config.session_token {
        builder = builder.with_token(session_token);
    }

    builder.build()
}
