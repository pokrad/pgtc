//! pgtc — Postgres terminal commander.

mod cli;

use anyhow::{Context, Result, bail};
use clap::Parser;
use pgc_core::paths::Paths;
use pgc_core::settings::Settings;
use pgc_core::{APP_NAME, APP_VERSION, logging};

use pgc_ui::setup::{self, StepOutcome};

use crate::cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    // First thing: a panic must never leave the terminal in raw mode.
    pgc_ui::install_panic_hook();

    // Parse before anything else: `--help` / `--version` / usage errors exit here
    // without side effects.
    let cli = Cli::parse();

    let paths = Paths::from_os().context("cannot determine user directories")?;
    let _log_guard = logging::init_logging(&paths.logs_dir()).context("cannot start logging")?;
    tracing::info!("{APP_NAME} {APP_VERSION} starting");
    tracing::debug!(?paths, "user directories");

    let result = run(&cli, paths).await;
    match &result {
        Ok(()) => tracing::info!("{APP_NAME} exiting"),
        Err(e) => tracing::error!("{APP_NAME} failed: {e:#}"),
    }
    result
}

async fn run(cli: &Cli, paths: Paths) -> Result<()> {
    let settings = Settings::load(&paths.settings_file())?;
    tracing::debug!(?settings, "settings loaded");

    // D-047: installation steps (all of them with --setup, or a single one)
    if cli.setup || cli.install_templates {
        let steps = if cli.setup {
            setup::run_setup(&paths)
        } else {
            vec![setup::templates_step(&paths)]
        };
        return print_steps(&steps);
    }

    // D-031: command-line connection (options, PG* variables, URI) — saved and
    // connected at startup.
    let conn = cli.connection_args();
    let startup = if conn.is_empty() {
        None
    } else {
        let args = pgc_db::conninfo::ConnArgs {
            host: conn.host,
            port: conn.port,
            username: conn.username,
            dbname: conn.dbname,
            password_prompt: conn.password_prompt,
        };
        let resolved = pgc_db::conninfo::resolve(&args)
            .map_err(|e| anyhow::anyhow!("invalid connection arguments: {e}"))?;
        tracing::info!(name = %resolved.connection.name, "startup connection");
        Some(resolved)
    };

    pgc_ui::run(paths, settings, startup).await
}

/// Print the outcome of installation steps; an error if any of them failed (D-047).
fn print_steps(steps: &[StepOutcome]) -> Result<()> {
    for s in steps {
        println!("{}", s.text);
    }
    let failed = steps.iter().filter(|s| !s.ok).count();
    if failed > 0 {
        bail!("{failed} of {} installation steps failed", steps.len());
    }
    Ok(())
}
