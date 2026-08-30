use std::{
    collections::BTreeMap,
    fmt, fs,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use futures_util::{StreamExt, stream::BoxStream};
use object_store::{
    CopyOptions, GetOptions, GetResult, GetResultPayload, ListResult, MultipartUpload, ObjectMeta,
    ObjectStore, ObjectStoreExt, PutMode, PutMultipartOptions, PutOptions, PutPayload, PutResult,
    memory::InMemory, path::Path as ObjectPath,
};

use super::*;

// -----------------------------------------------------------------------------
// Fixtures
// -----------------------------------------------------------------------------

fn files(body: &[u8]) -> Files {
    Files {
        entrypoint: "main.typ".to_owned(),
        content: BTreeMap::from([("main.typ".to_owned(), body.to_vec())]),
    }
}

fn multi_files() -> Files {
    Files {
        entrypoint: "main.typ".to_owned(),
        content: BTreeMap::from([
            ("main.typ".to_owned(), b"#include \"part.typ\"".to_vec()),
            ("part.typ".to_owned(), b"Hello".to_vec()),
        ]),
    }
}

static TEMP_ID: AtomicUsize = AtomicUsize::new(0);

fn temp_path(name: &str) -> PathBuf {
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);

    std::env::temp_dir().join(format!(
        "document-storage-{}-{}-{id}",
        std::process::id(),
        name
    ))
}

fn injected_error(operation: &'static str) -> object_store::Error {
    object_store::Error::Generic {
        store: "test",
        source: std::io::Error::other(format!("injected {operation} failure")).into(),
    }
}

fn already_exists(path: &ObjectPath) -> object_store::Error {
    object_store::Error::AlreadyExists {
        path: path.to_string(),
        source: std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "injected version collision",
        )
        .into(),
    }
}

// -----------------------------------------------------------------------------
// Fault-injecting object store
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default)]
enum Fault {
    #[default]
    None,

    /// The first conditional manifest write loses a race. The object is actually
    /// committed to the inner store first so `Storage::load()` sees the version
    /// supposedly published by the competing process.
    ManifestAlreadyExistsOnce,

    /// Every conditional manifest write loses. Used to exhaust `MAX_ATTEMPTS`.
    ManifestAlreadyExistsAlways,

    /// Conditional manifest writes fail with a generic backend error.
    ManifestPut,

    /// Template file writes fail before the manifest is attempted.
    TemplateFilePut,

    /// Artifact writes fail.
    ArtifactPut,

    /// Listing fails.
    List,

    /// Reads fail.
    Get,

    /// The manifest write loses a race and the following index reload fails.
    CollisionThenList,

    /// `get()` succeeds, but consuming the returned body fails.
    Body,
}

#[derive(Debug)]
struct FaultStore {
    inner: InMemory,
    fault: Fault,
    manifest_attempts: AtomicUsize,
}

impl FaultStore {
    fn new(fault: Fault) -> Self {
        Self {
            inner: InMemory::new(),
            fault,
            manifest_attempts: AtomicUsize::new(0),
        }
    }

    fn is_manifest(location: &ObjectPath) -> bool {
        location.filename() == Some("manifest.json")
    }

    fn is_template_file(location: &ObjectPath) -> bool {
        let path = location.to_string();

        path.starts_with("templates/") && path.contains("/files/")
    }

    fn is_artifact(location: &ObjectPath) -> bool {
        location.to_string().starts_with("artifacts/")
    }
}

impl fmt::Display for FaultStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("fault-injecting object store")
    }
}

