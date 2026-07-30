//! The path a user actually walks: start the service, publish a template,
//! render against it.
//!
//! Covered here rather than in a unit test because the interesting failures
//! live between the parts -- a route that does not match the handler, a job
//! that never reaches a worker, a PDF that comes back as an error body.

use std::{
    fs,
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use tempfile::TempDir;

const EXE: &str = env!("CARGO_BIN_EXE_document");

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("no free port")
        .local_addr()
        .expect("no local address")
        .port()
}

fn server_command_from(executable: &Path, port: u16) -> Command {
    let mut command = Command::new(executable);

    command
        .args(["serve", "--listen", &format!("127.0.0.1:{port}")])
        .env_remove("DOCUMENT_WORKERS")
        .env_remove("DOCUMENT_TOKEN")
        .env_remove("DOCUMENT_DATA_DIR")
        .env_remove("DOCUMENT_S3_BUCKET")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    command
}

fn server_command(port: u16) -> Command {
    server_command_from(Path::new(EXE), port)
}

fn wait_for_service(child: &mut Child, port: u16) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let health = format!("http://127.0.0.1:{port}/health");

    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().expect("failed to inspect service process") {
            panic!("the service exited before becoming ready: {status}");
        }

        if reqwest::blocking::get(&health).is_ok_and(|response| response.status().is_success()) {
            return;
        }

        thread::sleep(Duration::from_millis(100));
    }

    panic!("the service did not become ready");
}

const RUSTFS_ACCESS_KEY: &str = "document-test";
const RUSTFS_SECRET_KEY: &str = "document-test-secret";
const RUSTFS_BUCKET: &str = "document-test";

struct RustFs {
    child: Child,
    endpoint: String,
    _data: TempDir,
}

impl RustFs {
    fn start() -> Option<Self> {
        if !command_available("rustfs") {
            eprintln!("skipping RustFS integration test: `rustfs` is not installed");
            return None;
        }

        assert!(
            command_available("curl"),
            "`curl` is required for the RustFS integration test"
        );

        let port = free_port();
        let data = tempfile::tempdir().expect("failed to create RustFS data directory");
        let endpoint = format!("http://127.0.0.1:{port}");

        let child = Command::new("rustfs")
            .arg(data.path())
            .env("RUSTFS_ADDRESS", format!("127.0.0.1:{port}"))
            .env("RUSTFS_ACCESS_KEY", RUSTFS_ACCESS_KEY)
            .env("RUSTFS_SECRET_KEY", RUSTFS_SECRET_KEY)
            .env("RUSTFS_CONSOLE_ENABLE", "false")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to start RustFS");

        let mut rustfs = Self {
            child,
            endpoint,
            _data: data,
        };

        rustfs.create_bucket(RUSTFS_BUCKET);

        Some(rustfs)
    }

    fn create_bucket(&mut self, bucket: &str) {
        let credentials = format!("{RUSTFS_ACCESS_KEY}:{RUSTFS_SECRET_KEY}");
        let url = format!("{}/{bucket}", self.endpoint);
        let deadline = Instant::now() + Duration::from_secs(30);

        while Instant::now() < deadline {
            if let Some(status) = self
                .child
                .try_wait()
                .expect("failed to inspect RustFS process")
            {
                panic!("RustFS exited before becoming ready: {status}");
            }

            let output = Command::new("curl")
                .arg("--silent")
                .arg("--show-error")
                .arg("--request")
                .arg("PUT")
                .arg("--aws-sigv4")
                .arg("aws:amz:us-east-1:s3")
                .arg("--user")
                .arg(&credentials)
                .arg("--header")
                .arg(
                    "X-Amz-Content-Sha256: \
                 e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                )
                .arg("--write-out")
                .arg("\n%{http_code}")
                .arg(&url)
                .output()
                .expect("failed to execute curl");

            // RustFS has not opened the listener yet.
            if !output.status.success() {
                thread::sleep(Duration::from_millis(100));
                continue;
            }

            let stdout = String::from_utf8_lossy(&output.stdout);
            let (body, status) = stdout
                .rsplit_once('\n')
                .expect("curl output did not contain HTTP status");

            match status {
                "200" => return,

                // Listener is up, but the storage layer is still initializing.
                "503" => {
                    thread::sleep(Duration::from_millis(100));
                }

                _ => {
                    panic!("failed to create RustFS test bucket: HTTP {status}\n{body}");
                }
            }
        }

        panic!("RustFS did not become ready within 30 seconds");
    }
}

impl Drop for RustFs {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn command_available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// A running service, stopped when the test ends.
///
/// Every instance gets its own data directory. Server integration tests run in
/// parallel, and sharing the default store would make template versions depend
/// on test execution order.
///
/// Killed on drop rather than at the end of the test body, so a failing
/// assertion does not leave a process holding a port.
struct Service {
    child: Child,
    port: u16,
    data: TempDir,
}

impl Service {
    fn start() -> Self {
        Self::with_env(&[])
    }

    fn with_env(env: &[(&str, &str)]) -> Self {
        let port = free_port();
        let data = tempfile::tempdir().expect("failed to create server data directory");

        let mut command = server_command(port);

        command.env("DOCUMENT_DATA_DIR", data.path());

        for (key, value) in env {
            command.env(key, value);
        }

        let child = command.spawn().expect("failed to start the service");

        let mut service = Self { child, port, data };

        service.wait_until_ready();
        service
    }

    fn wait_until_ready(&mut self) {
        wait_for_service(&mut self.child, self.port);
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use nix::{
                sys::signal::{Signal, kill},
                unistd::Pid,
            };

            let _ = kill(
                Pid::from_raw(self.child.id().cast_signed()),
                Signal::SIGTERM,
            );
        }

        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }

        let _ = self.child.wait();
    }
}

