//! Terminal UI (ratatui): layout, object tree, tabs, menus, dialogs, F-key bar, colour
//! schemes, keymap (D-009, D-014, D-016).

mod app;
mod clipboard;
mod combo;
mod completion;
mod dashboard;
mod dialog;
mod export;
mod grid;
mod help;
mod hints;
mod hotkey;
mod icons;
mod keymap;
mod keys;
mod menu;
mod objects;
mod psql;
mod query_tab;
pub mod setup;
mod template_dialog;
mod terminal;
mod theme;
#[cfg(test)]
mod theme_check;
mod tree;
mod view;

use std::time::Duration;

use anyhow::Result;
use futures::StreamExt;
use pgc_core::paths::Paths;
use pgc_core::settings::Settings;
use pgc_db::EventSink;
use pgc_db::conninfo::Resolved;
use tokio::sync::mpsc;

use crate::app::{App, Effect, Msg};
use crate::view::Hits;

pub use terminal::install_panic_hook;

/// How long `Esc` waits for a digit (D-032).
const ESC_TIMEOUT: Duration = Duration::from_secs(1);

/// Status refresh / cancel-wait check interval while a query runs.
const TICK: Duration = Duration::from_millis(250);

/// Pause between background lexer slices (D-038).
const SCAN_TICK: Duration = Duration::from_millis(5);

/// How long the tree cursor must rest before the default tab loads (D-037).
const DEFINITION_DELAY: Duration = Duration::from_millis(200);

/// Runs the TUI until the user quits. The terminal is always restored.
/// `startup` is the command-line connection (D-031), saved and connected at once.
pub async fn run(paths: Paths, settings: Settings, startup: Option<Resolved>) -> Result<()> {
    let (tx, rx) = mpsc::unbounded_channel::<Msg>();
    let db_tx = tx.clone();
    let sink: EventSink = std::sync::Arc::new(move |event| {
        let _ = db_tx.send(Msg::Db(event));
    });
    let mut app = App::new(paths, settings, sink, startup);
    let export_tx = tx.clone();
    app.set_export_sink(std::sync::Arc::new(move |event| {
        let _ = export_tx.send(Msg::Export(event));
    }));
    let mut tui = terminal::enter()?;
    let result = event_loop(&mut tui, &mut app, tx, rx).await;
    terminal::leave();
    result
}

async fn event_loop(
    tui: &mut terminal::Tui,
    app: &mut App,
    tx: mpsc::UnboundedSender<Msg>,
    mut rx: mpsc::UnboundedReceiver<Msg>,
) -> Result<()> {
    // Terminal input → messages.
    let input_tx = tx.clone();
    let input = tokio::spawn(async move {
        let mut events = crossterm::event::EventStream::new();
        while let Some(event) = events.next().await {
            match event {
                Ok(e) => {
                    if input_tx.send(Msg::Input(e)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!("terminal input: {e}");
                    break;
                }
            }
        }
    });

    let mut hits = Hits::default();
    let result = loop {
        if let Err(e) = tui.draw(|frame| view::render(frame, app, &mut hits)) {
            break Err(e.into());
        }
        let Some(msg) = rx.recv().await else {
            break Ok(());
        };
        let mut effects = app.update(msg, &hits);
        // Process everything already queued before drawing again.
        while let Ok(msg) = rx.try_recv() {
            effects.extend(app.update(msg, &hits));
        }
        for effect in effects {
            match effect {
                Effect::Tick => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(TICK).await;
                        let _ = tx.send(Msg::Tick);
                    });
                }
                Effect::ScanTick => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(SCAN_TICK).await;
                        let _ = tx.send(Msg::ScanTick);
                    });
                }
                Effect::Osc52(text) => {
                    use crossterm::clipboard::CopyToClipboard;
                    if let Err(e) = crossterm::execute!(
                        std::io::stdout(),
                        CopyToClipboard::to_clipboard_from(text)
                    ) {
                        tracing::warn!("OSC 52 copy failed: {e}");
                    }
                }
                Effect::StartFindTimer(seq) => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(DEFINITION_DELAY).await;
                        let _ = tx.send(Msg::FindTimer(seq));
                    });
                }
                Effect::StartDefinitionTimer(seq) => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(DEFINITION_DELAY).await;
                        let _ = tx.send(Msg::DefinitionTimer(seq));
                    });
                }
                Effect::StartDashboardTimer { tab, seq, after } => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(after).await;
                        let _ = tx.send(Msg::DashboardTimer(tab, seq));
                    });
                }
                Effect::StartEscTimer(seq) => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(ESC_TIMEOUT).await;
                        let _ = tx.send(Msg::EscTimeout(seq));
                    });
                }
            }
        }
        if app.should_quit() {
            break Ok(());
        }
    };
    input.abort();
    result
}
