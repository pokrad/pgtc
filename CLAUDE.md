# CLAUDE.md — pgtc (Postgres Terminal Commander)

Guide for working on pgtc (Postgres Terminal Commander): how we work, how the code is organised, and in which
order the first alpha is implemented.

**`DESIGN.md` is the source of truth for *what* we build.** Every decision there has an ID
(`D-xxx`). This file covers *how* and *in which order*.

---

## 1. Working rules

- **The project owner leads the planning.** Do not make design decisions on your own and
  do not run ahead with questions or features. Propose, explain trade-offs, then wait.
- **Every agreed decision goes into `DESIGN.md`**: new or updated `D-xxx` entry plus a line
  in the change log. Do not record things that were not agreed.
- **Do not add features that are not in `DESIGN.md`.** If something seems missing, ask.
- If implementation shows that a decision does not work, stop and discuss it — do not
  silently deviate from `DESIGN.md`.
- Reference sources on the owner's machine (read-only, for ideas/queries):
  - pgAdmin 4: `F:\razno\pgadmin4` — SQL templates may be reused (PostgreSQL Licence, D-010).
  - Midnight Commander: `F:\razno\mc` — look & keys only, **no code** (GPL, D-016).
  - Zed: `F:\razno\zed` — reference only, **no code** (GPL, D-015).

---

## 2. Hard constraints (from DESIGN.md)

- Pure terminal app; must work over SSH; Windows + Linux required, macOS desired
  (D-002, D-003). No Unix-only APIs — go through crossterm.
- **Licence: PostgreSQL Licence (D-017).** Only permissively licensed dependencies
  (MIT, Apache-2.0, BSD, ISC, Zlib, PostgreSQL, BSL-1.0, Unicode-3.0). **Never copy or link GPL/LGPL/AGPL code.**
  Keep pgAdmin's copyright notice with reused SQL templates.
- The UI never waits for the database: all DB work runs in tokio tasks, the UI talks to
  them through channels (D-006, D-011).
- **User-supplied values go into SQL as bind parameters (`$1`, ...)**, never pasted into
  SQL text. Identifiers are quoted with the single `pgc-core` quoting implementation.
- Icons are decorative only; with icons off they become empty space (D-005).
- No hard-coded list of server versions; use `ServerVersion` + feature table (D-011).
- Keyboard first; every action reachable by keyboard; mouse optional (D-002, D-016).

---

## 3. Workspace layout (D-018)

```
pgtc/
  Cargo.toml              # [workspace], [workspace.dependencies], release profile with LTO
  DESIGN.md
  CLAUDE.md
  crates/
    pgtc/                 # binary: clap, startup, --setup, --install-templates
    pgc-ui/               # ratatui app: layout, tree, tabs, menus, dialogs, themes, keymap
    pgc-editor/           # SQL editor widget: ropey, SQL lexer
    pgc-db/               # connections, tokio-postgres, .pgpass, catalog, definitions
    pgc-sql/              # versioned template resolver, minijinja, embedded SQL, .pgtemplate
    pgc-model/            # structure model, .pgdef, diff, sync script generation
    pgc-core/             # ServerVersion, features, quoting, paths, settings, errors, logging
  sql/                    # versioned catalog SQL (embedded into pgc-sql), D-010
  templates/              # built-in developer templates (embedded), D-013
  themes/                 # built-in colour schemes (embedded), D-014
  THIRD_PARTY_LICENSES/   # pgAdmin, ...
  vendor/                 # patched third-party crates, not workspace members (D-040)
```

Dependency direction: `pgtc → pgc-ui → (pgc-editor, pgc-db, pgc-model, pgc-sql) → pgc-core`.
`pgc-db → pgc-sql, pgc-model`. Never the other way round. `pgc-core` has no heavy
dependencies.

---

## 4. Implementation guidelines

- **Application architecture (UI):** one central `App` state, an `Action`/message enum,
  an `update(state, action)` function, and a `render(state)` function (Elm-style, common
  ratatui pattern). Input events, DB responses and timers all arrive as messages on one
  channel; the UI loop is the only place that mutates state.
  `pgc-ui/src/app.rs` has the state, the message loop and the command dispatch; the
  rest of `impl App` is split by area into `app/*.rs` (tree pane, query tabs, closing,
  templates, ...). New handlers go into the module of their area; tests are in
  `app/tests.rs` (`tests/real_server.rs`, `tests/screenshots.rs`).
- **Terminal safety:** always restore the terminal (raw mode, alternate screen) on exit
  *and* on panic (install a panic hook first thing).
- **Logging:** `tracing` to a file only (D-007); never print to stdout/stderr while the
  TUI is active.
- **Errors:** `thiserror` in library crates, `anyhow` in the binary / app layer.
- **DB connections:** one tokio task per connection, owning the `tokio_postgres::Client`;
  requests in via `mpsc`, results / notices / status out via a channel to the UI.
  Keep the `CancelToken` outside the task so cancel works while a query runs.