#[async_trait]
impl ObjectStore for FaultStore {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        if Self::is_manifest(location) && opts.mode == PutMode::Create {
            match self.fault {
                Fault::ManifestAlreadyExistsOnce => {
                    let attempt = self.manifest_attempts.fetch_add(1, Ordering::SeqCst);

                    if attempt == 0 {
                        // Simulate another publisher winning exactly this version:
                        // the manifest exists by the time `Storage::load()` runs.
                        self.inner.put_opts(location, payload, opts).await?;

                        return Err(already_exists(location));
                    }
                }

                Fault::ManifestAlreadyExistsAlways => {
                    self.manifest_attempts.fetch_add(1, Ordering::SeqCst);

                    return Err(already_exists(location));
                }

                Fault::ManifestPut => {
                    return Err(injected_error("manifest put"));
                }

                Fault::CollisionThenList => {
                    return Err(already_exists(location));
                }

                Fault::None
                | Fault::TemplateFilePut
                | Fault::ArtifactPut
                | Fault::List
                | Fault::Get
                | Fault::Body => {}
            }
        }

        if matches!(self.fault, Fault::TemplateFilePut) && Self::is_template_file(location) {
            return Err(injected_error("template file put"));
        }

        if matches!(self.fault, Fault::ArtifactPut) && Self::is_artifact(location) {
            return Err(injected_error("artifact put"));
        }

        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &ObjectPath,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        if matches!(self.fault, Fault::Get) {
            return Err(injected_error("get"));
        }

        let result = self.inner.get_opts(location, options).await?;

        if matches!(self.fault, Fault::Body) {
            return Ok(GetResult {
                payload: GetResultPayload::Stream(
                    futures_util::stream::once(async { Err(injected_error("body")) }).boxed(),
                ),
                meta: result.meta,
                range: result.range,
                attributes: result.attributes,
                extensions: result.extensions,
            });
        }

        Ok(result)
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<ObjectPath>>,
    ) -> BoxStream<'static, object_store::Result<ObjectPath>> {
        self.inner.delete_stream(locations)
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        if matches!(self.fault, Fault::List | Fault::CollisionThenList) {
            return futures_util::stream::once(async { Err(injected_error("list")) }).boxed();
        }

        self.inner.list(prefix)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> object_store::Result<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &ObjectPath,
        to: &ObjectPath,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.inner.copy_opts(from, to, options).await
    }
}

// -----------------------------------------------------------------------------
// Construction
// -----------------------------------------------------------------------------

#[test]
fn memory_starts_empty() {
    let storage = Storage::memory();

    assert!(storage.list().is_empty());
    assert_eq!(storage.latest("invoice"), None);
}

#[test]
fn create_local_directory_creates_directory() {
    let root = temp_path("create-directory");

    create_local_directory(&root).expect("create local directory");

    assert!(root.is_dir());

    fs::remove_dir_all(root).expect("remove test directory");
}

#[test]
fn create_local_directory_rejects_invalid_parent() {
    let parent = temp_path("invalid-parent");

    fs::write(&parent, b"not a directory").expect("create parent file");

    let root = parent.join("storage");
    let result = create_local_directory(&root);

    assert!(matches!(
        result,
        Err(StorageError::LocalDirectory { ref path, .. }) if path == &root
    ));

    fs::remove_file(parent).expect("remove parent file");
}

#[test]
fn open_local_storage_opens_existing_directory() {
    let root = temp_path("open-store");
    fs::create_dir_all(&root).expect("create directory");

    let result = open_local_storage(&root);

    assert!(result.is_ok());

    fs::remove_dir_all(root).expect("remove test directory");
}

#[test]
fn open_local_storage_rejects_missing_directory() {
    let root = temp_path("missing-store");

    let result = open_local_storage(&root);

    assert!(matches!(
        result,
        Err(StorageError::LocalBackend { ref path, .. }) if path == &root
    ));
}

#[tokio::test]
async fn local_creates_and_opens_directory() {
    let root = temp_path("local");

    let storage = Storage::local(&root).expect("open local storage");

    assert_eq!(storage.load().await.expect("load"), 0);
    assert!(root.is_dir());

    fs::remove_dir_all(root).expect("remove local storage directory");
}

#[test]
fn local_rejects_file_as_directory() {
    let root = temp_path("file");

    fs::write(&root, b"not a directory").expect("create test file");

    let result = Storage::local(&root);

    assert!(matches!(
        result,
        Err(StorageError::LocalDirectory { ref path, .. }) if path == &root
    ));

    fs::remove_file(root).expect("remove test file");
}

