//! What a user actually types.
//!
//! `document` is a binary crate, so these cannot reach inside it -- they run
//! the built executable the way anyone else would. That is the point: unit
//! tests cover the pieces, this covers the promises.

use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
};

const EXE: &str = env!("CARGO_BIN_EXE_document");

fn document() -> Command {
    Command::new(EXE)
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

fn assert_failure(output: &Output) {
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded\n\nstdout:\n{}\n\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn temp() -> tempfile::TempDir {
    tempfile::tempdir().expect("failed to create temporary directory")
}

fn init_template(parent: &Path, name: &str) -> PathBuf {
    let directory = parent.join(name);

    let output = run(document().args(["template", "init"]).arg(&directory));

    assert_success(&output);

    directory
}

/// Starts one deliberately tiny HTTP server.
///
/// It accepts exactly one request and responds with the supplied status/body.
/// That is enough to exercise the CLI's HTTP contract without starting the
/// document server itself -- server behaviour has its own integration tests.
fn server(status: &str, body: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind test server");

    let address = listener
        .local_addr()
        .expect("failed to read test server address");

    let status = status.to_owned();
    let body = body.to_owned();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("failed to accept request");

        let mut buffer = [0_u8; 16 * 1024];

        // Reading once is enough for these tiny local requests. The response
        // closes the connection, so reqwest does not wait for another request.
        let _ = stream.read(&mut buffer);

        let response = format!(
            "HTTP/1.1 {status}\r\n\
             Content-Type: application/json\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n\
             {body}",
            body.len(),
        );

        stream
            .write_all(response.as_bytes())
            .expect("failed to write test response");
    });

    format!("http://{address}")
}

// -----------------------------------------------------------------------------
// Root CLI
// -----------------------------------------------------------------------------

#[test]
fn help_succeeds() {
    let output = run(document().arg("--help"));

    assert_success(&output);
    assert!(stdout(&output).contains("Deterministic PDFs from Typst templates"));
}