- **Rendering:** render only what is visible (tree rows, editor lines, grid cells).
- **Embedding:** SQL, templates, themes embedded at compile time
  (`include_str!` / `include_bytes!` / `include_dir`).
- **Per-OS paths:** only through `pgc-core` (own code, D-039); never build
  paths by hand.
- **Keys:** never hard-code a key in widget code — go through the keymap with contexts
  (`[global]`, `[tree]`, `[editor]`, `[results]`, `[dialog]`), D-016.
- **Colours:** never hard-code colours — use theme roles (D-014).
- **Frames and lines:** never hard-code box-drawing characters — use the theme's
  `double_border()` / `plain_border()` / `hline()` / `vline()` / `separator_ends()`, so
  the ASCII frames setting works (D-051).
- **New commands** get a keymap name *and* a short description in the `commands!`
  table of `keymap.rs`; the description is shown in Help → Keyboard shortcuts.
- **Deterministic output** for `.pgdef` and exports: stable ordering, pretty-printed (D-012).
- Commands:
  - `cargo build` / `cargo build --release`
  - `cargo run -p pgtc`
  - `cargo test`
  - `cargo fmt` and `cargo clippy --all-targets` before considering work done
  - README screenshots (`docs/screenshots/*.svg`, Dracula scheme, drawn by pgtc's
    renderer with sample data) after visible UI changes (PowerShell):
    `$env:PGTC_SCREENSHOT_DIR="$PWD\docs\screenshots"; cargo test -p pgc-ui readme_screenshots -- --ignored`
  - After adding or upgrading a dependency, regenerate the crate licence list (it fails
    on a licence not accepted by D-017):
    `cargo about generate -c THIRD_PARTY_LICENSES/about.toml THIRD_PARTY_LICENSES/about.hbs -o THIRD_PARTY_LICENSES/rust-crates.txt`

---

## 5. Implementation order (first alpha)

Each phase ends with something that **builds and runs**. Do phases in order; inside a
phase, small steps with a working state after each.

### Phase 0 — Workspace skeleton
- Workspace with the 7 crates, `[workspace.dependencies]`, release profile (LTO).
- `pgc-core`: error types, logging setup (file), per-OS paths, settings struct +
  load/save (TOML).
- `pgtc`: `clap` with the planned arguments (stubs: psql-style connection options D-031,
  `--setup`, `--install-templates`), starts the app.
- `THIRD_PARTY_LICENSES/` folder.
- **Result:** `cargo run` starts and exits cleanly, writes a log file.

### Phase 1 — TUI shell (mc look)
- ratatui + crossterm init/restore, panic hook, async event loop (tokio + channels).
- Layout: menu bar, left tree pane, right tab area, status line, key-hint bar (D-009, D-016).
- Theme system + **Classic** scheme (+ Dark, Light, Terminal default); user themes
  scanned from user dir (D-014).
- Keymap with contexts + `keymap.toml` overrides; `Esc`+digit F-key substitute.
- Menu bar (`F9`) with all menus from D-014 (unimplemented items disabled).
- mc-style dialog framework (buttons, inputs, checkboxes, lists) — needed by everything
  after this.
- Settings dialog skeleton (icons on/off, colour scheme); icons mode (D-005).
- Tabs: default tab, open/close/next/previous.
- **Result:** navigable empty application with working menu, dialogs, themes.

### Phase 2 — Connections
- `pgc-core`: `ServerVersion`, feature table, identifier/literal quoting.
- `pgc-db`: connection task, rustls TLS, `.pgpass` read + write (D-008),
  `server_version_num`, "not yet verified" warning (D-011).
- Saved connections in config; connection dialog with **Save password** checkbox.
- Multiple servers; per-database connections on demand.
- **Result:** connect / disconnect servers from the tree.

### Phase 3 — Versioned SQL and object tree
- `pgc-sql`: version resolver, minijinja environment, pgAdmin filters (`qtIdent`,
  `qtLiteral`, `qtTypeIdent`, `hasAny`, `_()`), embedded SQL.
- Import/adapt pgAdmin read-only queries (`nodes.sql`, `properties.sql`, ...) object type
  by object type: databases, schemas, tables, columns, indexes, constraints, views,
  functions, triggers, sequences, types, roles, ...
- Lazy-loading object tree; refresh (`Ctrl+R`), show/hide system objects (`Alt+.`),
  quick search.
- **Result:** browse the full object tree of a server.

### Phase 4 — Object view (definitions)
- Definitions via `pg_get_*` functions; assembled DDL for tables, sequences, types,
  roles, databases, schemas (D-010).
- Default tab follows the tree selection; context menu (`F2` / right click) with
  "New query"; "Show in new tab" on `F3` only (menus by row kind, D-049).
- Read-only SQL view with highlighting (uses the lexer from Phase 5 — start with plain
  text if Phase 5 is not done yet).
- **Result:** select any object and see its SQL definition.

