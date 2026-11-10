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