fn run(command: &mut Command) -> Output {
    command.output().expect("failed to run document")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\n\nstdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn initialized_template(name: &str) -> (TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("failed to create template directory");
    let directory = temp.path().join(name);

    let output = run(Command::new(EXE)
        .env_remove("DOCUMENT_TOKEN")
        .args(["template", "init"])
        .arg(&directory));

    assert_success(&output);

    (temp, directory)
}

/// Publish through the CLI rather than assembling multipart here.
///
/// That is how a user performs the operation, so an E2E test puts both sides of
/// the contract on trial at once: CLI multipart generation and server parsing.
fn publish(service: &Service, template_id: &str, directory: &Path, token: Option<&str>) -> Output {
    let mut command = Command::new(EXE);

    command
        .env_remove("DOCUMENT_TOKEN")
        .args(["template", "publish", template_id])
        .arg(directory)
        .args(["--server", &service.base()]);

    if let Some(token) = token {
        command.env("DOCUMENT_TOKEN", token);
    }

    run(&mut command)
}

fn publish_example(service: &Service, template_id: &str) -> (TempDir, PathBuf) {
    let (temp, directory) = initialized_template(template_id);

    let output = publish(service, template_id, &directory, None);
    assert_success(&output);

    (temp, directory)
}

/// Publishes the invoice template straight from the repository.
///
/// No temporary copy: publishing only reads, and the point of this helper is
/// that the template shipped in the repository is the one that gets exercised.
fn publish_invoice(service: &Service, template_id: &str) -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("templates")
        .join("invoice");

    let output = publish(service, template_id, &directory, None);
    assert_success(&output);

    directory
}

/// Multipart straight to the server, bypassing the CLI.
///
/// The CLI validates locally before it sends, so the server's own rejection
/// paths are only reachable from here.
fn post_multipart(
    service: &Service,
    path: &str,
    form: reqwest::blocking::multipart::Form,
) -> reqwest::blocking::Response {
    reqwest::blocking::Client::new()
        .post(format!("{}{path}", service.base()))
        .multipart(form)
        .send()
        .expect("the request failed")
}

fn file_part(name: &str, content: &str) -> reqwest::blocking::multipart::Part {
    reqwest::blocking::multipart::Part::text(content.to_owned()).file_name(name.to_owned())
}

fn binary_file_part(name: &str, content: Vec<u8>) -> reqwest::blocking::multipart::Part {
    reqwest::blocking::multipart::Part::bytes(content).file_name(name.to_owned())
}

const TRIVIAL: &str = "= Fine";

// -----------------------------------------------------------------------------
// Lifecycle and probes
// -----------------------------------------------------------------------------

#[test]
fn starts_and_becomes_healthy() {
    let service = Service::start();

    let response = reqwest::blocking::get(format!("{}/health", service.base()))
        .expect("health request failed");

    assert!(response.status().is_success());
}

#[test]
fn health_is_available_without_authentication() {
    let service = Service::with_env(&[("DOCUMENT_TOKEN", "secret")]);

    let response = reqwest::blocking::get(format!("{}/health", service.base()))
        .expect("health request failed");

    assert!(
        response.status().is_success(),
        "health probes must remain public"
    );
}

// -----------------------------------------------------------------------------
// Authentication
// -----------------------------------------------------------------------------

/// With a token set, an unauthenticated call has to be turned away -- and the
/// probes have to stay open, or an orchestrator cannot tell a live process from
/// a dead one.
#[test]
fn a_token_closes_the_api_but_not_the_probes() {
    let service = Service::with_env(&[("DOCUMENT_TOKEN", "secret")]);
    let client = reqwest::blocking::Client::new();

    let unauthorized = client
        .get(format!("{}/templates", service.base()))
        .send()
        .expect("the request failed");

    assert_eq!(unauthorized.status(), 401);

    let wrong = client
        .get(format!("{}/templates", service.base()))
        .bearer_auth("wrong")
        .send()
        .expect("the request failed");

    assert_eq!(wrong.status(), 401);

    let authorized = client
        .get(format!("{}/templates", service.base()))
        .bearer_auth("secret")
        .send()
        .expect("the request failed");

    assert!(authorized.status().is_success());

    let health = client
        .get(format!("{}/health", service.base()))
        .send()
        .expect("the request failed");

    assert!(health.status().is_success(), "probes must stay open");
}

#[test]
fn cli_can_publish_to_an_authenticated_server() {
    let service = Service::with_env(&[("DOCUMENT_TOKEN", "secret")]);
    let (_temp, directory) = initialized_template("authenticated");

    let unauthorized = publish(&service, "authenticated", &directory, None);

    assert!(
        !unauthorized.status.success(),
        "publishing without the configured token must fail"
    );

    let authorized = publish(&service, "authenticated", &directory, Some("secret"));

    assert_success(&authorized);
}

// -----------------------------------------------------------------------------
// Publishing
// -----------------------------------------------------------------------------

#[test]
fn publishes_a_template() {
    let service = Service::start();
    let (_temp, directory) = initialized_template("example");

    let output = publish(&service, "example", &directory, None);

    assert_success(&output);

    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("PUBLISHED"));
    assert!(stdout.contains("example v1"));
}

#[test]
fn invalid_template_is_rejected_at_publish() {
    let service = Service::start();
    let (_temp, directory) = initialized_template("broken");

    fs::remove_file(directory.join("main.typ")).expect("failed to remove entrypoint");

    let output = publish(&service, "broken", &directory, None);

    assert!(
        !output.status.success(),
        "a template without its entrypoint must not publish"
    );
}

#[test]
fn malformed_request_json_is_rejected_at_publish() {
    let service = Service::start();
    let (_temp, directory) = initialized_template("broken-json");

    fs::write(
        directory.join("__data/request.json"),
        b"{ definitely not json",
    )
    .expect("failed to corrupt request");

    let output = publish(&service, "broken-json", &directory, None);

    assert!(
        !output.status.success(),
        "malformed request JSON must not publish"
    );
}

// -----------------------------------------------------------------------------
// Listing and manifests
// -----------------------------------------------------------------------------

#[test]
fn published_templates_can_be_listed() {
    let service = Service::start();
    let (_temp, _directory) = publish_example(&service, "example");

    let response = reqwest::blocking::get(format!("{}/templates", service.base()))
        .expect("list request failed");

    assert!(response.status().is_success());

    let body = response.text().expect("list response has no body");

    serde_json::from_str::<serde_json::Value>(&body).expect("template list is not valid JSON");

    assert!(
        body.contains("example"),
        "published template is absent from the list: {body}"
    );
}

