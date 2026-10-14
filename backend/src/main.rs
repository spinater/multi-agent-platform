mod agents;
mod api;
mod config;
mod decide;
mod decisions;
mod detect;
mod git_access;
mod hub;
mod login;
mod model;
mod orchestrator;
mod project_git;
mod project_settings;
mod projects;
mod reflex;
mod reflex_screen;
mod repo;
mod skills;
mod store;
mod templates;
mod terminal;
mod updates;
mod usage;
mod watcher;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::hub::Hub;
use crate::store::Store;
use crate::terminal::tmux::Tmux;

#[derive(Parser)]
#[command(name = "map", about = "Multi-agent platform: run, watch and orchestrate coding-agent CLIs in tmux")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the API, dashboard, watcher and orchestrator.
    Serve {
        #[arg(short, long, default_value = "map.toml")]
        config: PathBuf,
    },
    /// Validate the config and report which provider CLIs are installed.
    Check {
        #[arg(short, long, default_value = "map.toml")]
        config: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "map=info,map_server=info,tower_http=warn".into()))
        .init();
    match Cli::parse().command {
        Command::Serve { config } => serve(Config::load(&config)?).await,
        Command::Check { config } => check(Config::load(&config)?).await,
    }
}

async fn open(config: Config) -> Result<Arc<Hub>> {
    std::fs::create_dir_all(&config.server.data_dir).with_context(|| format!("creating {}", config.server.data_dir.display()))?;
    let store = Store::open(&config.server.data_dir.join("map.db")).await?;
    Ok(Hub::new(config, store, Arc::new(Tmux::new())))
}

async fn check(config: Config) -> Result<()> {
    let hub = open(config).await?;
    println!("workspace: {}", hub.config.workspace.dir.display());
    println!("dashboard: {}", hub.config.server.static_dir.display());
    let reflex = &hub.config.reflex.binary;
    let found = if reflex.contains('/') { std::path::Path::new(reflex).exists() } else { std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).any(|d| d.join(reflex).is_file()) };
    println!("reflex:    {reflex} ({})", if found { "ok" } else { "missing: projects answer through their fallback only" });
    for p in hub.providers() {
        let mark = if p.available { "ok     " } else { "missing" };
        println!("  {mark} {:<8} {} (default {})", p.name, hub.spec(&p.name)?.binary, p.default_model);
    }
    Ok(())
}

async fn serve(config: Config) -> Result<()> {
    let addr: SocketAddr = config.server.bind.parse().context("server.bind must be host:port")?;
    if !addr.ip().is_loopback() && config.server.api_token.is_none() {
        tracing::warn!("listening on {addr} without an api_token: anyone who can reach it can run agents in your workspace");
    }
    let hub = open(config).await?;
    // MAP's gitconfig and known_hosts, before any agent starts (regenerated on every change after this).
    tracing::info!("git config for agents: {}", hub.gitconfig().display());
    tokio::spawn(watcher::run(hub.clone()));
    tokio::spawn(orchestrator::run(hub.clone()));
    tokio::spawn(usage::run(hub.clone()));
    tokio::spawn(updates::run(hub.clone()));
    for project in hub.store.projects().await? {
        let hub = hub.clone();
        tokio::spawn(async move {
            if let Err(err) = hub.restart_reflex(&project.slug).await {
                tracing::warn!("reflex for {}: {err:#}", project.slug);
            }
        });
    }

    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("binding {addr}"))?;
    tracing::info!("multi-agent platform on http://{addr}");
    axum::serve(listener, api::router(hub.clone()))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    hub.reflex.stop_all().await;
    Ok(())
}

/// Ctrl-C, or SIGTERM from `kill` or a service manager: either way the Reflex engines are stopped on the way out,
/// so none is left holding its port.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("installing the SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}