#[test]
fn version_succeeds() {
    let output = run(document().arg("--version"));

    assert_success(&output);
    assert!(stdout(&output).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn an_unknown_command_fails() {
    let output = run(document().arg("definitely-not-a-command"));

    assert_failure(&output);
}

#[test]
fn compile_help_succeeds() {
    assert_success(&run(document().args(["compile", "--help"])));
}

#[test]
fn template_help_succeeds() {
    assert_success(&run(document().args(["template", "--help"])));
}

#[test]
fn serve_help_succeeds() {
    assert_success(&run(document().args(["serve", "--help"])));
}

// -----------------------------------------------------------------------------
// template init
// -----------------------------------------------------------------------------

/// The starter template has to render. It is what `init` writes, what the
/// container serves, and the first thing a new user sees.
#[test]
fn the_starter_template_passes() {
    let temp = temp();
    let directory = init_template(temp.path(), "minimal");

    assert!(directory.join("main.typ").is_file());
    assert!(directory.join("__data/request.json").is_file());

    let check = run(document().args(["template", "check"]).arg(&directory));

    assert_success(&check);
    assert!(stdout(&check).contains("PASSED"));
}

#[test]
fn init_can_compile_the_starter_template() {
    let temp = temp();
    let directory = temp.path().join("invoice");

    let output = run(document()
        .args(["template", "init"])
        .arg(&directory)
        .arg("--compile"));

    assert_success(&output);
    assert!(directory.join("invoice.pdf").is_file());

    let stdout = stdout(&output);
    assert!(stdout.contains("CREATED"));
    assert!(stdout.contains("COMPILED"));
}

#[test]
fn init_refuses_an_existing_template() {
    let temp = temp();
    let directory = temp.path().join("existing");

    fs::create_dir(&directory).expect("failed to create existing directory");
    fs::write(directory.join("main.typ"), b"= mine").expect("failed to write template");

    let output = run(document().args(["template", "init"]).arg(&directory));

    assert_failure(&output);
    assert!(stderr(&output).contains("already holds a template"));
}

/// A prepared but empty directory is the common case for `init .`.
#[test]
fn init_accepts_an_existing_empty_directory() {
    let temp = temp();
    let directory = temp.path().join("prepared");

    fs::create_dir(&directory).expect("failed to create existing directory");

    let output = run(document().args(["template", "init"]).arg(&directory));

    assert_success(&output);
    assert!(directory.join("main.typ").is_file());
}

#[test]
fn init_fails_when_parent_is_a_file() {
    let temp = temp();
    let parent = temp.path().join("file");

    fs::write(&parent, b"not a directory").expect("failed to create file");

    let output = run(document()
        .args(["template", "init"])
        .arg(parent.join("template")));

    assert_failure(&output);
}

/// `__data/` cannot be created when a file already sits at that name.
#[test]
fn init_fails_when_the_data_directory_is_blocked() {
    let temp = temp();
    let directory = temp.path().join("blocked-data");

    fs::create_dir(&directory).expect("failed to create directory");
    fs::write(directory.join("__data"), b"not a directory").expect("failed to block __data");

    let output = run(document().args(["template", "init"]).arg(&directory));

    assert_failure(&output);
}

/// A target file cannot be written over a directory of the same name.
#[test]
fn init_fails_when_a_target_file_is_a_directory() {
    let temp = temp();
    let directory = temp.path().join("blocked-file");

    fs::create_dir_all(directory.join("__data/request.json")).expect("failed to block the target");

    let output = run(document().args(["template", "init"]).arg(&directory));

    assert_failure(&output);
}

/// `--compile` reports a failing render rather than claiming success.
#[test]
fn init_reports_a_failing_render() {
    let temp = temp();
    let directory = temp.path().join("no-pdf");

    // The render writes `<name>.pdf`; a directory there makes it fail.
    fs::create_dir_all(directory.join("no-pdf.pdf")).expect("failed to block the output");

    let output = run(document()
        .args(["template", "init"])
        .arg(&directory)
        .arg("--compile"));

    assert_failure(&output);
}

// -----------------------------------------------------------------------------
// template check
// -----------------------------------------------------------------------------

/// The gate has to fail when it should. A check that only ever succeeds is not
/// a gate.
#[test]
fn a_missing_entrypoint_fails() {
    let temp = temp();
    let directory = init_template(temp.path(), "broken");

    fs::remove_file(directory.join("main.typ")).expect("failed to remove the entrypoint");

    let output = run(document().args(["template", "check"]).arg(&directory));

    assert_failure(&output);
    assert!(stdout(&output).contains("FAILED"));
}

#[test]
fn a_missing_request_fails() {
    let temp = temp();
    let directory = init_template(temp.path(), "missing-request");

    fs::remove_file(directory.join("__data/request.json")).expect("failed to remove request");

    let output = run(document().args(["template", "check"]).arg(&directory));

    assert_failure(&output);
}

#[test]
fn malformed_request_json_fails() {
    let temp = temp();
    let directory = init_template(temp.path(), "invalid-json");

    fs::write(
        directory.join("__data/request.json"),
        b"{ definitely not json",
    )
    .expect("failed to corrupt request");

    let output = run(document().args(["template", "check"]).arg(&directory));

    assert_failure(&output);
}

#[test]
fn broken_typst_fails() {
    let temp = temp();
    let directory = init_template(temp.path(), "invalid-typst");

    fs::write(
        directory.join("main.typ"),
        "#this-function-does-not-exist()",
    )
    .expect("failed to corrupt template");

    let output = run(document().args(["template", "check"]).arg(&directory));

    assert_failure(&output);
    assert!(stdout(&output).contains("FAILED"));
}

#[test]
fn check_accepts_explicit_data() {
    let temp = temp();
    let directory = init_template(temp.path(), "explicit-data");
    let data = temp.path().join("custom.json");

    fs::copy(directory.join("__data/request.json"), &data).expect("failed to copy request");

    let output = run(document()
        .args(["template", "check"])
        .arg(&directory)
        .arg("--data")
        .arg(&data));

    assert_success(&output);
}

#[test]
fn check_accepts_an_explicit_entrypoint() {
    let temp = temp();
    let directory = init_template(temp.path(), "entrypoint");

    fs::rename(directory.join("main.typ"), directory.join("document.typ"))
        .expect("failed to rename entrypoint");

    let output = run(document()
        .args(["template", "check"])
        .arg(&directory)
        .arg("--entrypoint")
        .arg("document.typ"));

    assert_success(&output);
}

// -----------------------------------------------------------------------------
// template hash
// -----------------------------------------------------------------------------

#[test]
fn hash_prints_a_content_hash() {
    let temp = temp();
    let directory = init_template(temp.path(), "hash");

    let output = run(document().args(["template", "hash"]).arg(&directory));

    assert_success(&output);

    let hash = stdout(&output);
    let hash = hash.trim();

    assert!(!hash.is_empty());
    assert!(!hash.contains(char::is_whitespace));
}

#[test]
fn hash_fails_for_a_missing_bundle() {
    let temp = temp();

    let output = run(document()
        .args(["template", "hash"])
        .arg(temp.path().join("missing")));

    assert_failure(&output);
}

// -----------------------------------------------------------------------------
// template publish
// -----------------------------------------------------------------------------

#[test]
fn publish_succeeds() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish");

    let server = server(
        "200 OK",
        r#"{"version":1,"content_hash":"0123456789abcdef0123456789abcdef"}"#,
    );

    let output = run(document()
        .env("DOCUMENT_TOKEN", "")
        .args(["template", "publish", "invoice"])
        .arg(&directory)
        .arg("--server")
        .arg(&server));

    assert_success(&output);
    assert!(stdout(&output).contains("PUBLISHED"));
    assert!(stdout(&output).contains("invoice v1"));
}

#[test]
fn publish_uses_a_configured_token() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish-auth");

    let server = server(
        "200 OK",
        r#"{"version":2,"content_hash":"0123456789abcdef0123456789abcdef"}"#,
    );

    let output = run(document()
        .env("DOCUMENT_TOKEN", "test-token")
        .args(["template", "publish", "invoice"])
        .arg(&directory)
        .arg("--server")
        .arg(&server));

    assert_success(&output);
}

