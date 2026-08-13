mod cli;
mod core;
mod server;
mod storage;

use std::process::ExitCode;

/// musl's allocator roughly halves throughput under this workload — Typst
/// allocates heavily during a compile. mimalloc closes most of that gap.
/// Harmless on glibc, so it applies to every target.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    cli::run()
}