#[test]
fn a_published_version_can_be_read_back() {
    let service = Service::start();
    let (_temp, _directory) = publish_example(&service, "example");

    let response = reqwest::blocking::get(format!("{}/templates/example/1", service.base()))
        .expect("manifest request failed");

    assert!(response.status().is_success());

    let body = response.text().expect("manifest has no body");

    serde_json::from_str::<serde_json::Value>(&body).expect("manifest is not valid JSON");
}

#[test]
fn unknown_template_manifest_is_not_found() {
    let service = Service::start();

    let response = reqwest::blocking::get(format!("{}/templates/nothing/1", service.base()))
        .expect("manifest request failed");

    assert_eq!(response.status(), 404);
}

// -----------------------------------------------------------------------------
// Versioning
// -----------------------------------------------------------------------------

/// Published versions are immutable.
///
/// Publishing a changed bundle creates a new version. Reading version 1 after
/// version 2 exists must return exactly the same manifest it returned before.
#[test]
fn publishing_again_creates_an_immutable_new_version() {
    let service = Service::start();
    let (_temp, directory) = initialized_template("versioned");

    let first_publish = publish(&service, "versioned", &directory, None);
    assert_success(&first_publish);

    let version_one_before =
        reqwest::blocking::get(format!("{}/templates/versioned/1", service.base()))
            .expect("failed to read version 1")
            .text()
            .expect("version 1 has no body");

    // Change the bundle without making the Typst program invalid. The content
    // address must change, while version 1 remains untouched.
    let entrypoint = directory.join("main.typ");
    let mut source = fs::read_to_string(&entrypoint).expect("failed to read starter template");

    source.push_str("\n// second immutable version\n");

    fs::write(&entrypoint, source).expect("failed to update starter template");

    let second_publish = publish(&service, "versioned", &directory, None);
    assert_success(&second_publish);

    let stdout = String::from_utf8_lossy(&second_publish.stdout);
    assert!(
        stdout.contains("versioned v2"),
        "second publish should create version 2: {stdout}"
    );

    let version_one_after =
        reqwest::blocking::get(format!("{}/templates/versioned/1", service.base()))
            .expect("failed to reread version 1")
            .text()
            .expect("version 1 has no body");

    let version_two = reqwest::blocking::get(format!("{}/templates/versioned/2", service.base()))
        .expect("failed to read version 2")
        .text()
        .expect("version 2 has no body");

    assert_eq!(
        version_one_before, version_one_after,
        "publishing version 2 changed version 1"
    );

    assert_ne!(
        version_one_after, version_two,
        "changed bundles should not produce identical manifests"
    );
}

// -----------------------------------------------------------------------------
// Rendering
// -----------------------------------------------------------------------------

#[test]
fn publishes_and_renders() {
    let service = Service::start();
    let (_temp, _directory) = publish_example(&service, "example");

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/example/1/render", service.base()))
        .json(&serde_json::json!({
            "data": {
                "title": "Hello",
                "message": "An example message."
            },
            "archival": false,
        }))
        .send()
        .expect("the render request failed");

    assert!(
        response.status().is_success(),
        "render answered {}",
        response.status()
    );

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();

    let pdf = response.bytes().expect("no body");

    assert!(pdf.starts_with(b"%PDF"), "the response is not a pdf");

    if !content_type.is_empty() {
        assert!(
            content_type.contains("application/pdf"),
            "unexpected content type: {content_type}"
        );
    }
}

#[test]
fn renders_with_default_options() {
    let service = Service::start();
    let (_temp, _directory) = publish_example(&service, "defaults");

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/defaults/1/render", service.base()))
        .json(&serde_json::json!({
            "data": {
                "title": "Hello",
                "message": "Defaults."
            }
        }))
        .send()
        .expect("the render request failed");

    assert!(
        response.status().is_success(),
        "render answered {}",
        response.status()
    );

    let pdf = response.bytes().expect("no body");
    assert!(pdf.starts_with(b"%PDF"));
}

#[test]
fn an_archival_render_without_xml_is_a_pdf() {
    let service = Service::start();
    let (_temp, _directory) = publish_example(&service, "archival");

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/archival/1/render", service.base()))
        .json(&serde_json::json!({
            "data": {
                "title": "Archive",
                "message": "PDF/A"
            },
            "archival": true,
        }))
        .send()
        .expect("the render request failed");

    assert!(
        response.status().is_success(),
        "render answered {}",
        response.status()
    );

    let pdf = response.bytes().expect("no body");
    assert!(pdf.starts_with(b"%PDF"));
}

/// An unknown version is a 404, not a 500 and not an empty PDF.
#[test]
fn unknown_version_is_not_found() {
    let service = Service::start();

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/nothing/1/render", service.base()))
        .json(&serde_json::json!({
            "data": {}
        }))
        .send()
        .expect("the request failed");

    assert_eq!(response.status(), 404);
}

#[test]
fn malformed_render_json_is_rejected() {
    let service = Service::start();
    let (_temp, _directory) = publish_example(&service, "malformed");

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/malformed/1/render", service.base()))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{ definitely not json")
        .send()
        .expect("the request failed");

    assert!(
        response.status().is_client_error(),
        "malformed JSON should be rejected, got {}",
        response.status()
    );
}

// -----------------------------------------------------------------------------
// Ad hoc render
// -----------------------------------------------------------------------------

#[test]
fn renders_ad_hoc_source() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .text("data", r#"{"title":"Ad hoc"}"#)
        .part(
            "file",
            file_part("main.typ", "#json(\"__data/request.json\")\n= Ad hoc"),
        );

    let response = post_multipart(&service, "/render", form);

    assert!(
        response.status().is_success(),
        "render answered {}",
        response.status()
    );
    assert!(response.bytes().expect("no body").starts_with(b"%PDF"));
}

#[test]
fn ad_hoc_render_rejects_an_illegal_path() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", file_part("main.typ", TRIVIAL))
        .part("file", file_part("../escape.typ", TRIVIAL));

    assert_eq!(post_multipart(&service, "/render", form).status(), 400);
}

// -----------------------------------------------------------------------------
// Archival / ZUGFeRD
// -----------------------------------------------------------------------------