#[test]
fn publish_surfaces_server_validation_errors() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish-rejected");

    let server = server(
        "422 Unprocessable Entity",
        r#"{"errors":["template is broken"]}"#,
    );

    let output = run(document()
        .args(["template", "publish", "invoice"])
        .arg(&directory)
        .arg("--server")
        .arg(&server));

    assert_failure(&output);
    assert!(stderr(&output).contains("template is broken"));
}

#[test]
fn publish_handles_an_unstructured_server_error() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish-error");

    let server = server("500 Internal Server Error", r#"{"message":"nope"}"#);

    let output = run(document()
        .args(["template", "publish", "invoice"])
        .arg(&directory)
        .arg("--server")
        .arg(&server));

    assert_failure(&output);
    assert!(stderr(&output).contains("server answered 500"));
}

#[test]
fn publish_rejects_an_invalid_success_response() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish-invalid-response");

    let server = server("200 OK", r#"{"not":"a published version"}"#);

    let output = run(document()
        .args(["template", "publish", "invoice"])
        .arg(&directory)
        .arg("--server")
        .arg(&server));

    assert_failure(&output);
}

#[test]
fn publish_fails_when_the_server_cannot_be_reached() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish-offline");

    // Port zero cannot be used as an HTTP destination.
    let output = run(document()
        .args(["template", "publish", "invoice"])
        .arg(&directory)
        .arg("--server")
        .arg("http://127.0.0.1:0"));

    assert_failure(&output);
}

#[test]
fn publish_rejects_malformed_request_json() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish-json");

    fs::write(directory.join("__data/request.json"), b"{ nope").expect("failed to corrupt request");

    let output = run(document()
        .args(["template", "publish", "invoice"])
        .arg(&directory));

    assert_failure(&output);
}