#[test]
fn s3_config_debug_redacts_credentials() {
    let config = s3::Config {
        bucket: "document-test".to_owned(),
        region: "eu-central-1".to_owned(),
        endpoint: Some("https://s3.example.com".to_owned()),
        access_key_id: Some("visible-access-key-must-not-leak".to_owned()),
        secret_access_key: Some("visible-secret-must-not-leak".to_owned()),
        session_token: Some("visible-token-must-not-leak".to_owned()),
        virtual_hosted_style: true,
        allow_http: false,
    };

    let debug = format!("{config:?}");

    assert!(debug.contains("Config"));
    assert!(debug.contains("document-test"));
    assert!(debug.contains("eu-central-1"));
    assert!(debug.contains("https://s3.example.com"));
    assert!(debug.contains("access_key_id"));
    assert!(debug.contains("secret_access_key"));
    assert!(debug.contains("session_token"));
    assert!(debug.contains("virtual_hosted_style"));
    assert!(debug.contains("allow_http"));

    assert!(!debug.contains("visible-access-key-must-not-leak"));
    assert!(!debug.contains("visible-secret-must-not-leak"));
    assert!(!debug.contains("visible-token-must-not-leak"));
}

#[test]
fn s3_build_accepts_session_token() {
    let config = s3::Config {
        bucket: "document-test".to_owned(),
        region: "us-east-1".to_owned(),
        endpoint: Some("http://127.0.0.1:9000".to_owned()),
        access_key_id: Some("access".to_owned()),
        secret_access_key: Some("secret".to_owned()),
        session_token: Some("session".to_owned()),
        virtual_hosted_style: false,
        allow_http: true,
    };
    s3::build(config)
        .unwrap_or_else(|error| panic!("failed to build S3 client with session token: {error:#?}"));
}

#[test]
fn s3_build_accepts_minimal_config() {
    let config = s3::Config {
        bucket: "document-test".to_owned(),
        region: "us-east-1".to_owned(),
        endpoint: None,
        access_key_id: None,
        secret_access_key: None,
        session_token: None,
        virtual_hosted_style: false,
        allow_http: false,
    };

    let result = s3::build(config);

    assert!(result.is_ok());
}

#[test]
fn s3_build_rejects_incomplete_credentials() {
    let config = s3::Config {
        bucket: "document-test".to_owned(),
        region: "us-east-1".to_owned(),
        endpoint: None,
        access_key_id: Some("access".to_owned()),
        secret_access_key: None,
        session_token: None,
        virtual_hosted_style: false,
        allow_http: false,
    };

    match s3::build(config) {
        Err(s3::Error::IncompleteCredentials) => {}
        Err(error) => panic!("expected IncompleteCredentials, got: {error:#?}"),
        Ok(_) => panic!("expected IncompleteCredentials, but S3 client was built"),
    }
}

#[tokio::test]
async fn s3_connect_reports_unreachable_store() {
    let config = s3::Config {
        bucket: "document-test".to_owned(),
        region: "us-east-1".to_owned(),
        endpoint: Some("http://127.0.0.1:1".to_owned()),
        access_key_id: Some("access".to_owned()),
        secret_access_key: Some("secret".to_owned()),
        session_token: None,
        virtual_hosted_style: false,
        allow_http: true,
    };

    match s3::connect(config).await {
        Err(s3::Error::Unreachable { .. }) => {}
        Err(error) => panic!("expected Unreachable, got: {error:#?}"),
        Ok(_) => panic!("expected Unreachable, but connection succeeded"),
    }
}

// -----------------------------------------------------------------------------
// Manifest
// -----------------------------------------------------------------------------