/// The reason this project exists. Not a full validation -- that needs Mustang
/// -- but it catches the failures that actually happened: the attachment
/// missing, the XMP block not being written, and the extension schema landing
/// in a second bag, which makes XMP reject the entire packet.
#[test]
fn an_archival_render_carries_zugferd_metadata() {
    let service = Service::start();
    let _directory = publish_invoice(&service, "zugferd");

    // The same request the invoice template ships for local work: whatever
    // renders in an editor has to render here too.
    let request: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/templates/invoice/__data/request.json"
    )))
    .expect("the example request must be json");

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/zugferd/1/render", service.base()))
        .json(&serde_json::json!({
            "document": { "type": "invoice", "profile": "en16931" },
            "data": request["data"],
        }))
        .send()
        .expect("the render request failed");

    assert!(
        response.status().is_success(),
        "render answered {}",
        response.status()
    );

    let pdf = response.bytes().expect("no body");
    let text = String::from_utf8_lossy(&pdf);

    assert!(text.contains("factur-x.xml"), "the xml was not attached");

    // The last packet is the one the incremental update appended, and the only
    // one a reader resolves -- the original stays in the file untouched, which
    // is the whole point of an incremental update.
    let packet = text
        .rfind("<?xpacket begin")
        .map(|start| &text[start..])
        .expect("the patched pdf carries no xmp packet");

    assert!(
        packet.contains("EN 16931"),
        "the xmp must state the profile the xml actually claims"
    );

    assert_eq!(
        packet.matches("<pdfaExtension:schemas>").count(),
        1,
        "a second extension bag invalidates the whole packet"
    );
}

#[test]
fn a_file_part_without_a_filename_is_rejected() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", reqwest::blocking::multipart::Part::text(TRIVIAL));

    assert_eq!(post_multipart(&service, "/render", form).status(), 400);
}

#[test]
fn too_many_files_are_rejected() {
    let service = Service::start();

    let mut form = reqwest::blocking::multipart::Form::new().text("data", "{}");

    for index in 0..65 {
        form = form.part("file", file_part(&format!("f{index}.typ"), TRIVIAL));
    }

    assert_eq!(post_multipart(&service, "/render", form).status(), 400);
}

#[test]
fn an_oversized_file_is_rejected() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", file_part("main.typ", &"x".repeat(9 * 1024 * 1024)));

    assert_eq!(post_multipart(&service, "/render", form).status(), 400);
}

#[test]
fn a_named_entrypoint_must_be_among_the_files() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .text("entrypoint", "nowhere.typ")
        .part("file", file_part("main.typ", TRIVIAL));

    assert_eq!(post_multipart(&service, "/render", form).status(), 400);
}

#[test]
fn a_bundle_without_main_typ_is_rejected() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", file_part("other.typ", TRIVIAL));

    assert_eq!(post_multipart(&service, "/render", form).status(), 400);
}

#[test]
fn a_missing_data_field_is_rejected() {
    let service = Service::start();

    let form =
        reqwest::blocking::multipart::Form::new().part("file", file_part("main.typ", TRIVIAL));

    assert_eq!(post_multipart(&service, "/render", form).status(), 400);
}

#[test]
fn the_server_rejects_a_template_that_does_not_compile() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .text("fixture", r#"{"data":{}}"#)
        .part(
            "file",
            file_part("main.typ", "#definitely-does-not-exist()"),
        );

    let response = post_multipart(&service, "/templates/broken", form);

    assert_eq!(response.status(), 422);
    assert!(
        response
            .text()
            .expect("no body")
            .contains("\"accepted\":false")
    );
}

#[test]
fn the_server_rejects_an_illegal_path_at_publish() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .text("fixture", r#"{"data":{}}"#)
        .part("file", file_part("main.typ", TRIVIAL))
        .part("file", file_part("../escape.typ", TRIVIAL));

    assert_eq!(
        post_multipart(&service, "/templates/escape", form).status(),
        422
    );
}

#[test]
fn an_invoice_needs_a_template_that_carries_one() {
    let service = Service::start();
    let (_temp, _directory) = publish_example(&service, "no-fixture");

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/no-fixture/1/render", service.base()))
        .json(&serde_json::json!({
            "document": { "type": "invoice", "profile": "en16931" },
            "data": { "title": "x", "message": "y" },
        }))
        .send()
        .expect("the request failed");

    assert_eq!(response.status(), 500);
}

#[test]
fn invoice_data_that_is_not_an_invoice_is_rejected() {
    let service = Service::start();
    let _directory = publish_invoice(&service, "shape");

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/shape/1/render", service.base()))
        .json(&serde_json::json!({
            "document": { "type": "invoice", "profile": "en16931" },
            "data": {},
        }))
        .send()
        .expect("the request failed");

    assert_eq!(response.status(), 422);
}

/// Parses as an invoice but does not hold up as one: every problem comes back
/// at once, so an integration is fixed in one pass rather than field by field.
#[test]
fn an_invalid_invoice_reports_every_problem() {
    let service = Service::start();
    let _directory = publish_invoice(&service, "invalid");

    let request: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/templates/invoice/__data/request.json"
    )))
    .expect("the example request must be json");

    let mut data = request["data"].clone();
    data["currency"] = "EURO".into();
    data["issued"] = "30.07.2026".into();

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/invalid/1/render", service.base()))
        .json(&serde_json::json!({
            "document": { "type": "invoice", "profile": "en16931" },
            "data": data,
        }))
        .send()
        .expect("the request failed");

    assert_eq!(response.status(), 422);

    let body = response.text().expect("no body");
    assert!(body.contains("currency"));
    assert!(body.contains("issued"));
}

#[test]
fn a_render_that_cannot_compile_is_a_client_error() {
    let service = Service::start();
    let (_temp, _directory) = publish_example(&service, "needs-data");

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/needs-data/1/render", service.base()))
        .json(&serde_json::json!({ "data": { "nothing": "useful" } }))
        .send()
        .expect("the request failed");

    assert_eq!(response.status(), 422);
}

