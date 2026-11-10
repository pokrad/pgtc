//! SQL editor widget (D-015, D-038): `ropey` buffer, own PostgreSQL lexer with a
//! checkpoint cache, standard editing keys (mapped by the application keymap),
//! undo/redo, find/replace. No terminal I/O here: the application handles the
//! clipboard and keys.

pub mod complete;
pub mod editor;
pub mod lexer;
pub mod render;

pub use editor::{Action, Editor, Found, Motion, SearchOptions};
pub use render::{EditorStyles, gutter_width, render};
