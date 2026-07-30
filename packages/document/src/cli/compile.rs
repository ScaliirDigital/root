use std::{fs, path::PathBuf, process::ExitCode};

use crate::{
    cli::styles,
    core::{CompileError, Files, Job, Limits, Pdf, REQUEST, RenderOptions, compile},
};

pub fn run(input: PathBuf, output: Option<PathBuf>) -> ExitCode {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    runtime.block_on(run_async(input, output))
}

async fn run_async(input: PathBuf, output: Option<PathBuf>) -> ExitCode {
    // The bundle is the entrypoint's directory: the same set the server would
    // receive, so a local compile cannot succeed on files a publish would miss.
    let Some(root) = input.parent() else {
        eprintln!("error: input has no parent directory");
        return ExitCode::FAILURE;
    };

    let Some(entrypoint) = input.file_name().and_then(|name| name.to_str()) else {
        eprintln!("error: input filename is not valid utf-8");
        return ExitCode::FAILURE;
    };

    let files = match Files::read_dir(root, entrypoint) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = files.validate() {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }

    // Optional here, unlike `template check`: compiling is for looking at the
    // result, and a template that needs no data should not need a request.
    //
    // Only the `data` half is passed on -- the rest of the request is context
    // the engine derives itself.
    let path = root.join(REQUEST);
    let data = if path.exists() {
        match fs::read(&path).map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes)) {
            Ok(Ok(request)) => request.get("data").cloned(),
            Ok(Err(error)) => {
                eprintln!("error: {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
            Err(error) => {
                eprintln!("error: {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };

    let job = Job {
        files,
        data,
        xml: None,
        options: RenderOptions {
            timestamp: None,
            standard: Pdf::Plain,
        },
    };

    let pdf = match compile(&job, Limits::default()).await {
        Ok(document) => {
            for warning in &document.warnings {
                eprintln!("warning: {warning}");
            }

            document.pdf
        }
        Err(CompileError::Template { errors, .. }) => {
            for error in &errors {
                eprintln!("error: {error}");
            }

            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    // Never beside the entrypoint: the bundle is read from that directory, so
    // an earlier compile would end up published as part of the template.
    let output = output.unwrap_or_else(|| {
        PathBuf::from(input.file_stem().unwrap_or_default()).with_extension("pdf")
    });

    if let Err(error) = fs::write(&output, pdf) {
        eprintln!("error: {}: {error}", output.display());
        return ExitCode::FAILURE;
    }

    println!("{} {}", styles::ok("COMPILED"), output.display());
    ExitCode::SUCCESS
}