#[test]
fn a_publish_with_an_unreadable_source_is_rejected() {
    let service = Service::start();

    // Structurally valid bundle, but `main.typ` is not UTF-8. This gets past
    // Files::validate() and fails while the engine prepares the Typst source,
    // exercising validate()'s non-template CompileError path.
    let form = reqwest::blocking::multipart::Form::new()
        .text("fixture", r#"{"data":{}}"#)
        .part("file", binary_file_part("main.typ", vec![0xff, 0xfe, 0xfd]));

    let response = post_multipart(&service, "/templates/non-utf8", form);

    assert_eq!(response.status(), 422);

    let body = response.text().expect("no body");
    assert!(
        body.contains("\"accepted\":false"),
        "failed validation should reject the publish: {body}"
    );
}

#[test]
fn a_corrupt_stored_invoice_fixture_is_an_internal_error() {
    let service = Service::start();

    // The publish fixture itself is valid so publishing succeeds. The separate
    // `fixture.json` belongs to the stored template and is deliberately corrupt;
    // invoice rendering reads that file as the issuer definition.
    let form = reqwest::blocking::multipart::Form::new()
        .text(
            "fixture",
            r#"{"data":{"title":"Valid","message":"Publish fixture"}}"#,
        )
        .part("file", file_part("main.typ", TRIVIAL))
        .part("file", file_part("fixture.json", "{ definitely not json"));

    let published = post_multipart(&service, "/templates/corrupt-fixture", form);

    assert_eq!(
        published.status(),
        201,
        "the template itself should publish successfully"
    );

    let response = reqwest::blocking::Client::new()
        .post(format!(
            "{}/templates/corrupt-fixture/1/render",
            service.base()
        ))
        .json(&serde_json::json!({
            "document": {
                "type": "invoice",
                "profile": "en16931"
            },
            "data": {}
        }))
        .send()
        .expect("the render request failed");

    assert_eq!(response.status(), 500);

    let body = response.text().expect("no body");
    assert!(
        body.contains("fixture.json"),
        "the error should identify the corrupt stored fixture: {body}"
    );
}

#[test]
fn a_storage_failure_after_validation_is_a_server_error() {
    let service = Service::start();
    let (_temp, directory) = initialized_template("storage-failure");

    // Startup and index loading have already succeeded. Turn the storage root
    // into a regular file so validation can still succeed but persistence
    // cannot create the published template below it.
    fs::remove_dir_all(service.data.path()).expect("failed to remove server data directory");

    fs::write(service.data.path(), b"not a directory").expect("failed to poison server data path");

    let output = publish(&service, "storage-failure", &directory, None);

    assert!(
        !output.status.success(),
        "publishing must fail when persistence becomes unavailable"
    );
}

#[test]
fn an_ad_hoc_source_that_the_engine_cannot_prepare_is_an_internal_error() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", binary_file_part("main.typ", vec![0xff, 0xfe, 0xfd]));

    let response = post_multipart(&service, "/render", form);

    assert_eq!(response.status(), 500);
}

#[test]
fn malformed_ad_hoc_data_json_is_rejected() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .text("data", "{ definitely not json")
        .part("file", file_part("main.typ", TRIVIAL));

    assert_eq!(post_multipart(&service, "/render", form).status(), 400);
}

#[test]
fn a_non_utf8_entrypoint_is_rejected() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .part(
            "entrypoint",
            reqwest::blocking::multipart::Part::bytes(vec![0xff, 0xfe]),
        )
        .text("data", "{}")
        .part("file", file_part("main.typ", TRIVIAL));

    assert_eq!(post_multipart(&service, "/render", form).status(), 400);
}

#[test]
fn a_non_utf8_publish_entrypoint_is_rejected() {
    let service = Service::start();

    let form = reqwest::blocking::multipart::Form::new()
        .part(
            "entrypoint",
            reqwest::blocking::multipart::Part::bytes(vec![0xff, 0xfe]),
        )
        .text("fixture", r#"{"data":{}}"#)
        .part("file", file_part("main.typ", TRIVIAL));

    assert_eq!(
        post_multipart(&service, "/templates/non-utf8-entrypoint", form).status(),
        400
    );
}

#[test]
fn a_request_without_a_valid_multipart_boundary_is_rejected() {
    let service = Service::start();

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/render", service.base()))
        .header(reqwest::header::CONTENT_TYPE, "multipart/form-data")
        .body("not multipart")
        .send()
        .expect("the request failed");

    assert_eq!(response.status(), 400);
}

#[test]
fn malformed_multipart_is_rejected_at_publish() {
    let service = Service::start();

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/templates/broken-multipart", service.base()))
        .header(reqwest::header::CONTENT_TYPE, "multipart/form-data")
        .body("not multipart")
        .send()
        .expect("the request failed");

    assert_eq!(response.status(), 400);
}

#[test]
fn truncated_multipart_body_is_rejected() {
    let service = Service::start();

    let boundary = "document-test-boundary";

    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"data\"\r\n\
         \r\n\
         {{}}\r\n\
         --{boundary}\r\n\
         Content-Disposition: form-data; name=\"file\"; filename=\"main.typ\"\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         {TRIVIAL}" // deliberately no closing boundary
    );

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/render", service.base()))
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .expect("the request failed");

    assert_eq!(response.status(), 400);
}

#[test]
fn archival_render_returns_500_when_artifact_storage_fails() {
    let service = Service::start();
    let (_temp, _directory) = publish_example(&service, "artifact-failure");

    // Keep the published template readable, but make the artifact namespace
    // unwritable: put_artifact() needs `artifacts/...` to be a directory.
    let artifacts = service.data.path().join("artifacts");

    if artifacts.exists() {
        fs::remove_dir_all(&artifacts).expect("failed to remove artifact directory");
    }

    fs::write(&artifacts, b"not a directory").expect("failed to poison artifact path");

    let response = reqwest::blocking::Client::new()
        .post(format!(
            "{}/templates/artifact-failure/1/render",
            service.base()
        ))
        .json(&serde_json::json!({
            "data": {
                "title": "Archive",
                "message": "Persistence must fail"
            },
            "archival": true
        }))
        .send()
        .expect("the request failed");

    assert_eq!(response.status(), 500);
}