#[test]
fn manifest_contains_complete_metadata() {
    let files = multi_files();
    let fixture = serde_json::json!({
        "language": "de",
        "customer": "Example"
    });

    let manifest = Manifest::new("invoice", 7, &files, fixture.clone());

    assert_eq!(manifest.template_id, "invoice");
    assert_eq!(manifest.version, 7);
    assert_eq!(manifest.entrypoint, "main.typ");
    assert_eq!(manifest.content_hash, files.hash().to_string());
    assert_eq!(manifest.fixture, fixture);
    assert_eq!(manifest.renderer, RENDERER);
    assert_eq!(manifest.files.len(), 2);
    assert!(!manifest.created_at.is_empty());

    for (path, bytes) in &files.content {
        assert_eq!(
            manifest.files.get(path),
            Some(&blake3::hash(bytes).to_hex().to_string())
        );
    }
}

// -----------------------------------------------------------------------------
// Publishing
// -----------------------------------------------------------------------------

#[tokio::test]
async fn publish_starts_at_one_and_increments() {
    let storage = Storage::memory();

    let first = storage
        .publish("invoice", &files(b"one"), serde_json::json!({"n": 1}))
        .await
        .expect("publish v1");

    let second = storage
        .publish("invoice", &files(b"two"), serde_json::json!({"n": 2}))
        .await
        .expect("publish v2");

    assert_eq!(first, 1);
    assert_eq!(second, 2);
    assert_eq!(storage.latest("invoice"), Some(2));
    assert_eq!(storage.latest("unknown"), None);
}

#[tokio::test]
async fn identical_publish_is_idempotent() {
    let storage = Storage::memory();
    let files = files(b"same");

    let first = storage
        .publish("invoice", &files, serde_json::json!({"attempt": 1}))
        .await
        .expect("first publish");

    let second = storage
        .publish("invoice", &files, serde_json::json!({"attempt": 2}))
        .await
        .expect("second publish");

    assert_eq!(first, 1);
    assert_eq!(second, 1);
    assert_eq!(storage.latest("invoice"), Some(1));

    let manifest = storage.manifest("invoice", 1).expect("manifest");

    // Republishing identical bytes must not mutate the existing fixture either.
    assert_eq!(manifest.fixture, serde_json::json!({"attempt": 1}));
}

#[tokio::test]
async fn templates_are_versioned_independently() {
    let storage = Storage::memory();

    assert_eq!(
        storage
            .publish("invoice", &files(b"a"), serde_json::Value::Null)
            .await
            .expect("invoice v1"),
        1
    );

    assert_eq!(
        storage
            .publish("letter", &files(b"b"), serde_json::Value::Null)
            .await
            .expect("letter v1"),
        1
    );

    assert_eq!(
        storage
            .publish("invoice", &files(b"c"), serde_json::Value::Null)
            .await
            .expect("invoice v2"),
        2
    );
}

#[tokio::test]
async fn publish_recovers_from_version_collision() {
    let objects: Arc<dyn ObjectStore> = Arc::new(FaultStore::new(Fault::ManifestAlreadyExistsOnce));

    let storage = Storage::s3(objects);

    let version = storage
        .publish(
            "invoice",
            &files(b"ours"),
            serde_json::json!({"publisher": "ours"}),
        )
        .await
        .expect("publish after collision");

    // Attempt 1 writes v1 on behalf of the simulated competing publisher,
    // returns AlreadyExists, reloads the index, and retries as v2.
    assert_eq!(version, 2);
    assert_eq!(storage.latest("invoice"), Some(2));

    assert!(storage.manifest("invoice", 1).is_ok());
    assert!(storage.manifest("invoice", 2).is_ok());
}

#[tokio::test]
async fn publish_propagates_reload_error_after_collision() {
    let objects: Arc<dyn ObjectStore> = Arc::new(FaultStore::new(Fault::CollisionThenList));

    let storage = Storage::s3(objects);

    let result = storage
        .publish("invoice", &files(b"x"), serde_json::Value::Null)
        .await;

    assert!(matches!(result, Err(StorageError::Backend(_))));
}

#[tokio::test]
async fn publish_propagates_manifest_backend_error() {
    let objects: Arc<dyn ObjectStore> = Arc::new(FaultStore::new(Fault::ManifestPut));
    let storage = Storage::s3(objects);

    let result = storage
        .publish("invoice", &files(b"x"), serde_json::Value::Null)
        .await;

    assert!(matches!(result, Err(StorageError::Backend(_))));
}