#[test]
fn publish_accepts_explicit_data() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish-explicit-data");
    let data = temp.path().join("custom.json");

    fs::copy(directory.join("__data/request.json"), &data).expect("failed to copy request");

    let server = server(
        "200 OK",
        r#"{"version":1,"content_hash":"0123456789abcdef0123456789abcdef"}"#,
    );

    let output = run(document()
        .args(["template", "publish", "invoice"])
        .arg(&directory)
        .arg("--data")
        .arg(&data)
        .arg("--server")
        .arg(&server));

    assert_success(&output);
}

// -----------------------------------------------------------------------------
// template list
// -----------------------------------------------------------------------------

#[test]
fn list_prints_pretty_json() {
    let server = server("200 OK", r#"[{"id":"invoice","versions":[1,2]}]"#);

    let output = run(document()
        .env("DOCUMENT_TOKEN", "")
        .args(["template", "list", "--server"])
        .arg(&server));

    assert_success(&output);
    assert!(stdout(&output).contains("\"invoice\""));
}

#[test]
fn list_uses_a_configured_token() {
    let server = server("200 OK", "[]");

    let output = run(document()
        .env("DOCUMENT_TOKEN", "test-token")
        .args(["template", "list", "--server"])
        .arg(&server));

    assert_success(&output);
}

#[test]
fn list_rejects_invalid_json() {
    let server = server("200 OK", "not json");

    let output = run(document()
        .args(["template", "list", "--server"])
        .arg(&server));

    assert_failure(&output);
}

#[test]
fn list_fails_on_server_error() {
    let server = server("500 Internal Server Error", r#"{"error":"broken"}"#);

    let output = run(document()
        .args(["template", "list", "--server"])
        .arg(&server));

    assert_failure(&output);
    assert!(stderr(&output).contains("server answered 500"));
}

#[test]
fn list_fails_when_server_is_unreachable() {
    let output = run(document().args(["template", "list", "--server", "http://127.0.0.1:0"]));

    assert_failure(&output);
}

// -----------------------------------------------------------------------------
// template get
// -----------------------------------------------------------------------------

#[test]
fn get_prints_a_manifest() {
    let server = server(
        "200 OK",
        r#"{"template_id":"invoice","version":3,"files":[]}"#,
    );

    let output = run(document()
        .args(["template", "get", "invoice", "3", "--server"])
        .arg(&server));

    assert_success(&output);
    assert!(stdout(&output).contains("\"version\": 3"));
}

#[test]
fn get_rejects_invalid_json() {
    let server = server("200 OK", "not json");

    let output = run(document()
        .args(["template", "get", "invoice", "1", "--server"])
        .arg(&server));

    assert_failure(&output);
}

#[test]
fn get_fails_on_server_error() {
    let server = server("404 Not Found", r#"{"error":"missing"}"#);

    let output = run(document()
        .args(["template", "get", "invoice", "99", "--server"])
        .arg(&server));

    assert_failure(&output);
}

// -----------------------------------------------------------------------------
// compile
// -----------------------------------------------------------------------------

#[test]
fn compile_writes_an_explicit_output() {
    let temp = temp();
    let directory = init_template(temp.path(), "compile");
    let pdf = temp.path().join("result.pdf");

    let output = run(document()
        .arg("compile")
        .arg(directory.join("main.typ"))
        .arg("--output")
        .arg(&pdf));

    assert_success(&output);
    assert!(pdf.is_file());
    assert!(stdout(&output).contains("COMPILED"));
}

#[test]
fn compile_uses_the_default_output_name() {
    let temp = temp();
    let directory = init_template(temp.path(), "default-output");

    let output = run(document()
        .current_dir(temp.path())
        .arg("compile")
        .arg(directory.join("main.typ")));

    assert_success(&output);
    assert!(temp.path().join("main.pdf").is_file());
}

#[test]
fn compile_fails_for_a_missing_input() {
    let temp = temp();

    let output = run(document()
        .arg("compile")
        .arg(temp.path().join("missing.typ")));

    assert_failure(&output);
}

#[test]
fn compile_rejects_malformed_request_json() {
    let temp = temp();
    let directory = init_template(temp.path(), "compile-json");

    fs::write(directory.join("__data/request.json"), b"{ nope").expect("failed to corrupt request");

    let output = run(document().arg("compile").arg(directory.join("main.typ")));

    assert_failure(&output);
}

#[test]
fn compile_rejects_broken_typst() {
    let temp = temp();
    let directory = init_template(temp.path(), "compile-typst");

    fs::write(directory.join("main.typ"), "#definitely-does-not-exist()")
        .expect("failed to corrupt template");

    let output = run(document().arg("compile").arg(directory.join("main.typ")));

    assert_failure(&output);
}

#[test]
fn compile_fails_when_output_is_a_directory() {
    let temp = temp();
    let directory = init_template(temp.path(), "compile-output");
    let output_directory = temp.path().join("output");

    fs::create_dir(&output_directory).expect("failed to create output directory");

    let output = run(document()
        .arg("compile")
        .arg(directory.join("main.typ"))
        .arg("--output")
        .arg(&output_directory));

    assert_failure(&output);
}

#[test]
fn compile_fails_without_a_filename() {
    let output = run(document().arg("compile").arg(".."));

    assert_failure(&output);
}

#[test]
fn compile_fails_without_a_parent_directory() {
    let output = run(document().arg("compile").arg("/"));

    assert_failure(&output);
}

#[test]
fn compile_fails_for_a_missing_directory() {
    let temp = temp();

    let output = run(document()
        .arg("compile")
        .arg(temp.path().join("nope").join("main.typ")));

    assert_failure(&output);
}

#[test]
fn compile_fails_when_the_request_is_unreadable() {
    let temp = temp();
    let directory = init_template(temp.path(), "unreadable-request");
    let path = directory.join("__data/request.json");

    fs::remove_file(&path).expect("failed to remove request");
    fs::create_dir(&path).expect("failed to create a directory in its place");

    let output = run(document().arg("compile").arg(directory.join("main.typ")));

    assert_failure(&output);
}

#[test]
fn compile_works_without_request_data() {
    let temp = temp();
    let directory = temp.path().join("no-data");

    fs::create_dir(&directory).expect("failed to create directory");
    fs::write(directory.join("main.typ"), "= Nothing to fill in")
        .expect("failed to write template");

    let output = run(document()
        .current_dir(&directory)
        .arg("compile")
        .arg(directory.join("main.typ")));

    assert_success(&output);
}

#[test]
fn compile_prints_warnings() {
    let temp = temp();
    let directory = temp.path().join("warns");

    fs::create_dir(&directory).expect("failed to create directory");
    fs::write(
        directory.join("main.typ"),
        "#set text(font: \"No Such Font\")\n= Heading",
    )
    .expect("failed to write template");

    let output = run(document()
        .current_dir(&directory)
        .arg("compile")
        .arg(directory.join("main.typ")));

    assert_success(&output);
    assert!(stderr(&output).contains("warning:"));
}

#[test]
fn compile_rejects_a_non_utf8_source_file() {
    let temp = temp();
    let directory = init_template(temp.path(), "not-utf8");

    fs::write(directory.join("broken.typ"), [0xff, 0xfe, 0xfd])
        .expect("failed to write invalid utf-8");

    let output = run(document().arg("compile").arg(directory.join("main.typ")));

    assert_failure(&output);
}

// -----------------------------------------------------------------------------
// Styling
//
// stdout is a pipe under test, so the colour path is never taken by default --
// `CLICOLOR_FORCE` is what a CI with colour enabled sets, and it is the only
// way to see the styled branch from out here.
// -----------------------------------------------------------------------------

#[test]
fn a_verdict_is_coloured_when_forced() {
    let temp = temp();
    let directory = init_template(temp.path(), "coloured");

    let output = run(document()
        .env("CLICOLOR_FORCE", "1")
        .args(["template", "check"])
        .arg(&directory));

    assert_success(&output);
    assert!(
        stdout(&output).contains('\u{1b}'),
        "the label is not styled"
    );
}

#[test]
fn a_failed_verdict_is_coloured_when_forced() {
    let temp = temp();
    let directory = init_template(temp.path(), "coloured-failure");

    fs::remove_file(directory.join("main.typ")).expect("failed to remove the entrypoint");

    let output = run(document()
        .env("CLICOLOR_FORCE", "1")
        .args(["template", "check"])
        .arg(&directory));

    assert_failure(&output);
    assert!(
        stdout(&output).contains('\u{1b}'),
        "the label is not styled"
    );
}

#[cfg(unix)]
fn invalid_utf8_path() -> PathBuf {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

    PathBuf::from(OsStr::from_bytes(&[0xff, 0xfe]))
}

#[cfg(unix)]
#[test]
fn check_rejects_a_non_utf8_entrypoint() {
    let temp = temp();
    let directory = init_template(temp.path(), "check-utf8");

    let output = run(document()
        .args(["template", "check"])
        .arg(&directory)
        .arg("--entrypoint")
        .arg(invalid_utf8_path()));

    assert_failure(&output);
}

#[cfg(unix)]
#[test]
fn hash_rejects_a_non_utf8_entrypoint() {
    let temp = temp();
    let directory = init_template(temp.path(), "hash-utf8");

    let output = run(document()
        .args(["template", "hash"])
        .arg(&directory)
        .arg("--entrypoint")
        .arg(invalid_utf8_path()));

    assert_failure(&output);
}

#[cfg(unix)]
#[test]
fn publish_rejects_a_non_utf8_entrypoint() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish-utf8");

    let output = run(document()
        .args(["template", "publish", "invoice"])
        .arg(&directory)
        .arg("--entrypoint")
        .arg(invalid_utf8_path()));

    assert_failure(&output);
}

#[test]
fn check_fails_for_a_missing_directory() {
    let temp = temp();

    let output = run(document()
        .args(["template", "check"])
        .arg(temp.path().join("nope")));

    assert_failure(&output);
}

#[test]
fn publish_fails_for_a_missing_directory() {
    let temp = temp();

    let output = run(document()
        .args(["template", "publish", "invoice"])
        .arg(temp.path().join("nope")));

    assert_failure(&output);
}

#[test]
fn check_prints_warnings() {
    let temp = temp();
    let directory = init_template(temp.path(), "check-warns");

    fs::write(
        directory.join("main.typ"),
        "#set text(font: \"No Such Font\")\n= Heading",
    )
    .expect("failed to write template");

    let output = run(document().args(["template", "check"]).arg(&directory));

    assert_success(&output);
    assert!(stderr(&output).contains("warning:"));
}

/// A template can be both wrong and noisy, and the verdict has to carry both.
#[test]
fn a_broken_template_still_reports_its_warnings() {
    let temp = temp();
    let directory = init_template(temp.path(), "warns-and-fails");

    fs::write(
        directory.join("main.typ"),
        "#set text(font: \"No Such Font\")\n#definitely-does-not-exist()",
    )
    .expect("failed to write template");

    let output = run(document().args(["template", "check"]).arg(&directory));

    assert_failure(&output);
    assert!(stderr(&output).contains("warning:"));
    assert!(stderr(&output).contains("error:"));
}

#[test]
fn check_rejects_a_non_utf8_source_file() {
    let temp = temp();
    let directory = init_template(temp.path(), "check-not-utf8");

    fs::write(directory.join("broken.typ"), [0xff, 0xfe, 0xfd])
        .expect("failed to write invalid utf-8");

    let output = run(document().args(["template", "check"]).arg(&directory));

    assert_failure(&output);
}

#[test]
fn publish_fails_when_the_request_is_unreadable() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish-unreadable");
    let path = directory.join("__data/request.json");

    fs::remove_file(&path).expect("failed to remove request");
    fs::create_dir(&path).expect("failed to create a directory in its place");

    let output = run(document()
        .args(["template", "publish", "invoice"])
        .arg(&directory));

    assert_failure(&output);
}

#[test]
fn publish_handles_a_non_array_errors_field() {
    let temp = temp();
    let directory = init_template(temp.path(), "publish-odd-errors");

    let server = server("422 Unprocessable Entity", r#"{"errors":"just a string"}"#);

    let output = run(document()
        .args(["template", "publish", "invoice"])
        .arg(&directory)
        .arg("--server")
        .arg(&server));

    assert_failure(&output);
    assert!(stderr(&output).contains("server answered 422"));
}

#[test]
fn check_rejects_invalid_worker_configuration() {
    let temp = temp();
    let directory = init_template(temp.path(), "invalid-workers-check");

    let output = run(document()
        .env("DOCUMENT_WORKERS", "invalid")
        .args(["template", "check"])
        .arg(&directory));

    assert_failure(&output);
    assert!(stderr(&output).contains("invalid DOCUMENT_WORKERS"));
}

#[test]
fn init_compile_rejects_invalid_worker_configuration() {
    let temp = temp();
    let directory = temp.path().join("invalid-workers-init");

    let output = run(document()
        .env("DOCUMENT_WORKERS", "invalid")
        .args(["template", "init"])
        .arg(&directory)
        .arg("--compile"));

    assert_failure(&output);
    assert!(stderr(&output).contains("invalid DOCUMENT_WORKERS"));
}

#[test]
fn worker_rejects_an_invalid_frame() {
    let mut child = document()
        .arg("worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn worker");

    child
        .stdin
        .take()
        .expect("worker stdin")
        .write_all(&u32::MAX.to_le_bytes())
        .expect("write frame");

    let output = child.wait_with_output().expect("wait for worker");

    assert_failure(&output);
    assert!(stderr(&output).contains("framing error"));
}

#[test]
fn compile_times_out() {
    let temp = temp();
    let directory = temp.path().join("timeout");

    fs::create_dir_all(&directory).expect("create template directory");

    fs::write(
        directory.join("main.typ"),
        r"
#let sum = 0.0
#for x in range(1, 10000000) {
  sum += calc.sqrt(x)
}
#sum
",
    )
    .expect("write template");

    let output = run(document().arg("compile").arg(directory.join("main.typ")));

    assert_failure(&output);

    let err = stderr(&output);

    assert!(
        err.contains("compile exceeded its limits"),
        "expected compile limit failure, stderr:\n{err}"
    );
}

#[test]
fn worker_rejects_undecodable_job() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    const EXE: &str = env!("CARGO_BIN_EXE_document");

    let mut child = Command::new(EXE)
        .arg("worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to start worker");

    let payload = b"not json";
    let length = u32::try_from(payload.len()).expect("payload fits in frame");

    let stdin = child.stdin.as_mut().expect("worker stdin is piped");
    stdin
        .write_all(&length.to_le_bytes())
        .expect("write frame length");
    stdin.write_all(payload).expect("write frame payload");

    drop(child.stdin.take());

    let status = child.wait().expect("wait for worker");

    assert!(!status.success());
}

