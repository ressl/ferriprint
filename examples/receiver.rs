// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
//! A local, deterministic IPP test device. Never connects to a physical printer.
//! Submitted jobs remain processing until Ctrl-C; tests control explicit completion.
#[path = "../tests/support/mod.rs"]
mod support;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let receiver = support::Receiver::bind("127.0.0.1:18631".parse()?).await;
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
