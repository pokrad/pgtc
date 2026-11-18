//! Terminal UI (ratatui): layout, object tree, tabs, menus, dialogs, colour
//! schemes, keymap (D-009, D-014, D-016).

mod ai;
mod app;
mod clipboard;
mod combo;
mod completion;
mod dashboard;
mod db_stats;
mod dialog;
mod export;
mod grid;
mod help;
mod hotkey;
mod keymap;
mod keys;
mod menu;
mod message_pos;
mod monitor;
mod objects;
mod profiler;
mod profiler_restore;
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
    let file_tx = tx.clone();
    app.set_file_sink(std::sync::Arc::new(move |event| {
        let _ = file_tx.send(Msg::File(event));
    }));
    let ai_tx = tx.clone();
    app.set_ai_sink(std::sync::Arc::new(move |event| {
        let _ = ai_tx.send(match event {
            app::AiMsg::Answer(e) => Msg::Ai(e),
            app::AiMsg::Models(m) => Msg::AiModels(m),
        });
    }));
    let mut tui = terminal::enter()?;
    let result = event_loop(&mut tui, &mut app, tx, rx).await;
    terminal::leave();
    // D-071: the sessions end properly; a moment for that, never long
    let handles = app.end_connections();
    let deadline = std::time::Instant::now() + QUIT_WAIT;
    while handles.iter().any(|h| !h.is_finished()) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    result
}

/// How long quitting waits for the open connections to end (D-071).
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(1);

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
                Effect::Timer(timer) => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(timer.delay()).await;
                        let _ = tx.send(timer.msg());
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
            }
        }
        if app.should_quit() {
            break Ok(());
        }
    };
    input.abort();
    result
}
