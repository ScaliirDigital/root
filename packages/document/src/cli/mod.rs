mod compile;
mod styles;
mod template;

use crate::{core, server};
use clap::{Parser, Subcommand};
use std::{path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(
    name = "document",
    version,
    about = "Deterministic PDFs from Typst templates | check, publish, compile"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Render a Typst template into PDF.
    #[command(after_help = "\
Examples:
  document compile example/main.typ
  document compile example/main.typ --output example.pdf
")]
    Compile {
        /// Typst entrypoint.
        input: PathBuf,

        /// Output PDF path. Defaults to the input with a .pdf extension.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Validate and publish document templates.
    Template {
        #[command(subcommand)]
        command: template::Command,
    },

    /// Serve document rendering over HTTP.
    #[command(after_help = "\
Examples:
  document serve
  document serve --listen 127.0.0.1:8080
")]
    Serve {
        /// Address the server listens on.
        #[arg(long, default_value = "0.0.0.0:8080")]
        listen: String,
    },

    /// Internal isolated rendering worker.
    #[command(hide = true)]
    Worker,
    // TODO: Convert supported structured document formats into compliant PDFs.
    //
    // Examples:
    //   document convert invoice.ledes
    //   document convert invoice.ledes --output invoice.pdf
}

pub fn run() -> ExitCode {
    match Cli::parse().command {
        Command::Compile { input, output } => with_pool(|| compile::run(input, output)),
        Command::Template { command } => with_pool(|| template::run(command)),

        // `serve` owns the pool end to end, including shutdown.
        Command::Serve { listen } => server::start(&listen),

        Command::Worker => {
            let mut stdin = std::io::stdin().lock();
            let mut stdout = std::io::stdout().lock();

            core::run_worker(&mut stdin, &mut stdout)
        }
    }
}

/// Configures the worker pool before running a command that renders.
fn with_pool(run: impl FnOnce() -> ExitCode) -> ExitCode {
    match core::initialize() {
        Ok(()) => run(),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
