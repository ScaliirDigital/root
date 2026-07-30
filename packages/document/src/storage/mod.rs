//! Template storage: append-only, versioned.
//!
//! "Immutable" does not mean "static". Publishing is a runtime API -- you can
//! upload a new layout at three in the morning without a deploy. What is
//! forbidden is mutating a version in place.
//!
//! The reason is concrete, not aesthetic: if `invoice/v2` can be overwritten,
//! last year's invoice can no longer be reproduced. Changed content gets a new
//! version; `v2` stays what it was.
//!
//! Everything is objects. Memory is the default so a fresh binary runs without
//! configuration; persistence is either the local filesystem or S3, never both.
//!
//! Layout:
//!
//!   `templates/{id}/files/{content_hash}/{path`}   file content, deduplicated
//!   templates/{id}/{version}/manifest.json       written last, the commit point
//!   `artifacts/{id}/{version}/{pdf_hash}.pdf`      archived renders
//!
//! Files go under their content hash rather than under the version, because the
//! version is only settled once the manifest lands. A half-written bundle is
//! invisible without its manifest; a manifest without its files would not be,
//! which is why the order is never reversed.
//!
//! Artifacts sit under their own root rather than beside the template: the two
//! have different lifecycles, and a retention rule should be able to reach one
//! without touching the other.
//!
//! Neighbour: [`s3`] -- building and probing an S3-compatible object store.

pub mod s3;

use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use futures_util::StreamExt;
use lru::LruCache;
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutPayload, path::Path as ObjectPath};
use serde::{Deserialize, Serialize};

use crate::core::{Files, RENDERER};

pub type Version = u32;

/// File sets kept after their first render. Bounded, or it grows with history.
const CACHE_ENTRIES: usize = 64;

/// Concurrent publishers race for the next version number. Each loss costs one
/// listing, so this ceiling catches a broken backend, not contention.
const MAX_ATTEMPTS: u32 = 8;

// ---------------------------------------------------------------------------
// What a version is
// ---------------------------------------------------------------------------

/// What makes a version.
///
/// Written last during a publish, so its existence means the files are
/// complete: an object store has no transaction, and this is the only atomic
/// switch available. Everything here is either assigned (the version) or comes
/// from outside the file set (the fixture, the renderer) -- none of it can be
/// derived from the content, which is why content addressing alone is not
/// enough.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub template_id: String,
    pub version: Version,
    pub entrypoint: String,

    /// Hash over the whole file set. Doubles as the directory the files live
    /// in, because it is known before the version is.
    pub content_hash: String,

    /// Path -> hash. Carries the check on load and records which objects
    /// belong to this version.
    pub files: BTreeMap<String, String>,

    pub fixture: serde_json::Value,

    /// The build that produced this version. Cannot be reconstructed
    /// afterwards, and without it a stored document cannot be defended later.
    pub renderer: String,

    pub created_at: String,
}

impl Manifest {
    /// Builds the manifest for a set of files about to be published.
    ///
    /// Hashes each file separately: the bundle hash proves the set is intact,
    /// the per-file hashes say which object went wrong when it is not.
    #[must_use]
    pub fn new(
        template_id: &str,
        version: Version,
        files: &Files,
        fixture: serde_json::Value,
    ) -> Self {
        Self {
            template_id: template_id.to_owned(),
            version,
            entrypoint: files.entrypoint.clone(),
            content_hash: files.hash().to_string(),
            files: files
                .content
                .iter()
                .map(|(path, bytes)| (path.clone(), blake3::hash(bytes).to_hex().to_string()))
                .collect(),
            fixture,
            renderer: RENDERER.to_owned(),
            created_at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        }
    }
}

/// One published version, without its fixture or file list.
///
/// Enough to recognise and compare a version -- the content hash is what a
/// caller matches against a local bundle. The full manifest is one request
/// away and carries the rest.
#[derive(Clone, Debug, Serialize)]
pub struct VersionSummary {
    pub version: Version,
    pub content_hash: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// One object store, one index, one cache.
///
/// The index exists so a render needs no network call: it is built once at
/// startup and only appended to. The cache holds file sets after their first
/// render, for the same reason -- with S3 behind this, a per-render fetch would
/// dominate the response time.
pub struct Storage {
    objects: Arc<dyn ObjectStore>,
    index: RwLock<BTreeMap<(String, Version), Manifest>>,
    cache: Mutex<LruCache<String, Arc<Files>>>,
}

impl Storage {
    fn new(objects: Arc<dyn ObjectStore>) -> Self {
        let entries = NonZeroUsize::new(CACHE_ENTRIES).expect("cache size is a non-zero constant");

        Self {
            objects,
            index: RwLock::new(BTreeMap::new()),
            cache: Mutex::new(LruCache::new(entries)),
        }
    }

    /// In-process. Nothing survives a restart -- the default, and what tests use.
    #[must_use]
    pub fn memory() -> Self {
        Self::new(Arc::new(object_store::memory::InMemory::new()))
    }