### Phase 5 — SQL editor widget (`pgc-editor`)
- `ropey` buffer, viewport rendering of visible lines only, cursor, selection.
- Standard keys + `Ctrl+Insert` / `Shift+Insert` / `Shift+Delete` (D-015).
- Undo/redo, find/replace, tab indent.
- Clipboard: internal buffer + `arboard` + OSC 52 + bracketed paste (D-023).
- SQL lexer (keywords, identifiers, strings, numbers, comments, dollar quoting) with
  checkpoint state cache and background scan.
- Check speed with a very large SQL file (e.g. 100 MB) before moving on.
- **Result:** fast editor usable in query tabs.

### Phase 6 — Query execution
- Query tab layout: editor + results pane, resizable split, `Ctrl+R` show/hide (D-019).
- `F5`: selection or whole script, sent as one request.
- Streaming results, 1,000-row batches, auto-fetch on scroll, "Fetch all", status line.
- Result grid widget (visible cells only); numbered result sub-tabs, first always shown.
- Messages tab: notices, command tags, errors with DETAIL/HINT and line jump, total time;
  auto-switch to Messages on error.
- Cancel `Alt+Shift+C` (CancelToken; offer closing the connection if no reaction).
- Transactions as in pgAdmin: auto commit / auto rollback toggles, Commit / Rollback,
  transaction status in status line + tab label, confirm on close.
- PL/pgSQL mode (`DO` block wrapping), mode shown in tab + status line (D-020).
- File → Open / Save SQL file.
- **Result:** full daily-use query tool.

### Phase 7 — Result export
- CSV (separator, BOM), JSON, XML (base64 for forbidden control chars); full result via
  the paused stream, streamed to file (D-021). Options remembered in settings.
- **Result:** every result set exportable.

### Phase 8 — Developer templates
- `.pgtemplate` parser (`+++` header in TOML, Jinja body), parameter types incl. `list`,
  `choice` with `source` / `query` (D-013).
- Dialog generated from the parameters (all shown; `list` rows with Add / Edit /
  Delete / Up / Down; `sql` editor box); live read-only preview; context variables
  (version / features, database); `ident` filter (D-046).
- Picker: folder tree with search box (D-046). Only from a connected query tab.
- Choice lists through the query tab's connection; defaults from the tree selection of
  the same server and database (D-046).
- Scanning the user's templates folder (the only source; embedded templates are only
  installed there), `--install-templates`, Settings → Install templates / Show
  templates folder.
- A first set of built-in templates (function, table, index, view, trigger, ...).
- Result inserted at the cursor of the query tab's editor (not executed, D-046).
- **Result:** Tools → Generate SQL from developer template works end to end.

### Phase 9 — Structure model, definition export and load (sync)
- `pgc-model` structs (structured properties, not only DDL text), D-048 object types.
- Read the model from a live database (`pgc-db`, versioned SQL under `sql/model/`).
- `.pgdef` save/load — deterministic JSON (D-012); File → Export database definition.
- Diff of file model ↔ live model; sync script with only the needed statements, no drops
  of objects only in the database, dependency-ordered (topological sort) (D-048).
- File → Load database definition: script opens in a query tab connected to the
  selected database for review.
- **Result:** export a database structure and bring another database to it.

### Phase 10 — Help and finishing
- *(Done early, D-047: `--setup`, Settings → Run setup; fonts are not bundled, D-005.)*
- *(Done 2026-11-10:)* Help: keyboard shortcuts (from the active keymap), third-party
  licences (`cargo-about`), About.
- *(Done 2026-11-10:)* Settings dialog complete (D-051).
- *(Done 2026-11-10:)* Review all of `DESIGN.md` against the implementation; gaps fixed
  (D-051). CI parts are deferred until the code is published (D-025).
- **Result:** first alpha.

### Phase 11 — Autocomplete (D-052)
- `pgc-editor`: completion context from the lexer (statement around the cursor,
  prefix, `FROM` / `JOIN` tables and aliases, `name.`) — pure logic with unit tests.
- `pgc-db`: lookups with bind parameters; `sql/completion/` catalog queries (objects,
  columns).
- `pgc-ui`: popup at the cursor, keys, triggers (`Ctrl+Space` / `Alt+/`, after `.` and
  3 letters), per-tab name cache, dropped after DDL and on tree refresh.
- **Result:** basic autocomplete in query tabs.

### Phase 12 — psql-style commands (D-054)
- `pgc-db`: catalog runs — several parameterised queries as one run, emitting the
  usual query events (titled result sets, empty sections skipped).
- `pgc-ui`: `F4` command line (matching commands, session history), parser (commands,
  `S` / `+`, psql patterns → bind parameters), list / describe queries, `conninfo`,
  `?` (psql's commands without the backslash); generate commands `gts` / `gti` / `gtu` / `gtd` (pure generators with unit
  tests) inserted at the cursor.
- **Result:** psql's everyday commands inside query tabs.

Testing tools are deferred (D-024); still write ordinary `cargo test` unit tests for
pure logic (quoting, `.pgpass`, resolver, lexer, diff) as the code is written.
