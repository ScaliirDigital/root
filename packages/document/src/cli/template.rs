use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Subcommand, ValueEnum};

use crate::{
    cli::styles,
    core::{CompileError, Files, Job, Limits, REQUEST, RenderOptions, compile},
};

/// The part of the publish response worth showing. Everything else the server
/// sends back is either already known here or only interesting on failure.
#[derive(serde::Deserialize)]
struct PublishedVersion {
    version: u32,
    content_hash: String,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Validate a local template directory.
    ///
    /// By convention, the directory contains `main.typ` as its entrypoint
    /// and may contain `data.json`, assets, and nested component directories.
    #[command(after_help = "\
Examples:
  document template check ./invoice
  document template check ./invoice --entrypoint src/main.typ
  document template check ./invoice --data examples/reverse-charge.json
")]
    Check {
        /// Root directory of the template bundle.
        directory: PathBuf,

        /// Entrypoint relative to the template directory.
        #[arg(long, default_value = "main.typ")]
        entrypoint: PathBuf,

        /// Data file relative to the template directory.
        ///
        /// When omitted, `data.json` is used if it exists.
        #[arg(long)]
        data: Option<PathBuf>,
    },

    /// Publish a local template directory as a new immutable version.
    ///
    /// All files below the directory are uploaded while preserving their
    /// relative paths.
    #[command(after_help = "\
Examples:
  document template publish invoice ./invoice
  document template publish invoice ./invoice --server https://documents.example.com
  document template publish invoice ./invoice --data examples/reverse-charge.json
")]
    Publish {
        /// Stable identifier of the template.
        template_id: String,

        /// Root directory of the template bundle.
        directory: PathBuf,

        /// Document server base URL.
        #[arg(long, default_value = "http://localhost:8080")]
        server: String,

        /// Entrypoint relative to the template directory.
        #[arg(long, default_value = "main.typ")]
        entrypoint: PathBuf,

        /// Data file relative to the template directory.
        ///
        /// When omitted, `data.json` is used if it exists.
        #[arg(long)]
        data: Option<PathBuf>,
    },

    /// Print the content hash of a local template directory.
    ///
    /// The same address the server derives at publish time, so a bundle can be
    /// compared against a published version without uploading it.
    #[command(after_help = "\
Examples:
  document template hash ./example
  test \"$(document template hash ./example)\" = \"$(curl -s $SERVER/templates/example/1 | jq -r .content_hash)\"
")]
    Hash {
        /// Root directory of the template bundle.
        directory: PathBuf,

        /// Entrypoint relative to the template directory.
        #[arg(long, default_value = "main.typ")]
        entrypoint: PathBuf,
    },

    /// List published templates and their versions.
    #[command(after_help = "\
Examples:
  document template list
  document template list --server https://documents.example.com
")]
    List {
        /// Document server base URL.
        #[arg(long, default_value = "http://localhost:8080")]
        server: String,
    },

    /// Show the manifest of one published version.
    ///
    /// File contents are not included -- the manifest carries their hashes.
    #[command(after_help = "\
Examples:
  document template get example 1
  document template get invoice 3 --server https://documents.example.com
")]
    Get {
        /// Stable identifier of the template.
        template_id: String,

        /// Version to show.
        version: u32,

        /// Document server base URL.
        #[arg(long, default_value = "http://localhost:8080")]
        server: String,
    },

    /// Create a starter template directory.
    ///
    /// Writes a built-in template. Nothing is downloaded -- the templates ship
    /// in the binary. Pass `--compile` to render it straight away.
    #[command(after_help = "\
Examples:
  document template init ./hello
  document template init ./invoice --template invoice
  document template init ./invoice --template invoice --compile
")]
    Init {
        /// Directory to write into. An existing `main.typ` is never replaced.
        directory: PathBuf,

        /// Which built-in template to write.
        #[arg(long, value_enum, default_value_t = Starter::Minimal)]
        template: Starter,

        /// Render the template once after writing it.
        #[arg(long)]
        compile: bool,
    },
}