#[test]
fn malformed_multipart_field_is_rejected() {
    let service = Service::start();
    let boundary = "document-test-boundary";

    let body = format!(
        "--{boundary}\r\n\
         Content-Disposition: form-data; name=\"data\"\r\n\
         \r\n\
         {{}}\r\n\
         --{boundary}\r\n\
         this is not a valid multipart header\r\n\
         \r\n\
         broken\r\n\
         --{boundary}--\r\n"
    );

    let response = reqwest::blocking::Client::new()
        .post(format!("{}/render", service.base()))
        .header(
            reqwest::header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .body(body)
        .send()
        .expect("the request failed");

    assert_eq!(response.status(), 400);
}

#[test]
fn a_missing_publish_fixture_is_rejected() {
    let service = Service::start();

    let form =
        reqwest::blocking::multipart::Form::new().part("file", file_part("main.typ", TRIVIAL));

    assert_eq!(
        post_multipart(&service, "/templates/missing-fixture", form).status(),
        400
    );
}

#[test]
fn server_fails_when_listen_address_is_in_use() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to reserve listen address");

    let port = listener
        .local_addr()
        .expect("reserved listener has no address")
        .port();

    let data = tempfile::tempdir().expect("failed to create server data directory");

    let output = server_command(port)
        .env("DOCUMENT_DATA_DIR", data.path())
        .output()
        .expect("failed to run document server");

    assert!(
        !output.status.success(),
        "server unexpectedly started on an occupied port"
    );
}

#[cfg(unix)]
#[test]
fn server_without_storage_uses_memory_and_handles_sigint() {
    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };

    let port = free_port();

    // server_command() removes both DOCUMENT_DATA_DIR and DOCUMENT_S3_BUCKET,
    // therefore open_storage() must take the in-memory fallback.
    let mut child = server_command(port)
        .spawn()
        .expect("failed to start memory-backed server");

    wait_for_service(&mut child, port);

    kill(Pid::from_raw(child.id().cast_signed()), Signal::SIGINT).expect("failed to send SIGINT");

    let status = child.wait().expect("failed to wait for server");

    assert!(
        status.success(),
        "server did not shut down cleanly after SIGINT: {status}"
    );
}

#[test]
fn server_starts_with_local_rustfs() {
    let Some(rustfs) = RustFs::start() else {
        return;
    };

    let port = free_port();

    let mut child = server_command(port)
        .env("DOCUMENT_S3_BUCKET", RUSTFS_BUCKET)
        .env("DOCUMENT_S3_ENDPOINT", &rustfs.endpoint)
        .env("DOCUMENT_S3_ACCESS_KEY_ID", RUSTFS_ACCESS_KEY)
        .env("DOCUMENT_S3_SECRET_ACCESS_KEY", RUSTFS_SECRET_KEY)
        .env("DOCUMENT_S3_ALLOW_HTTP", "1")
        .spawn()
        .expect("failed to start S3-backed document server");

    wait_for_service(&mut child, port);

    #[cfg(unix)]
    {
        use nix::{
            sys::signal::{Signal, kill},
            unistd::Pid,
        };

        kill(Pid::from_raw(child.id().cast_signed()), Signal::SIGTERM)
            .expect("failed to stop document server");
    }

    #[cfg(not(unix))]
    child.kill().expect("failed to stop document server");

    let status = child.wait().expect("failed to wait for document server");

    assert!(
        status.success(),
        "S3-backed document server did not shut down cleanly: {status}"
    );
}

#[test]
fn server_fails_with_incomplete_rustfs_credentials() {
    let Some(rustfs) = RustFs::start() else {
        return;
    };

    let port = free_port();

    let mut child = server_command(port)
        .env("DOCUMENT_S3_BUCKET", RUSTFS_BUCKET)
        .env("DOCUMENT_S3_ENDPOINT", &rustfs.endpoint)
        .env("DOCUMENT_S3_ACCESS_KEY_ID", RUSTFS_ACCESS_KEY)
        .env_remove("DOCUMENT_S3_SECRET_ACCESS_KEY")
        .env("DOCUMENT_S3_ALLOW_HTTP", "1")
        .spawn()
        .expect("failed to start document server");

    let status = child.wait().expect("failed to wait for document server");

    assert!(
        !status.success(),
        "document server unexpectedly started with incomplete S3 credentials"
    );
}

