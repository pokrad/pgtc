# Vendored crates

## tokio-postgres 0.7.18 (MIT OR Apache-2.0)

Copy of <https://crates.io/crates/tokio-postgres> 0.7.18 with a small change for pgtc
(DESIGN.md D-040), used through `[patch.crates-io]` in the workspace `Cargo.toml`.
Licence files: `tokio-postgres/LICENSE-MIT`, `tokio-postgres/LICENSE-APACHE`.

Changes (search for `pgtc patch`):

- `src/codec.rs`, `src/connect_raw.rs`, `src/client.rs`, `src/lib.rs`: the codec stores
  the status byte of every ReadyForQuery; `Client::transaction_status()` returns
  `TransactionStatus::{Idle, InTransaction, Failed}`.
- `src/simple_query.rs`: `SimpleQueryStream::command_tag()` (tag of the last
  CommandComplete, e.g. `INSERT 0 5`) and `SimpleColumn::type_oid()`.
- `src/client.rs`: the `PathBuf` import is `#[cfg(unix)]` (unused-import warning on
  Windows).

## crossterm 0.29.0 (MIT)

Copy of <https://crates.io/crates/crossterm> 0.29.0 with a small change for pgtc
(DESIGN.md D-061), used through `[patch.crates-io]` in the workspace `Cargo.toml`
(ratatui's crossterm backend gets the same copy). Licence file: `crossterm/LICENSE`.

Changes (search for `pgtc patch`):

- `src/event/sys/unix/parse.rs`: `ESC` followed by an escape sequence (`ESC [` or
  `ESC O`) is that key with Alt added — PuTTY sends Alt+Up as `ESC ESC [ A`. Upstream
  reported a lone Esc and left `[` and `A` as typed characters. `ESC ESC` alone stays
  one Esc. Unit test `test_esc_prefixed_sequence_is_alt` (Unix only).
- `src/terminal/sys/unix.rs`: needless parentheses removed (`unused_parens` warning,
  shown because a path dependency is not lint-capped).