pub(crate) fn run(command: Command) -> ExitCode {
    match command {
        Command::Check {
            directory,
            entrypoint,
            data,
        } => check(&directory, &entrypoint, data.as_deref()),

        Command::Publish {
            template_id,
            directory,
            server,
            entrypoint,
            data,
        } => publish(
            &template_id,
            &directory,
            &server,
            &entrypoint,
            data.as_deref(),
        ),

        Command::Hash {
            directory,
            entrypoint,
        } => hash(&directory, &entrypoint),

        Command::List { server } => list(&server),

        Command::Get {
            template_id,
            version,
            server,
        } => get(&template_id, version, &server),

        Command::Init {
            directory,
            template,
            compile,
        } => init(&directory, template, compile),
    }
}

/// Compiles a bundle against its fixture without touching a server.
///
/// Exit code only: this is a CI gate, and the diagnostics go to stderr where a
/// build log already looks for them.
fn check(directory: &Path, entrypoint: &Path, fixture: Option<&Path>) -> ExitCode {
    let Some(entrypoint) = entrypoint.to_str() else {
        return broken(
            directory,
            &["entrypoint is not valid utf-8".to_owned()],
            &[],
        );
    };

    let files = match Files::read_dir(directory, entrypoint) {
        Ok(files) => files,
        Err(error) => return broken(directory, &[error.to_string()], &[]),
    };

    if let Err(error) = files.validate() {
        return broken(directory, &[error.to_string()], &[]);
    }

    // A template is validated against sample data, so this is required rather
    // than optional: without it, "it compiles" is the only guarantee.
    let path = fixture.map_or_else(|| directory.join(REQUEST), Path::to_path_buf);

    let request: serde_json::Value = match std::fs::read(&path) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => {
                return broken(directory, &[format!("{}: {error}", path.display())], &[]);
            }
        },
        Err(error) => {
            return broken(directory, &[format!("{}: {error}", path.display())], &[]);
        }
    };

    let data = request.get("data").cloned();

    let files_count = files.content.len();
    let files_hash = files.hash();

    // The same options the server uses when publishing: a template that only
    // renders with a live timestamp is a defect, and this is where it surfaces.
    let job = Job {
        files,
        data,
        xml: None,
        options: RenderOptions {
            timestamp: Some(0),
            ..RenderOptions::default()
        },
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    // Verdict line, so a human scanning a build log sees the outcome without
    // reading the diagnostics underneath it.
    //
    // Two states, not three: a template either renders or it does not. A
    // warning is worth printing but does not make the template broken, or the
    // gate would start failing on things nobody chose to fix.
    match runtime.block_on(compile(&job, Limits::default())) {
        Ok(document) => {
            for warning in &document.warnings {
                eprintln!("warning: {warning}");
            }

            println!(
                "{}  {} {} ({} files, {} bytes, {} warnings)",
                styles::ok("PASSED"),
                directory.display(),
                &files_hash.to_string()[..12],
                files_count,
                document.pdf.len(),
                document.warnings.len(),
            );

            ExitCode::SUCCESS
        }
        Err(CompileError::Template { errors, warnings }) => broken(directory, &errors, &warnings),
        Err(error) => broken(directory, &[error.to_string()], &[]),
    }
}