    /// # Errors
    ///
    /// [`StorageError::LocalDirectory`] if `root` cannot be created,
    /// [`StorageError::Backend`] if it cannot be opened.
    pub fn local(root: &Path) -> Result<Self, StorageError> {
        create_local_directory(root)?;
        open_local_storage(root)
    }

    #[must_use]
    pub fn s3(bucket: Arc<dyn ObjectStore>) -> Self {
        Self::new(bucket)
    }

    /// Reads every manifest into the index.
    ///
    /// Runs once at startup, and again when a version number was claimed by
    /// someone else. A manifest that will not parse aborts the start rather
    /// than silently hiding a version: serving a store that is quietly missing
    /// entries is worse than not serving at all.
    ///
    /// # Errors
    ///
    /// [`StorageError::Corrupt`] for an unreadable manifest,
    /// [`StorageError::Backend`] if listing fails.
    pub async fn load(&self) -> Result<usize, StorageError> {
        let prefix = ObjectPath::from("templates");
        let mut listing = self.objects.list(Some(&prefix));
        let mut index = BTreeMap::new();

        while let Some(object) = listing.next().await {
            let object = object?;

            if object.location.filename() != Some("manifest.json") {
                continue;
            }

            let bytes = self.objects.get(&object.location).await?.bytes().await?;

            let manifest: Manifest =
                serde_json::from_slice(&bytes).map_err(|source| StorageError::Corrupt {
                    path: object.location.to_string(),
                    source,
                })?;

            index.insert((manifest.template_id.clone(), manifest.version), manifest);
        }

        let count = index.len();
        *self.index.write().expect("index lock poisoned") = index;

        Ok(count)
    }

    /// Appends a new version and returns it. Never overwrites.
    ///
    /// Republishing byte-identical content returns the existing version rather
    /// than minting a new one, so idempotent retries do not inflate history.
    ///
    /// # Errors
    ///
    /// [`StorageError::Backend`] on write failure, [`StorageError::Contention`] if
    /// the version number could not be claimed.
    pub async fn publish(
        &self,
        template_id: &str,
        files: &Files,
        fixture: serde_json::Value,
    ) -> Result<Version, StorageError> {
        let content_hash = files.hash().to_string();

        // Checked before writing: the other order would cost a bundle per retry.
        if let Some(version) = self.find_by_hash(template_id, &content_hash) {
            return Ok(version);
        }

        // Files first. Orphaned objects after an abort are harmless -- without a
        // manifest nobody sees them. A manifest without its files would not be,
        // which is why the order is never reversed.
        self.put_files(template_id, &content_hash, files).await?;

        for _ in 0..MAX_ATTEMPTS {
            let version = self.next_version(template_id);
            let manifest = Manifest::new(template_id, version, files, fixture.clone());

            let body = serde_json::to_vec(&manifest).expect("Manifest serialization is infallible");

            let result = self
                .objects
                .put_opts(
                    &manifest_path(template_id, version),
                    PutPayload::from(body),
                    PutMode::Create.into(),
                )
                .await;

            match result {
                Ok(_) => {
                    self.index
                        .write()
                        .expect("index lock poisoned")
                        .insert((template_id.to_owned(), version), manifest);

                    return Ok(version);
                }
                // Someone else claimed it. Refresh and recompute.
                Err(object_store::Error::AlreadyExists { .. }) => {
                    self.load().await?;
                }
                Err(error) => return Err(error.into()),
            }
        }

        Err(StorageError::Contention(template_id.to_owned()))
    }

    /// Loads a published version, fetching its files on first use.
    ///
    /// # Errors
    ///
    /// [`StorageError::NotFound`] for an unknown template or version,
    /// [`StorageError::Tampered`] if stored bytes no longer match the manifest.
    pub async fn get(
        &self,
        template_id: &str,
        version: Version,
    ) -> Result<(Manifest, Arc<Files>), StorageError> {
        // Scoped so no lock is held across an await: the guards are std, not
        // tokio, and holding one over a network call would block the runtime.
        let manifest = {
            self.index
                .read()
                .expect("index lock poisoned")
                .get(&(template_id.to_owned(), version))
                .cloned()
        }
        .ok_or_else(|| StorageError::NotFound(template_id.to_owned(), version))?;

        let cached = {
            self.cache
                .lock()
                .expect("cache lock poisoned")
                .get(&manifest.content_hash)
                .map(Arc::clone)
        };

        if let Some(files) = cached {
            return Ok((manifest, files));
        }

        let files = Arc::new(self.fetch_files(&manifest).await?);

        self.cache
            .lock()
            .expect("cache lock poisoned")
            .put(manifest.content_hash.clone(), Arc::clone(&files));

        Ok((manifest, files))
    }

    /// Stores a rendered document and returns its address.
    ///
    /// Content-addressed: an identical retry overwrites identical bytes, so a
    /// repeated request cannot produce a second stored document. The address is
    /// what a receipt would later point at.
    ///
    /// # Errors
    ///
    /// [`StorageError::Backend`] if the write fails.
    pub async fn put_artifact(
        &self,
        template_id: &str,
        version: Version,
        pdf: &[u8],
    ) -> Result<String, StorageError> {
        let hash = blake3::hash(pdf).to_hex().to_string();
        let location = ObjectPath::from(format!("artifacts/{template_id}/{version}/{hash}.pdf"));

        self.objects
            .put(&location, PutPayload::from(pdf.to_vec()))
            .await?;

        Ok(hash)
    }

