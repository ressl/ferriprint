// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ferriprint::{
    config::{Config, ProfileSettings},
    discovery::Discovery,
    profile::PrinterProfile,
    server::App,
};
use std::{path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(
    version,
    about = "Driverless IPP frontend for explicitly mapped native CUPS queues"
)]
struct Cli {
    #[arg(short, long, default_value = "ferriprint.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    /// Validate bootstrap and authoritative profiles offline; runtime credentials/TLS are opened on serve.
    Check,
    /// Run the IPP service (also the default command).
    Serve,
    /// Inspect or install an explicitly confirmed native CUPS profile, never a queue.
    Setup {
        #[command(subcommand)]
        command: ferriprint::setup::SetupCommand,
    },
    /// Provision local frontend credentials without plaintext arguments.
    User {
        #[command(subcommand)]
        command: UserCommand,
    },
    /// Offline: discard an unknown-outcome job. It MAY STILL PRINT upstream.
    Abandon {
        job_id: i32,
        #[arg(long, required = true)]
        acknowledge_possible_upstream_job: bool,
    },
}
#[derive(Subcommand)]
enum UserCommand {
    /// Read a password from hidden terminal input or bounded stdin; refuse overwrite.
    PasswordHash {
        #[arg(long)]
        output: PathBuf,
    },
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ferriprint=info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Setup { command }) => return ferriprint::setup::run(command).await,
        Some(Command::User {
            command: UserCommand::PasswordHash { output },
        }) => return ferriprint::auth::write_password_hash(&output),
        _ => {}
    }
    let config = Config::load(&cli.config)?;
    if matches!(cli.command, Some(Command::Check)) {
        for printer in &config.printers {
            let settings = ProfileSettings::load(&config.profile_path(&printer.name))?;
            let profile = PrinterProfile::load(printer, &settings)?;
            println!(
                "{}: {} numeric media variants; {} explicitly mapped materials; authoritative schema v{}; upstream formats are probed only on serve/setup",
                printer.name,
                profile.capabilities().media.len(),
                profile.capabilities().media_types.len(),
                settings.schema_version
            );
        }
        return Ok(());
    }
    if let Some(Command::Abandon {
        job_id,
        acknowledge_possible_upstream_job: true,
    }) = cli.command
    {
        let engine = ferriprint::jobs::JobEngine::open(&config).await?;
        let job = engine.abandon_uncertain(job_id).await?;
        println!(
            "Job {} abandoned locally. {} An unknown native job must be canceled directly in CUPS if present.",
            job.id, job.message
        );
        return Ok(());
    }
    let app = App::open(config).await?;
    let tls = if let Some(tls) = &app.config.tls {
        Some(
            axum_server::tls_rustls::RustlsConfig::from_pem_file(
                &tls.certificate,
                &tls.private_key,
            )
            .await
            .context("load TLS certificate/key")?,
        )
    } else {
        None
    };
    let router = app
        .router()
        .into_make_service_with_connect_info::<std::net::SocketAddr>();
    let handle = axum_server::Handle::new();
    let server_handle = handle.clone();
    let address = app.config.listen;
    let mut server = tokio::spawn(async move {
        if let Some(tls) = tls {
            axum_server::bind_rustls(address, tls)
                .handle(server_handle)
                .serve(router)
                .await
        } else {
            axum_server::bind(address)
                .handle(server_handle)
                .serve(router)
                .await
        }
    });
    if handle.listening().await.is_none() {
        server.await.context("HTTP server task failed")??;
        bail!("HTTP listener failed before readiness");
    }
    // No Bonjour record exists before the actual TLS listener is ready. A failed
    // publisher startup shuts down this listener without starting the scheduler.
    let mut discovery = match Discovery::start(&app).await {
        Ok(value) => value,
        Err(error) => {
            handle.shutdown();
            let _ = server.await;
            return Err(error);
        }
    };
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let scheduler = tokio::spawn(app.engine.clone().run(shutdown_rx));
    tracing::info!(listen=%app.config.listen, "Ferriprint is accepting IPP connections");
    let result: Result<()> = tokio::select! {
        result = &mut server => result.context("HTTP server task failed").and_then(|result| result.context("HTTP server failed")),
        signal = shutdown_signal() => {
            if let Some(publisher) = discovery.take() { publisher.stop().await; }
            handle.graceful_shutdown(Some(Duration::from_secs(10)));
            let stopped = server.await.context("HTTP server task failed").and_then(|result| result.context("HTTP server failed"));
            signal.and(stopped)
        },
        failure = async {
            if let Some(publisher) = discovery.as_mut() { publisher.failed().await }
            else { std::future::pending::<Result<()>>().await }
        } => {
            handle.shutdown();
            let _ = server.await;
            failure.and_then(|()| Err(anyhow::anyhow!("native publisher stopped unexpectedly")))
        }
    };
    if let Some(publisher) = discovery {
        publisher.stop().await;
    }
    let _ = shutdown_tx.send(true);
    scheduler.await.context("job scheduler panicked")??;
    result
}
async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("install SIGTERM handler")?;
        tokio::select! { result=tokio::signal::ctrl_c()=>result?, _=terminate.recv()=>{} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    Ok(())
}