/// Prints the bundle's content address and nothing else.
///
/// Bare stdout on purpose: this is meant to be captured by a shell and compared
/// against what a server reports, so a decorated line would only get in the way.
fn hash(directory: &Path, entrypoint: &Path) -> ExitCode {
    let Some(entrypoint) = entrypoint.to_str() else {
        eprintln!("error: entrypoint is not valid utf-8");
        return ExitCode::FAILURE;
    };

    match Files::read_dir(directory, entrypoint) {
        Ok(files) => {
            println!("{}", files.hash());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Prints the verdict for a rejected template.
///
/// Same shape however far the run got, so the last line is the answer and a
/// build log can be read without knowing where it stopped.
fn broken(directory: &Path, errors: &[String], warnings: &[String]) -> ExitCode {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
    for error in errors {
        eprintln!("error: {error}");
    }

    println!(
        "{}  {} ({} errors)",
        styles::failed("FAILED"),
        directory.display(),
        errors.len(),
    );
    ExitCode::FAILURE
}

/// Uploads a bundle. The server validates it again and assigns the version.
///
/// The local check is not skipped by publishing: the server runs the same
/// validation, and a rejection there comes back as diagnostics, not as a
/// mystery. Sending first and asking later is the cheaper order for the caller.
fn publish(
    template_id: &str,
    directory: &Path,
    server: &str,
    entrypoint: &Path,
    fixture: Option<&Path>,
) -> ExitCode {
    let Some(entrypoint) = entrypoint.to_str() else {
        return broken(
            directory,
            &["entrypoint is not valid utf-8".to_owned()],
            &[],
        );
    };

    let files = match Files::read_dir(directory, entrypoint) {
        Ok(files) => files,
        Err(error) => return broken(directory, &[error.to_string()], &[]),
    };

    if let Err(error) = files.validate() {
        return broken(directory, &[error.to_string()], &[]);
    }

    let path = fixture.map_or_else(|| directory.join(REQUEST), Path::to_path_buf);
    let request = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return broken(directory, &[format!("{}: {error}", path.display())], &[]);
        }
    };

    // Parsed here only to fail early on malformed JSON -- the bytes are sent
    // as they are, so the server sees exactly what is on disk.
    if let Err(error) = serde_json::from_slice::<serde_json::Value>(&request) {
        return broken(directory, &[format!("{}: {error}", path.display())], &[]);
    }

    let mut form = reqwest::blocking::multipart::Form::new()
        .text("entrypoint", files.entrypoint.clone())
        .part(
            "fixture",
            reqwest::blocking::multipart::Part::bytes(request).file_name(REQUEST),
        );

    for (path, bytes) in files.content {
        form = form.part(
            "file",
            reqwest::blocking::multipart::Part::bytes(bytes).file_name(path),
        );
    }

    let url = format!("{}/templates/{template_id}", server.trim_end_matches('/'));
    let mut request = reqwest::blocking::Client::new().post(&url).multipart(form);

    // Same variable the server reads, so a configured shell needs no flag.
    if let Ok(token) = std::env::var("DOCUMENT_TOKEN")
        && !token.is_empty()
    {
        request = request.bearer_auth(token);
    }

    let response = match request.send() {
        Ok(response) => response,
        Err(error) => return broken(directory, &[format!("{url}: {error}")], &[]),
    };

    let status = response.status();
    let body = response.text().unwrap_or_default();

    if !status.is_success() {
        let errors = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("errors")?
                    .as_array()?
                    .iter()
                    .map(|entry| entry.as_str().map(ToOwned::to_owned))
                    .collect::<Option<Vec<_>>>()
            })
            .unwrap_or_else(|| vec![format!("server answered {status}")]);

        return broken(directory, &errors, &[]);
    }

    // The response is small and known: pull the two fields worth showing and
    // drop the rest. Printing the raw body would make the caller read JSON to
    // learn a version number.
    let published: PublishedVersion = match serde_json::from_str(&body) {
        Ok(published) => published,
        Err(error) => return broken(directory, &[format!("{url}: {error}")], &[]),
    };

    println!(
        "{}  {template_id} v{} ({})",
        styles::ok("PUBLISHED"),
        published.version,
        &published.content_hash[..12],
    );

    ExitCode::SUCCESS
}