    // -- reads served from the index, without touching the object store -------

    /// Every template with its published versions, ascending.
    ///
    /// Listing is a browsing operation and should not cost a round trip per
    /// caller.
    #[must_use]
    pub fn list(&self) -> BTreeMap<String, Vec<VersionSummary>> {
        let mut templates: BTreeMap<String, Vec<VersionSummary>> = BTreeMap::new();

        for manifest in self.index.read().expect("index lock poisoned").values() {
            templates
                .entry(manifest.template_id.clone())
                .or_default()
                .push(VersionSummary {
                    version: manifest.version,
                    content_hash: manifest.content_hash.clone(),
                    created_at: manifest.created_at.clone(),
                });
        }

        templates
    }

    /// The manifest for one version, without fetching its files.
    ///
    /// Separate from [`Storage::get`] on purpose: answering "what is in this
    /// version" should not pull the bundle into the cache.
    ///
    /// # Errors
    ///
    /// [`StorageError::NotFound`] for an unknown template or version.
    pub fn manifest(&self, template_id: &str, version: Version) -> Result<Manifest, StorageError> {
        self.index
            .read()
            .expect("index lock poisoned")
            .get(&(template_id.to_owned(), version))
            .cloned()
            .ok_or_else(|| StorageError::NotFound(template_id.to_owned(), version))
    }

    #[must_use]
    pub fn latest(&self, template_id: &str) -> Option<Version> {
        self.index
            .read()
            .expect("index lock poisoned")
            .keys()
            .filter(|(id, _)| id == template_id)
            .map(|(_, version)| *version)
            .max()
    }

    // -- internals ------------------------------------------------------------
    async fn put_files(
        &self,
        template_id: &str,
        content_hash: &str,
        files: &Files,
    ) -> Result<(), StorageError> {
        for (path, bytes) in &files.content {
            let location = ObjectPath::from(format!(
                "templates/{template_id}/files/{content_hash}/{path}"
            ));

            self.objects
                .put(&location, PutPayload::from(bytes.clone()))
                .await?;
        }

        Ok(())
    }

    /// Verifies every file against the manifest: a mismatch means the bytes
    /// changed underneath us, which breaks the one promise this module makes.
    async fn fetch_files(&self, manifest: &Manifest) -> Result<Files, StorageError> {
        let mut content = BTreeMap::new();

        for (path, expected) in &manifest.files {
            let location = ObjectPath::from(format!(
                "templates/{}/files/{}/{path}",
                manifest.template_id, manifest.content_hash
            ));

            let bytes = self.objects.get(&location).await?.bytes().await?;
            let actual = blake3::hash(&bytes).to_hex().to_string();

            if &actual != expected {
                return Err(StorageError::Tampered {
                    path: location.to_string(),
                });
            }

            content.insert(path.clone(), bytes.to_vec());
        }

        Ok(Files {
            entrypoint: manifest.entrypoint.clone(),
            content,
        })
    }

    fn find_by_hash(&self, template_id: &str, content_hash: &str) -> Option<Version> {
        self.index
            .read()
            .expect("index lock poisoned")
            .values()
            .find(|manifest| {
                manifest.template_id == template_id && manifest.content_hash == content_hash
            })
            .map(|manifest| manifest.version)
    }

    fn next_version(&self, template_id: &str) -> Version {
        self.latest(template_id).map_or(1, |version| version + 1)
    }
}

fn manifest_path(template_id: &str, version: Version) -> ObjectPath {
    ObjectPath::from(format!("templates/{template_id}/{version}/manifest.json"))
}

fn create_local_directory(root: &Path) -> Result<(), StorageError> {
    std::fs::create_dir_all(root).map_err(|source| StorageError::LocalDirectory {
        path: root.to_owned(),
        source,
    })
}

fn open_local_storage(root: &Path) -> Result<Storage, StorageError> {
    let objects =
        object_store::local::LocalFileSystem::new_with_prefix(root).map_err(|source| {
            StorageError::LocalBackend {
                path: root.to_owned(),
                source,
            }
        })?;

    Ok(Storage::new(Arc::new(objects)))
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("template `{0}` version {1} not found")]
    NotFound(String, Version),

    #[error("stored file `{path}` does not match its manifest hash")]
    Tampered { path: String },

    #[error("could not claim a version for `{0}` after {MAX_ATTEMPTS} attempts")]
    Contention(String),

    #[error("manifest `{path}` is not readable")]
    Corrupt {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("could not create local storage directory `{path}`")]
    LocalDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not open local storage directory `{path}`")]
    LocalBackend {
        path: PathBuf,
        #[source]
        source: object_store::Error,
    },

    #[error(transparent)]
    S3(#[from] s3::Error),

    #[error(transparent)]
    Backend(#[from] object_store::Error),
}

#[cfg(test)]
mod tests;
