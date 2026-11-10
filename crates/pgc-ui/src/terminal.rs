//! Terminal setup and restore. The terminal is restored on normal exit and on panic.

use std::io::{self, Stdout, stdout};
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

pub type Tui = Terminal<CrosstermBackend<Stdout>>;

/// True while the terminal is in raw mode / alternate screen.
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// True while the terminal's enhanced keyboard mode is on (D-053).
static ENHANCED: AtomicBool = AtomicBool::new(false);

/// Switches to raw mode and the alternate screen; enables mouse, bracketed paste and
/// (where supported) the enhanced keyboard mode.
pub fn enter() -> io::Result<Tui> {
    enable_raw_mode()?;
    ACTIVE.store(true, Ordering::SeqCst);
    let mut out = stdout();
    if let Err(e) = execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    ) {
        leave();
        return Err(e);
    }
    // Unambiguous key reports (kitty keyboard protocol) where the terminal has them:
    // Ctrl+Shift+letter, Ctrl+Backspace, Ctrl+Tab, Shift+F3 then arrive as pressed
    // (D-053). Other terminals (and Windows, which reports keys itself) are unchanged.
    if supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        ENHANCED.store(true, Ordering::SeqCst);
        tracing::info!("terminal: enhanced keyboard mode on");
    }
    match Terminal::new(CrosstermBackend::new(out)) {
        Ok(t) => Ok(t),
        Err(e) => {
            leave();
            Err(e)
        }
    }
}

/// Restores the terminal. Safe to call several times; does nothing if not active.
pub fn leave() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = stdout();
    // Errors are ignored: we do our best to restore, and there is nobody to report to.
    if ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    );
    let _ = disable_raw_mode();
}

/// Installs a panic hook that restores the terminal before the panic message is
/// printed. Call this first thing in `main`.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        leave();
        tracing::error!("panic: {info}");
        previous(info);
    }));
}