#[tokio::test]
async fn publish_propagates_template_file_backend_error() {
    let objects: Arc<dyn ObjectStore> = Arc::new(FaultStore::new(Fault::TemplateFilePut));

    let storage = Storage::s3(objects);

    let result = storage
        .publish("invoice", &files(b"x"), serde_json::Value::Null)
        .await;

    assert!(matches!(result, Err(StorageError::Backend(_))));
}

#[tokio::test]
async fn publish_reports_contention_after_max_attempts() {
    let objects = Arc::new(FaultStore::new(Fault::ManifestAlreadyExistsAlways));
    let storage = Storage::s3(objects.clone());

    let result = storage
        .publish("invoice", &files(b"x"), serde_json::Value::Null)
        .await;

    assert!(matches!(
        result,
        Err(StorageError::Contention(ref id)) if id == "invoice"
    ));

    assert_eq!(
        objects.manifest_attempts.load(Ordering::SeqCst),
        MAX_ATTEMPTS as usize
    );
}

// -----------------------------------------------------------------------------
// Loading index
// -----------------------------------------------------------------------------

#[tokio::test]
async fn load_skips_non_manifest_objects() {
    let storage = Storage::memory();

    storage
        .objects
        .put(
            &ObjectPath::from("templates/invoice/files/hash/main.typ"),
            PutPayload::from_static(b"hello"),
        )
        .await
        .expect("put ordinary template file");

    assert_eq!(storage.load().await.expect("load"), 0);
    assert!(storage.list().is_empty());
}

#[tokio::test]
async fn load_rejects_corrupt_manifest() {
    let storage = Storage::memory();
    let location = manifest_path("invoice", 1);

    storage
        .objects
        .put(&location, PutPayload::from_static(b"not json"))
        .await
        .expect("put corrupt manifest");

    let result = storage.load().await;

    assert!(matches!(
        result,
        Err(StorageError::Corrupt { ref path, .. }) if path == &location.to_string()
    ));
}

#[tokio::test]
async fn load_rebuilds_index() {
    let objects: Arc<dyn ObjectStore> = Arc::new(InMemory::new());

    let writer = Storage::s3(Arc::clone(&objects));

    writer
        .publish("invoice", &files(b"one"), serde_json::json!({"v": 1}))
        .await
        .expect("publish v1");

    writer
        .publish("invoice", &files(b"two"), serde_json::json!({"v": 2}))
        .await
        .expect("publish v2");

    writer
        .publish("letter", &files(b"three"), serde_json::json!({"v": 1}))
        .await
        .expect("publish letter");

    let reader = Storage::s3(objects);

    assert!(reader.list().is_empty());

    assert_eq!(reader.load().await.expect("load"), 3);
    assert_eq!(reader.latest("invoice"), Some(2));
    assert_eq!(reader.latest("letter"), Some(1));
}

#[tokio::test]
async fn load_propagates_listing_error() {
    let objects: Arc<dyn ObjectStore> = Arc::new(FaultStore::new(Fault::List));
    let storage = Storage::s3(objects);

    let result = storage.load().await;

    assert!(matches!(result, Err(StorageError::Backend(_))));
}

#[tokio::test]
async fn load_propagates_manifest_read_error() {
    let fault = FaultStore::new(Fault::Get);

    let files = files(b"x");
    let manifest = Manifest::new("invoice", 1, &files, serde_json::Value::Null);

    fault
        .inner
        .put(
            &manifest_path("invoice", 1),
            PutPayload::from(serde_json::to_vec(&manifest).expect("encode manifest")),
        )
        .await
        .expect("seed manifest");

    let storage = Storage::s3(Arc::new(fault));

    let result = storage.load().await;

    assert!(matches!(result, Err(StorageError::Backend(_))));
}