fn list(server: &str) -> ExitCode {
    match fetch(server, "/templates") {
        Ok(body) => print_json(&body),
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Prints a response as indented JSON.
///
/// The server's shape is the shape: reformatting it here would mean keeping a
/// second view of a schema that still moves, and it would break `| jq`.
fn print_json(body: &str) -> ExitCode {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => {
            println!("{value:#}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Prints the manifest as it came back, indented.
///
/// No reformatting into a table: the manifest is the record of what a version
/// is, and reshaping it here would mean maintaining a second view of a schema
/// that is still moving.
fn get(template_id: &str, version: u32, server: &str) -> ExitCode {
    let path = format!("/templates/{template_id}/{version}");

    let body = match fetch(server, &path) {
        Ok(body) => body,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };

    match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(value) => {
            println!("{value:#}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Shared GET: base URL, bearer token, and turning a bad status into a message.
fn fetch(server: &str, path: &str) -> Result<String, String> {
    let url = format!("{}{path}", server.trim_end_matches('/'));
    let mut request = reqwest::blocking::Client::new().get(&url);

    if let Ok(token) = std::env::var("DOCUMENT_TOKEN")
        && !token.is_empty()
    {
        request = request.bearer_auth(token);
    }

    let response = request.send().map_err(|error| format!("{url}: {error}"))?;
    let status = response.status();
    let body = response.text().unwrap_or_default();

    if status.is_success() {
        Ok(body)
    } else {
        Err(format!("server answered {status}: {body}"))
    }
}

/// The built-in templates, compiled into the binary.
///
/// `minimal` is two files: where data comes from, and what it looks like.
/// `invoice` is the one that matters -- it is what produces a valid Factur-X
/// document, and a reader should not need the repository to get at it.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Starter {
    Minimal,
    Invoice,
}

type StarterFile = (&'static str, &'static [u8]);

/// Bundles one built-in template. Paths are relative to the template
/// directory and are written verbatim.
///
/// The literal `__data/request.json` duplicates [`REQUEST`] because `concat!`
/// only takes literals. The test below pins them together.
macro_rules! starter {
    ($name:literal, $($file:literal),+ $(,)?) => {
        &[$((
            $file,
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/assets/templates/", $name, "/", $file
            )) as &'static [u8],
        )),+]
    };
}

const MINIMAL: &[StarterFile] = starter!("minimal", "main.typ", "__data/request.json");

const INVOICE: &[StarterFile] = starter!(
    "invoice",
    "main.typ",
    "brand.typ",
    "fixture.json",
    "__data/request.json",
    "de.json",
    "en.json",
    "logo.svg",
);

impl Starter {
    fn files(self) -> &'static [StarterFile] {
        match self {
            Self::Minimal => MINIMAL,
            Self::Invoice => INVOICE,
        }
    }
}

fn init(directory: &Path, template: Starter, compile: bool) -> ExitCode {
    // Refuses to clobber an existing template, not to use an existing
    // directory: `init .` in a prepared project directory is the common case,
    // and overwriting someone's `main.typ` is the thing that actually hurts.
    if directory.join("main.typ").exists() {
        eprintln!("error: {} already holds a template", directory.display());
        return ExitCode::FAILURE;
    }

    // One call, not a check followed by a create: the gap between the two is a
    // race, and this reports the same cases the check would have.
    if let Err(error) = std::fs::create_dir_all(directory) {
        eprintln!("error: {}: {error}", directory.display());
        return ExitCode::FAILURE;
    }

    let files = template.files();

    for (name, content) in files {
        let path = directory.join(name);

        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            eprintln!("error: {}: {error}", parent.display());
            return ExitCode::FAILURE;
        }

        if let Err(error) = std::fs::write(&path, content) {
            eprintln!("error: {}: {error}", path.display());
            return ExitCode::FAILURE;
        }
    }

    println!("{}  {}", styles::ok("CREATED"), directory.display());
    for (name, _) in files {
        println!("         {}/{name}", directory.display());
    }

    // The directory name doubles as the template id in the hint below, and as
    // the name of the rendered file: `init ./invoice` should not leave a
    // `hello.pdf` behind.
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("template");

    if compile {
        println!();

        let entrypoint = directory.join("main.typ");
        let output = directory.join(format!("{name}.pdf"));

        if crate::cli::compile::run(entrypoint, Some(output)) != ExitCode::SUCCESS {
            return ExitCode::FAILURE;
        }
    }

    println!("\nnext:");
    println!("  document template check {}", directory.display());
    println!("  document compile {}/main.typ", directory.display());
    println!("  document template publish {name} {}", directory.display());

    ExitCode::SUCCESS
}
