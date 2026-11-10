//! Database structure model, `.pgdef` read/write, diff and sync script generation
//! (D-012, D-048).

pub mod file;
pub mod model;
pub mod sync;

pub use model::*;