#[tokio::test]
async fn load_propagates_manifest_body_error() {
    let fault = FaultStore::new(Fault::Body);

    let files = files(b"x");
    let manifest = Manifest::new("invoice", 1, &files, serde_json::Value::Null);

    fault
        .inner
        .put(
            &manifest_path("invoice", 1),
            PutPayload::from(serde_json::to_vec(&manifest).expect("encode manifest")),
        )
        .await
        .expect("seed manifest");

    let storage = Storage::s3(Arc::new(fault));

    let result = storage.load().await;

    assert!(matches!(result, Err(StorageError::Backend(_))));
}

// -----------------------------------------------------------------------------
// Get / cache / integrity
// -----------------------------------------------------------------------------

#[test]
fn manifest_rejects_unknown_version() {
    let storage = Storage::memory();

    let result = storage.manifest("invoice", 42);

    assert!(matches!(
        result,
        Err(StorageError::NotFound(ref id, 42)) if id == "invoice"
    ));
}

#[tokio::test]
async fn get_rejects_unknown_version() {
    let storage = Storage::memory();

    let result = storage.get("invoice", 42).await;

    assert!(matches!(
        result,
        Err(StorageError::NotFound(ref id, 42)) if id == "invoice"
    ));
}

#[tokio::test]
async fn get_fetches_published_files() {
    let storage = Storage::memory();
    let expected = multi_files();

    storage
        .publish("invoice", &expected, serde_json::json!({"language": "de"}))
        .await
        .expect("publish");

    let (manifest, actual) = storage.get("invoice", 1).await.expect("get");

    assert_eq!(manifest.template_id, "invoice");
    assert_eq!(manifest.version, 1);
    assert_eq!(actual.entrypoint, expected.entrypoint);
    assert_eq!(actual.content, expected.content);
}

#[tokio::test]
async fn get_uses_cache_after_first_fetch() {
    let storage = Storage::memory();
    let expected = multi_files();

    storage
        .publish("invoice", &expected, serde_json::Value::Null)
        .await
        .expect("publish");

    let (_, first) = storage.get("invoice", 1).await.expect("first get");

    let manifest = storage.manifest("invoice", 1).expect("manifest");

    for path in manifest.files.keys() {
        let location = ObjectPath::from(format!(
            "templates/{}/files/{}/{path}",
            manifest.template_id, manifest.content_hash
        ));

        storage
            .objects
            .delete(&location)
            .await
            .expect("delete stored file");
    }

    // The backing objects are gone. Success therefore proves this came from
    // the LRU cache rather than fetch_files().
    let (_, second) = storage.get("invoice", 1).await.expect("cached get");

    assert_eq!(first.content, second.content);
    assert!(Arc::ptr_eq(&first, &second));
}

#[tokio::test]
async fn get_detects_tampered_file() {
    let storage = Storage::memory();
    let files = files(b"original");

    storage
        .publish("invoice", &files, serde_json::Value::Null)
        .await
        .expect("publish");

    let manifest = storage.manifest("invoice", 1).expect("manifest");

    let location = ObjectPath::from(format!(
        "templates/{}/files/{}/main.typ",
        manifest.template_id, manifest.content_hash
    ));

    storage
        .objects
        .put(&location, PutPayload::from_static(b"tampered"))
        .await
        .expect("tamper stored file");

    let result = storage.get("invoice", 1).await;

    assert!(matches!(
        result,
        Err(StorageError::Tampered { ref path }) if path == &location.to_string()
    ));
}