#[tokio::test]
async fn rustfs_supports_conditional_put() {
    use object_store::{
        ObjectStore, ObjectStoreExt, aws::AmazonS3Builder, path::Path as ObjectPath,
    };

    let Some(rustfs) = RustFs::start() else {
        return;
    };

    let store = AmazonS3Builder::new()
        .with_bucket_name(RUSTFS_BUCKET)
        .with_region("us-east-1")
        .with_endpoint(&rustfs.endpoint)
        .with_access_key_id(RUSTFS_ACCESS_KEY)
        .with_secret_access_key(RUSTFS_SECRET_KEY)
        .with_allow_http(true)
        .build()
        .expect("build RustFS client");

    let path = ObjectPath::from(format!(
        "probe/conditional-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));

    let first = store
        .put_opts(
            &path,
            object_store::PutPayload::from_static(b"x"),
            object_store::PutMode::Create.into(),
        )
        .await;

    let second = store
        .put_opts(
            &path,
            object_store::PutPayload::from_static(b"y"),
            object_store::PutMode::Create.into(),
        )
        .await;

    // Clean up before asserting, so a failing assertion leaves nothing behind.
    let _ = store.delete(&path).await;

    first.expect("first conditional write should succeed");

    match second {
        Err(object_store::Error::AlreadyExists { .. }) => {}
        Err(error) => panic!("conditional put not supported: {error}"),
        // The dangerous case: silently overwritten, so `n+1` versioning would
        // not be safe against concurrent publishes.
        Ok(_) => panic!("conditional put silently overwrote"),
    }
}

#[test]
fn server_fails_when_data_directory_is_not_a_directory() {
    let root = tempfile::tempdir().expect("failed to create temporary directory");
    let data = root.path().join("data");

    fs::write(&data, b"not a directory").expect("failed to create file in place of data directory");

    let output = server_command(free_port())
        .env("DOCUMENT_DATA_DIR", &data)
        .output()
        .expect("failed to run document server");

    assert!(
        !output.status.success(),
        "server unexpectedly started with an unusable data directory"
    );
}

#[test]
fn server_fails_when_template_index_is_corrupt() {
    let data = tempfile::tempdir().expect("failed to create server data directory");

    let manifest = data.path().join("templates/example/1/manifest.json");

    fs::create_dir_all(manifest.parent().expect("manifest has no parent directory"))
        .expect("failed to create template directory");

    fs::write(&manifest, b"{not json").expect("failed to write corrupt manifest");

    let output = server_command(free_port())
        .env("DOCUMENT_DATA_DIR", data.path())
        .output()
        .expect("failed to run document server");

    assert!(
        !output.status.success(),
        "server unexpectedly started with a corrupt template index"
    );
}

#[test]
fn server_fails_when_s3_is_unreachable() {
    let port = free_port();

    let output = server_command(port)
        .env("DOCUMENT_S3_BUCKET", "document-test")
        .env("DOCUMENT_S3_REGION", "us-east-1")
        .env("DOCUMENT_S3_ENDPOINT", "http://127.0.0.1:1")
        .env("DOCUMENT_S3_ACCESS_KEY_ID", "document-test")
        .env("DOCUMENT_S3_SECRET_ACCESS_KEY", "document-test-secret")
        .env("DOCUMENT_S3_ALLOW_HTTP", "1")
        .output()
        .expect("failed to run document server");

    assert!(
        !output.status.success(),
        "server unexpectedly started with unreachable S3 storage"
    );
}

#[test]
fn server_rejects_zero_workers() {
    let output = server_command(free_port())
        .env("DOCUMENT_WORKERS", "0")
        .output()
        .expect("failed to run document server");

    assert!(
        !output.status.success(),
        "server unexpectedly started with zero workers"
    );
}

#[cfg(unix)]
#[test]
fn worker_spawn_failure_is_reported_after_discard() {
    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };

    let executable_dir = tempfile::tempdir().expect("temporary executable directory");
    let executable = executable_dir.path().join("document");

    fs::copy(EXE, &executable).expect("copy document executable");

    let data = tempfile::tempdir().expect("server data directory");
    let port = free_port();

    let mut child = server_command_from(&executable, port)
        .env("DOCUMENT_DATA_DIR", data.path())
        .env("DOCUMENT_WORKERS", "1")
        .spawn()
        .expect("start document server");

    wait_for_service(&mut child, port);

    // The server and its single warmed worker are already running.
    // Unix allows unlinking a running executable.
    fs::remove_file(&executable).expect("remove worker executable");

    let client = reqwest::blocking::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // This reaches Engine::render(), produces a non-template CompileError,
    // execute_job() returns Err, and the worker process exits.
    let broken = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", binary_file_part("main.typ", vec![0xff, 0xfe, 0xfd]));

    let response = client
        .post(format!("{base}/render"))
        .multipart(broken)
        .send()
        .expect("broken render request");

    assert_eq!(response.status(), 500);

    // The previous worker was discarded. The pool is now empty.
    // This request forces Pool::take() -> Worker::spawn(), but the executable
    // path was removed above, so spawn must fail.
    let valid = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", file_part("main.typ", "= Fine"));

    let response = client
        .post(format!("{base}/render"))
        .multipart(valid)
        .send()
        .expect("respawn request");

    assert_eq!(response.status(), 500);

    kill(Pid::from_raw(child.id().cast_signed()), Signal::SIGTERM).expect("stop document server");

    let status = child.wait().expect("wait for document server");
    assert!(status.success());
}

#[test]
fn exhausted_worker_is_recycled() {
    let service = Service::with_env(&[("DOCUMENT_WORKERS", "1")]);
    let (_temp, _directory) = publish_example(&service, "recycle");

    let client = reqwest::blocking::Client::new();
    let url = format!("{}/templates/recycle/1/render", service.base());

    for job in 0..500 {
        let response = client
            .post(&url)
            .json(&serde_json::json!({
                "data": {
                    "title": "Recycle",
                    "message": format!("Job {job}")
                }
            }))
            .send()
            .expect("render request failed");

        assert!(
            response.status().is_success(),
            "render {job} failed with {}",
            response.status()
        );
    }

    // The 500th job exhausts the only worker. Pool::give_back() must discard it.
    // The next request therefore has to spawn a replacement worker.
    let response = client
        .post(&url)
        .json(&serde_json::json!({
            "data": {
                "title": "Recycle",
                "message": "Replacement worker"
            }
        }))
        .send()
        .expect("replacement render request failed");

    assert!(
        response.status().is_success(),
        "replacement worker failed with {}",
        response.status()
    );
}

#[cfg(unix)]
#[test]
fn malformed_worker_response_is_an_internal_error() {
    use std::os::unix::fs::PermissionsExt;

    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };

    let executable_dir = tempfile::tempdir().expect("temporary executable directory");
    let executable = executable_dir.path().join("document");

    fs::copy(EXE, &executable).expect("copy document executable");

    let data = tempfile::tempdir().expect("server data directory");
    let port = free_port();

    let mut child = server_command_from(&executable, port)
        .env("DOCUMENT_DATA_DIR", data.path())
        .env("DOCUMENT_WORKERS", "1")
        .spawn()
        .expect("start document server");

    wait_for_service(&mut child, port);

    let client = reqwest::blocking::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // The server and its single worker are already running from the copied
    // executable. Unlinking it does not affect either running process.
    fs::remove_file(&executable).expect("remove document executable");

    // Any worker spawned from now on writes two syntactically valid protocol
    // frames:
    //
    //   outcome: b"not json"
    //   pdf:     b""
    //
    // The framing is valid; only the JobResult payload is corrupt. That forces
    // the real parent process through decode_outcome()'s JSON error path.
    fs::write(
        &executable,
        b"#!/bin/sh\n\
          printf '\\010\\000\\000\\000not json\\000\\000\\000\\000'\n",
    )
    .expect("write fake worker");

    let mut permissions = fs::metadata(&executable)
        .expect("fake worker metadata")
        .permissions();

    permissions.set_mode(0o755);

    fs::set_permissions(&executable, permissions).expect("make fake worker executable");

    // First kill/discard the worker that was warmed when the real server
    // executable still existed.
    //
    // The non-UTF-8 Typst source gets through framing but makes execute_job()
    // return Err, so the worker exits and compile_isolated_on() discards it.
    let broken = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", binary_file_part("main.typ", vec![0xff, 0xfe, 0xfd]));

    let response = client
        .post(format!("{base}/render"))
        .multipart(broken)
        .send()
        .expect("broken render request");

    assert_eq!(response.status(), 500);

    // The pool is empty now. This request causes:
    //
    // Pool::take()
    //   -> Worker::spawn(fake executable)
    //   -> Worker::run()
    //   -> run_io()
    //   -> correctly framed "not json"
    //   -> decode_outcome()
    //   -> serde_json::from_slice::<JobResult>() fails
    //
    // The HTTP layer must surface that as an internal error.
    let valid = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", file_part("main.typ", "= Fine"));

    let response = client
        .post(format!("{base}/render"))
        .multipart(valid)
        .send()
        .expect("malformed worker response request");

    assert_eq!(response.status(), 500);

    kill(Pid::from_raw(child.id().cast_signed()), Signal::SIGTERM).expect("stop document server");

    let status = child.wait().expect("wait for document server");

    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn worker_that_omits_pdf_frame_is_discarded() {
    use std::os::unix::fs::PermissionsExt;

    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };

    let executable_dir = tempfile::tempdir().expect("temporary executable directory");
    let executable = executable_dir.path().join("document");

    fs::copy(EXE, &executable).expect("copy document executable");

    let data = tempfile::tempdir().expect("server data directory");
    let port = free_port();

    let mut child = server_command_from(&executable, port)
        .env("DOCUMENT_DATA_DIR", data.path())
        .env("DOCUMENT_WORKERS", "1")
        .spawn()
        .expect("start document server");

    wait_for_service(&mut child, port);

    let client = reqwest::blocking::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // The server and its initially warmed worker keep running after the
    // executable is unlinked.
    fs::remove_file(&executable).expect("remove document executable");

    // Replacement worker:
    //
    // - keep consuming stdin so the parent's write_frame() succeeds;
    // - write one correctly framed outcome;
    // - exit without writing the required PDF frame.
    //
    // The outcome bytes themselves do not need to be valid JobResult JSON,
    // because run_io() must read both frames before decode_outcome() runs.
    fs::write(
        &executable,
        b"#!/bin/sh\n\
          (cat >/dev/null) &\n\
          printf '\\010\\000\\000\\000not json'\n\
          exit 0\n",
    )
    .expect("write fake worker");

    let mut permissions = fs::metadata(&executable)
        .expect("fake worker metadata")
        .permissions();

    permissions.set_mode(0o755);

    fs::set_permissions(&executable, permissions).expect("make fake worker executable");

    // Discard the worker that was warmed from the real executable.
    let broken = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", binary_file_part("main.typ", vec![0xff, 0xfe, 0xfd]));

    let response = client
        .post(format!("{base}/render"))
        .multipart(broken)
        .send()
        .expect("broken render request");

    assert_eq!(response.status(), 500);

    // Pool is empty now, so this request spawns our replacement.
    //
    // run_io():
    //   write_frame()       -> succeeds
    //   read outcome frame  -> succeeds ("not json")
    //   read PDF frame      -> EOF/error
    //
    // compile_isolated_on() therefore treats the worker as failed/discarded.
    let valid = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", file_part("main.typ", "= Fine"));

    let response = client
        .post(format!("{base}/render"))
        .multipart(valid)
        .send()
        .expect("truncated worker response request");

    assert_eq!(response.status(), 500);

    kill(Pid::from_raw(child.id().cast_signed()), Signal::SIGTERM).expect("stop document server");

    let status = child.wait().expect("wait for document server");

    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn worker_that_exits_without_a_response_is_discarded() {
    use std::os::unix::fs::PermissionsExt;

    use nix::{
        sys::signal::{Signal, kill},
        unistd::Pid,
    };

    let executable_dir = tempfile::tempdir().expect("temporary executable directory");
    let executable = executable_dir.path().join("document");

    fs::copy(EXE, &executable).expect("copy document executable");

    let data = tempfile::tempdir().expect("server data directory");
    let port = free_port();

    let mut child = server_command_from(&executable, port)
        .env("DOCUMENT_DATA_DIR", data.path())
        .env("DOCUMENT_WORKERS", "1")
        .spawn()
        .expect("start document server");

    wait_for_service(&mut child, port);

    let client = reqwest::blocking::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // The already-running server and warmed worker survive the unlink.
    fs::remove_file(&executable).expect("remove document executable");

    // Replacement worker:
    //
    // Read stdin until EOF so the parent's write_frame() can complete, then
    // exit without writing even the first response frame.
    fs::write(
        &executable,
        b"#!/bin/sh\n\
          cat >/dev/null\n\
          exit 0\n",
    )
    .expect("write fake worker");

    let mut permissions = fs::metadata(&executable)
        .expect("fake worker metadata")
        .permissions();

    permissions.set_mode(0o755);

    fs::set_permissions(&executable, permissions).expect("make fake worker executable");

    // Discard the worker that was warmed from the real executable.
    let broken = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", binary_file_part("main.typ", vec![0xff, 0xfe, 0xfd]));

    let response = client
        .post(format!("{base}/render"))
        .multipart(broken)
        .send()
        .expect("broken render request");

    assert_eq!(response.status(), 500);

    // Pool::take() now spawns our fake worker.
    //
    // run_io():
    //
    // write_frame(stdin, payload) -> succeeds
    // read_frame(stdout)          -> EOF/error
    //
    // The second read is never reached.
    let valid = reqwest::blocking::multipart::Form::new()
        .text("data", "{}")
        .part("file", file_part("main.typ", "= Fine"));

    let response = client
        .post(format!("{base}/render"))
        .multipart(valid)
        .send()
        .expect("missing worker response request");

    assert_eq!(response.status(), 500);

    kill(Pid::from_raw(child.id().cast_signed()), Signal::SIGTERM).expect("stop document server");

    let status = child.wait().expect("wait for document server");

    assert!(status.success());
}