#[test]
fn compile_reports_an_invalid_worker_count() {
    let temp = temp();
    let directory = init_template(temp.path(), "bad-workers");

    let output = run(document()
        .env("DOCUMENT_WORKERS", "nope")
        .arg("compile")
        .arg(directory.join("main.typ")));

    assert_failure(&output);
    assert!(stderr(&output).contains("invalid DOCUMENT_WORKERS"));
}

#[test]
fn init_writes_the_invoice_template() {
    let temp = temp();
    let directory = temp.path().join("invoice");

    let output = run(document()
        .args(["template", "init"])
        .arg(&directory)
        .args(["--template", "invoice"]));

    assert_success(&output);
    assert!(directory.join("brand.typ").is_file());
    assert!(directory.join("fixture.json").is_file());

    // The invoice template is the one that has to stay valid.
    let check = run(document().args(["template", "check"]).arg(&directory));

    assert_success(&check);
    assert!(stdout(&check).contains("PASSED"));
}

#[test]
fn init_defaults_to_the_minimal_template() {
    let temp = temp();
    let directory = init_template(temp.path(), "default");

    assert!(directory.join("main.typ").is_file());
    assert!(!directory.join("fixture.json").exists());
}

#[test]
fn init_rejects_an_unknown_template() {
    let temp = temp();

    let output = run(document()
        .args(["template", "init"])
        .arg(temp.path().join("bad"))
        .args(["--template", "definitely-not-a-template"]));

    assert_failure(&output);
}