#[tokio::test]
async fn get_propagates_backend_read_error() {
    let fault = FaultStore::new(Fault::Get);
    let expected = multi_files();

    // Seed objects and manifest directly through the inner store because all
    // reads from the wrapper are intentionally broken.
    let content_hash = expected.hash().to_string();

    for (path, bytes) in &expected.content {
        let location = ObjectPath::from(format!("templates/invoice/files/{content_hash}/{path}"));

        fault
            .inner
            .put(&location, PutPayload::from(bytes.clone()))
            .await
            .expect("seed template file");
    }

    let manifest = Manifest::new("invoice", 1, &expected, serde_json::Value::Null);

    fault
        .inner
        .put(
            &manifest_path("invoice", 1),
            PutPayload::from(serde_json::to_vec(&manifest).expect("encode manifest")),
        )
        .await
        .expect("seed manifest");

    let storage = Storage::s3(Arc::new(fault));

    // load() would use the deliberately failing get(), so seed the in-memory
    // index exactly as a successful startup load would have done.
    storage
        .index
        .write()
        .expect("index lock")
        .insert(("invoice".to_owned(), 1), manifest);

    let result = storage.get("invoice", 1).await;

    assert!(matches!(result, Err(StorageError::Backend(_))));
}

#[tokio::test]
async fn get_propagates_template_body_error() {
    let fault = FaultStore::new(Fault::Body);
    let expected = files(b"hello");
    let content_hash = expected.hash().to_string();

    for (path, bytes) in &expected.content {
        let location = ObjectPath::from(format!("templates/invoice/files/{content_hash}/{path}"));

        fault
            .inner
            .put(&location, PutPayload::from(bytes.clone()))
            .await
            .expect("seed template file");
    }

    let manifest = Manifest::new("invoice", 1, &expected, serde_json::Value::Null);

    let storage = Storage::s3(Arc::new(fault));

    storage
        .index
        .write()
        .expect("index lock")
        .insert(("invoice".to_owned(), 1), manifest);

    let result = storage.get("invoice", 1).await;

    assert!(matches!(result, Err(StorageError::Backend(_))));
}

// -----------------------------------------------------------------------------
// Listing / version lookup
// -----------------------------------------------------------------------------

#[tokio::test]
async fn list_groups_versions_by_template() {
    let storage = Storage::memory();

    storage
        .publish("invoice", &files(b"a"), serde_json::Value::Null)
        .await
        .expect("invoice v1");

    storage
        .publish("invoice", &files(b"b"), serde_json::Value::Null)
        .await
        .expect("invoice v2");

    storage
        .publish("letter", &files(b"c"), serde_json::Value::Null)
        .await
        .expect("letter v1");

    let listed = storage.list();

    assert_eq!(listed.len(), 2);

    assert_eq!(
        listed["invoice"]
            .iter()
            .map(|version| version.version)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );

    assert_eq!(
        listed["letter"]
            .iter()
            .map(|version| version.version)
            .collect::<Vec<_>>(),
        vec![1]
    );

    assert_eq!(
        listed["invoice"][0].content_hash,
        files(b"a").hash().to_string()
    );

    assert!(!listed["invoice"][0].created_at.is_empty());
}

// -----------------------------------------------------------------------------
// Artifacts
// -----------------------------------------------------------------------------

#[tokio::test]
async fn artifact_is_content_addressed_and_stored() {
    let storage = Storage::memory();
    let pdf = b"%PDF-test";

    let hash = storage
        .put_artifact("invoice", 3, pdf)
        .await
        .expect("put artifact");

    assert_eq!(hash, blake3::hash(pdf).to_hex().to_string());

    let location = ObjectPath::from(format!("artifacts/invoice/3/{hash}.pdf"));

    let stored = storage
        .objects
        .get(&location)
        .await
        .expect("artifact")
        .bytes()
        .await
        .expect("artifact bytes");

    assert_eq!(stored.as_ref(), pdf);
}

#[tokio::test]
async fn artifact_propagates_backend_error() {
    let objects: Arc<dyn ObjectStore> = Arc::new(FaultStore::new(Fault::ArtifactPut));

    let storage = Storage::s3(objects);

    let result = storage.put_artifact("invoice", 1, b"%PDF-test").await;

    assert!(matches!(result, Err(StorageError::Backend(_))));
}

// -----------------------------------------------------------------------------
// Paths
// -----------------------------------------------------------------------------

#[test]
fn manifest_path_has_expected_layout() {
    assert_eq!(
        manifest_path("invoice", 42).to_string(),
        "templates/invoice/42/manifest.json"
    );
}
