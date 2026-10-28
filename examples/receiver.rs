// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
//! A local, deterministic IPP test device. Never connects to a physical printer.
//! Submitted jobs remain processing until Ctrl-C; tests control explicit completion.
#[path = "../tests/support/mod.rs"]
mod support;
use clap::Parser;

#[derive(Parser)]
struct Arguments {
    /// Isolated loopback only; never a real printer endpoint.
    #[arg(long, default_value = "127.0.0.1:18631")]
    listen: std::net::SocketAddr,
    /// QA only: report media-empty-error without any physical printer.
    #[arg(long)]
    paper_empty: bool,
    /// QA only: explicitly reject documents without creating an upstream job.
    #[arg(long)]
    reject_jobs: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let receiver = support::Receiver::bind(arguments.listen).await;
    receiver
        .device
        .paper_empty
        .store(arguments.paper_empty, std::sync::atomic::Ordering::SeqCst);
    if arguments.reject_jobs {
        receiver
            .device
            .mode
            .store(1, std::sync::atomic::Ordering::SeqCst);
    }
    println!("NONPRINTING test receiver ready at {}", receiver.uri());
    tokio::signal::ctrl_c().await?;
    for job in receiver.device.jobs.lock().iter() {
        println!(
            "job={} format={} bytes={} native={:?}",
            job.id, job.format, job.bytes, job.options
        );
    }
    Ok(())
}
