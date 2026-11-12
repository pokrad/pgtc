# pgtc — Design Decisions

Living document. Every decision gets an ID (D-xxx), a short rationale and a status.
Status values: **Accepted** · **Proposed** · **Open** · **Deferred**.

Short name / executable: **`pgtc`**. Long name: **Postgres terminal commander** (D-030).

---

## 1. Vision

pgtc is a PostgreSQL administration tool written in Rust that runs entirely
inside a terminal. It must be usable locally and over SSH, with no GUI, browser or
desktop dependency.

---

## 2. Decisions

### D-001 — Implementation language: Rust
- **Status:** Accepted
- **Rationale:** Single static-ish binary, no runtime to install, memory safety,
  good cross-platform story, mature PostgreSQL and terminal UI libraries.

### D-002 — Pure terminal application (TUI)
- **Status:** Accepted
- **Details:** Full-screen text UI that runs in any terminal emulator. No X11/Wayland,
  no web server, no embedded browser. It must work identically over an SSH session.
- **Consequences:**
  - Keyboard-first interaction; mouse support is optional/nice-to-have.
  - Must degrade gracefully on limited terminals (16 colors: *Terminal default* scheme,
    D-014; no Unicode box drawing: *ASCII frames* setting, D-051).
  - No reliance on OS clipboard being available (over SSH it often isn't) —
    clipboard support should be best-effort (e.g. OSC 52).

### D-003 — Target platforms
- **Status:** Accepted
- **Required:** Windows (Windows Terminal, conhost/PowerShell), Linux.
- **Desired:** macOS (Terminal.app, iTerm2).
- **Consequences:** Every dependency must be cross-platform; no Unix-only APIs
  (termios, signals) used directly — go through the terminal library abstraction.
  CI should build and test on all three OSes (*deferred* until the CI system is
  chosen, D-025, D-051).

### D-004 — Terminal UI library: ratatui + crossterm backend
- **Status:** Accepted
- **Choice:** `ratatui` (0.30.x) with the `crossterm` backend (`ratatui-crossterm`).
- **Rationale:**
  - ratatui is the most mature and actively maintained Rust TUI library
    (community successor of `tui-rs`), with a large ecosystem of widgets.
  - crossterm works natively on Windows (console API), Linux and macOS, and over SSH.
    It is ratatui's default backend.
- **Rejected alternatives:**
  - `termion` — Unix only, no Windows support.
  - `termwiz` — cross-platform but smaller community and fewer widgets.
  - `cursive` — higher-level retained-mode model, less control over complex layouts
    such as large result grids.
- **Notes:** ratatui has no built-in multi-line text editor widget — see D-015.

### D-005 — Icons and fonts: Nerd Fonts, two display modes
- **Status:** Accepted
- **Font:** The UI uses icon glyphs from **Nerd Fonts** (https://www.nerdfonts.com).
  Any Nerd Font works (icons share the same code points).
- **Background:**
  - A terminal program cannot select, load or detect fonts — the font is chosen by the
    user's terminal emulator, and there is no portable way to query it.
  - Over SSH the font must be installed on the **client machine** (where the terminal
    runs), not on the server running pgtc.
  - Most modern terminals support **font fallback** (a glyph missing from the main font
    is taken from another installed font): Linux terminals and macOS do it
    automatically; Windows Terminal accepts a font list
    (e.g. `"Cascadia Mono, Symbols Nerd Font Mono"`); WezTerm and Ghostty ship Nerd Font
    symbols built in. The legacy Windows console (conhost) has no practical fallback.
- **Display modes (only two):**
  1. **Icons off** — **default**. Each icon is replaced by empty space of the same width,
     so layout and alignment are unchanged.
  2. **Icons on** — Nerd Font glyphs are shown.
- **Switching modes:** no auto-detection. The user turns icons on/off in the
  **Settings dialog**; the change applies immediately at runtime and is saved to the
  configuration. The checkbox reads **"Show icons (needs a Nerd Font)"** and a **sample
  row of icons** is drawn next to it: if the terminal shows icons, the font is fine; if
  it shows boxes or question marks, there is no Nerd Font and icons should stay off.
- **Rule:** Icons are decorative only. No information may be conveyed by an icon alone;
  object types, states, etc. must also be clear from text, labels or grouping, so the
  UI stays fully usable with icons off.
- **No bundled fonts, no font installation** (D-047): pgtc does not ship, install or
  configure fonts. As with other terminal tools (eza, lsd, yazi, lazygit), users who
  want icons install a Nerd Font themselves and select it in their terminal.

### D-006 — PostgreSQL client: tokio-postgres (async, tokio runtime)
- **Status:** Accepted
- **Choice:** `tokio-postgres` (rust-postgres project) on the `tokio` async runtime.
- **Rationale:**
  - Pure Rust implementation of the PostgreSQL wire protocol — no libpq / C library,
    keeps the single cross-platform binary goal.
  - Closest-to-the-protocol client that is still practical:
    - **Simple query protocol** (`simple_query`) — runs arbitrary user SQL, including
      multiple statements, DDL, `DO` blocks, `VACUUM`; values returned as text.
      Used by the SQL editor.
    - **Extended protocol** (prepared statements, typed parameters) — used for
      pgtc's own catalog/metadata queries.
    - **Query cancellation** via `CancelToken` (real PostgreSQL cancel request).
    - **COPY IN/OUT** streaming — import/export. *Later* (not in the first alpha,
      D-051): `COPY … FROM STDIN / TO STDOUT` is not supported in query tabs.
    - **LISTEN/NOTIFY** and **server notices** (`RAISE NOTICE`, warnings,
      `VACUUM VERBOSE` output) — shown in the Messages tab (D-019, D-051).
    - Column metadata (names, type OIDs) for the result grid.
  - Async: database work never blocks the UI.
  - Mature, widely used; foundation for most of the Rust PostgreSQL ecosystem.
- **Lower-level fallback:** `postgres-protocol` (message encoding/decoding used by
  tokio-postgres) is available if a feature is needed that tokio-postgres does not expose.
- **Known limitation:** no replication protocol support (logical replication streams,
  base backup). Replication *monitoring* (`pg_stat_replication`, replication slots)
  is plain SQL and unaffected.
- **Rejected alternatives:**
  - `sqlx` — application-oriented (compile-time checked queries), no query
    cancellation, awkward for ad-hoc SQL with unknown result shape.
  - `diesel`, `sea-orm` — ORMs, not suited for an admin tool.
  - libpq bindings (`pq-sys`) — require the C library on every platform.
- **TLS:** `rustls` via `tokio-postgres-rustls` — see D-007.

### D-007 — Core supporting crates
- **Status:** Accepted
- These crates form the project skeleton and are used from the first line of code.

| Need | Crate(s) | Purpose |
|------|----------|---------|
| TLS | `rustls` (via `tokio-postgres-rustls`) | Encrypted connections to PostgreSQL; pure Rust, no OpenSSL/OS TLS dependency. |
| Command-line arguments | `clap` | Argument parsing (e.g. `--setup`, connection arguments). |
| Config files | `serde` + `toml` | Settings (icon mode, etc.) and saved connections. |
| Config/data folder paths | own code in `pgc-core` (D-039) | Per-OS locations: Windows `%APPDATA%`, Linux `~/.config` (XDG), macOS `~/Library/Application Support`. |
| Error handling | `thiserror` + `anyhow` | `thiserror` for typed errors in library modules, `anyhow` for propagation in the application layer. |
| Logging | `tracing` + `tracing-appender` | Structured logging to a file (a TUI cannot log to the screen). |
| Password storage | none — own `.pgpass` parser | See D-008. |
| Clipboard | `arboard` + crossterm OSC 52 (`CopyToClipboard`) | See D-023. |

- Feature-specific crates are decided together with the feature (e.g. editor D-015,
  export D-021). Testing tools: deferred (D-024).

### D-008 — Password storage: .pgpass only
- **Status:** Accepted
- **Choice:** Passwords are stored only in PostgreSQL's standard password file
  (`.pgpass`). No OS credential store (`keyring` rejected).
- **Rationale:**
  - Simple; works everywhere, including headless Linux servers and SSH sessions,
    where an OS keyring is often unavailable or locked.
  - Standard PostgreSQL mechanism, shared with psql, pg_dump and other clients —
    existing user setups work immediately.
- **Details:**
  - Location: `PGPASSFILE` environment variable if set; otherwise `~/.pgpass`
    (Linux/macOS) or `%APPDATA%\postgresql\pgpass.conf` (Windows).
  - Format: `hostname:port:database:username:password`, one entry per line, `*` as
    wildcard, `\:` and `\\` escapes, lines starting with `#` are comments; first
    matching line wins (same rules as libpq).
  - On Linux/macOS the file is ignored if its permissions are wider than `0600`
    (same as libpq).
  - tokio-postgres does not read `.pgpass`; pgtc implements a small parser.
  - If no matching entry exists, the user is prompted for the password at connect time.
- **Known trade-off:** `.pgpass` is plain text, protected only by file permissions.
- **Saving passwords:** the user decides, per connection, via a **"Save password"
  checkbox** in the login/connection dialog (unchecked by default).
  - When checked and the connection succeeds, pgtc writes the entry to `.pgpass`.
  - If an entry with the same host, port, database and username already exists, it is
    replaced; otherwise a new line is appended. Other lines and comments are preserved.
  - If the file does not exist it is created; on Linux/macOS with `0600` permissions.

### D-009 — Main window layout
- **Status:** Accepted
- **Structure:** menu bar on top; below it a window split into a left and a right pane;
  key-hint bar at the bottom (D-016).

```
+----------------------------------------------------------------+
| Menu bar                                                       |
+---------------------------+------------------------------------+
| Connections / Object tree | [Default] [customers] [Query 1]    |
|                           +------------------------------------+
| v Server A (connected)    | -- SQL definition of the object    |
|   v Databases             |    selected in the tree            |
|     v mydb                |                                    |
|       v Schemas           | CREATE TABLE public.customers (    |
|         v public          |     id integer NOT NULL,           |
|           v Tables        |     ...                            |
|             > customers   | );                                 |
| > Server B                |                                    |
+---------------------------+------------------------------------+
| F1 Help F2 Menu F3 New tab ... F9 Menu bar F10 Quit (key hints) |
+----------------------------------------------------------------+
```

- **Left pane — Connections and object tree:**
  - Tree view. Expandable nodes are marked `►` (collapsed) and `▼` (expanded).
  - Top level lists the configured server connections.
  - Each connected server expands into its tree of SQL objects (databases, schemas,
    tables, columns, indexes, functions, triggers, ...).
  - Multiple servers can be connected at the same time.
- **Right pane — Tabbed object view:**
  - The right pane holds **multiple tabs**.
  - **Default tab:** exactly one. Selecting an object in the tree always shows it in the
    default tab (replacing what was shown there).
  - **"Show in new tab"** (`F3`) — opens the object selected in the tree in a new tab
    (key only, not in the context menus, D-049).
  - **"New query"** — context menu option that opens a new query tab (on a database or
    an object below it, D-049).
  - An object tab shows the object selected in the tree (database, table, column,
    index, function, trigger, ...).
  - **By default shows the SQL definition** of the object.
  - Viewing the definition is the primary function for the first version.
  - **Later:** modifying the object from this view.
  - Note (D-002, keyboard-first): the context menu must also be reachable from the
    keyboard, not only by right click — `F2` (D-016).

### D-010 — PostgreSQL version support: versioned SQL templates (reused from pgAdmin)
- **Status:** Accepted
- **Problem:** catalog queries differ between PostgreSQL versions; Rust has no class
  inheritance to "override" only the queries that changed.
- **Approach (same model as pgAdmin 4):**
  - Every catalog / definition query is a separate SQL template file, organised per
    object type and per version folder:
    ```
    sql/<object>/default/   all queries (baseline)
    sql/<object>/16_plus/   only queries that differ from 16 onwards
    sql/<object>/17_plus/   ...
    ```
  - **Version resolver:** for (object, query name, server version) walk the version
    folders from newest to oldest, skip folders newer than the server, return the first
    folder that contains the file; fall back to `default`. Only changed files are
    duplicated — "inheritance per file".
  - Server version is taken from `server_version_num` after connecting.
  - **Templates:** `minijinja` (Rust implementation of Jinja2), so small in-file
    differences use conditions (`{% if server_version >= 160000 %}`) and shared
    fragments use macros — same syntax as pgAdmin.
  - Template files are **embedded in the binary** at compile time (`include_str!` /
    `include_dir`) — no loose files to distribute.
  - Values that come from the user are passed as bind parameters (`$1`, ...), never
    pasted into the SQL text by the template.
- **Reuse of pgAdmin SQL:**
  - pgAdmin 4's SQL templates (~2000 files under `web/pgadmin/browser/**/templates`)
    are reused and adapted. Reference copy: `F:\razno\pgadmin4`.
  - License: pgAdmin is under the PostgreSQL Licence (permissive). The pgAdmin
    copyright notice and license text must be kept with the reused files
    (e.g. `THIRD_PARTY_LICENSES` / header in the template folder).
  - pgAdmin-specific template filters must be implemented in minijinja:
    `qtIdent`, `qtLiteral`, `qtTypeIdent`, `hasAny`; and `_()` (translation —
    returns the text unchanged).
  - **Scope of reuse:** read-only catalog queries (`properties.sql`, `nodes.sql`,
    `stats.sql`, ...). pgAdmin's DDL-generating templates (`create.sql`, `update.sql`)
    are **not** reused for object definitions — they depend on data prepared by
    pgAdmin's Python code; see "Object definitions" below.
- **Object definitions (SQL shown in the object view):**
  - Wherever PostgreSQL provides a function that returns the definition, it is used,
    so definitions stay correct for new server versions without changes in pgtc.
    All of these exist in PostgreSQL 11:

    | Function | Returns | Since |
    |----------|---------|-------|
    | `pg_get_viewdef` | view / materialized view query | 7.x |
    | `pg_get_ruledef` | rule definition | 7.x |
    | `pg_get_indexdef` | `CREATE INDEX ...` | 7.x |
    | `pg_get_constraintdef` | PK / FK / CHECK / UNIQUE / EXCLUDE | 7.4 |
    | `pg_get_triggerdef` | `CREATE TRIGGER ...` | 7.x |
    | `pg_get_expr` | defaults, partition bounds, stored expressions | 7.x |
    | `pg_get_functiondef` | `CREATE OR REPLACE FUNCTION/PROCEDURE ...` | 8.4 |
    | `pg_get_function_arguments` / `_identity_arguments` / `_result` | function signature parts | 8.4 |
    | `pg_get_serial_sequence` | sequence owned by a column | 8.0 |
    | `pg_get_partkeydef` | `PARTITION BY ...` clause | 10 |
    | `pg_get_statisticsobjdef` | `CREATE STATISTICS ...` | 10 |

  - Objects without such a function (there is no `pg_get_tabledef`, even in PG 18) —
    tables, sequences, types, roles, databases, schemas — have their definition
    assembled by pgtc, using the functions above for the parts
    (defaults, constraints, indexes, triggers, partition key).
- **Testing:** every query runs in CI against every supported PostgreSQL version
  (e.g. Docker images), to catch version differences (*deferred* with CI, D-051;
  until then the ignored real-server tests are run by hand).
- **Minimum supported PostgreSQL version: 11** — same as pgAdmin 4 (its oldest
  version folder is `11_plus`; folders for older versions have been removed).

### D-011 — Connection design and handling of new server versions
- **Status:** Accepted (initial concept, may be refined)
- **Goal:** a new PostgreSQL release should need little or no rework.
- **Server version is a number:**
  - After connecting, `server_version_num` (e.g. `170002`) is read and stored as a number.
  - No code enumerates known versions (no `match` over 16, 17, ...). The template
    resolver (D-010) picks the newest version folder not newer than the server, so a new
    major version automatically uses the newest existing queries.
  - Result: a new PostgreSQL release needs no code change unless its catalog changed;
    then only a new SQL file or a feature entry is added.
- **Unknown newer versions:** connection is allowed; a non-blocking warning is shown
  (e.g. "PostgreSQL 19 – not yet verified with pgtc").
- **Feature table — version checks in one place:**
  - One table in code maps features to the first version that has them, e.g.
    ```rust
    (Feature::LastSeqScan,      160000),
    (Feature::NullsNotDistinct, 150000),
    ```
  - Templates and code test features (`{% if has("LastSeqScan") %}`), not raw version
    numbers. Supporting a new version feature = one line in the table + template change.
- **Saved connection (configuration file, D-007):** name, host, port, maintenance
  database, username, SSL mode. Password comes from `.pgpass` (D-008).
- **Runtime connections:**
  - A PostgreSQL connection is bound to a single database, so a connected server opens
    additional connections on demand:
    - one per database expanded in the object tree;
    - one per query tab — a long-running query does not block the tree and can be
      cancelled independently.
  - Each connection runs in its own async task (tokio, D-006). The UI communicates with
    it through channels (requests in, results/notices out) and never waits on the
    database.
- **Testing new versions:** CI runs against a list of PostgreSQL versions; supporting a
  new release starts by adding it to that list (*deferred* with CI, D-051).

### D-012 — Database structure export and sync from a definition (beyond pgAdmin)
- **Status:** Accepted (details in D-048)
- **Features:**
  1. **Export database definition** — save the whole structure of a database into a
     single file.
  2. **Load database definition** — compare a definition file with a live database and
     generate only the SQL needed to bring the database to the structure in the file.
     A clone is a load into an empty database.
- **File format: JSON** (`serde` + `serde_json`), file extension **`.pgdef`**.
  - Content is plain JSON; editors need a one-time association of `.pgdef` with JSON
    for syntax highlighting.
  - Suits the deeply nested structure (database → schemas → tables → columns,
    constraints, indexes, triggers ...); TOML is awkward for deep nesting, XML is
    verbose and less convenient in Rust.
  - **Deterministic output:** pretty-printed, objects always sorted by name, so two
    exports of the same structure produce identical files and changes show as clean
    diffs (git-friendly).
  - serde keeps the model independent of the format; other formats could be added later
    at low cost if needed.
- **Shared structure model:** one in-memory model (Rust structs describing the database
  structure):
  - Export = read database → model → write JSON file.
  - Load = read file → model; read the live database → model; compare the two →
    generate the sync statements.
  - The file is self-contained and can be loaded on another pgtc instance or machine.
  - Objects are stored as **structured properties** (e.g. column name, type, nullable,
    default), not only as DDL text, so the sync can generate precise `ALTER` statements.

### D-013 — Developer templates (beyond pgAdmin)
- **Status:** Accepted
- **Purpose:** the user picks a template for an object type, enters a **minimal set of
  data**, and gets the **complete, ready-to-run SQL**.
  - Example: template "Function" — user enters name, parameters and return type →
    pgtc produces the full `CREATE FUNCTION ...` statement.
- **Parameters:** each template defines its own, custom number of parameters; the
  template combined with the user-entered values produces the final SQL.
- **Version fallback — same mechanism as the catalog queries (D-010):**
  - Templates are organised in version folders; a template in `default/` works for all
    server versions.
  - If a version folder (e.g. `16_plus/`) contains the same template, that version is
    used when the current connection's server version matches; the resolver picks the
    newest folder not newer than the server version (D-011).
  - Rendering uses the same template engine (`minijinja`) as the catalog queries.
- **File extension:** `.pgtemplate` (one template per file).
- **Templates folder — the only one pgtc reads:** the user's data directory.

  | Platform | Templates folder |
  |----------|------------------|
  | Linux    | `~/.local/share/pgtc/developer_templates` |
  | macOS    | `~/Library/Application Support/pgtc/developer_templates` |
  | Windows  | `%APPDATA%\pgtc\developer_templates` |

  - The path is computed in `pgc-core` (D-028, D-039).
  - Inside the folder the version-folder layout applies (`default/`, `16_plus/`, ...).
  - No second (system-wide) folder and no merging by name: one folder, one file per
    template and version folder.
- **Templates that come with pgtc:**
  - **Embedded in the binary**, but only as the source for installing — pgtc never
    reads them directly. Without an installed templates folder the picker says
    *No developer templates — run Settings → Install templates*.
  - `pgtc --install-templates` (also `--setup`, D-047; *Settings → Install
    templates*) copies them into the templates folder; no administrator rights needed.
    - Folder **does not exist:** it is created and every template installed.
    - Folder **exists:** pgtc asks first whether to overwrite the templates that come
      with pgtc (on the command line `[y/N]`; no input means no). **Yes:** all of them
      are written, files of the same name are overwritten (so a new pgtc version
      brings its updated templates). **No:** nothing is written.
    - pgtc **never deletes** a template file; files of other names (the user's own
      templates) are never touched. Users who change a template that comes with pgtc
      save it under another name, so installing does not overwrite it.
- **Custom templates:**
  - The application has **no UI for creating or editing templates**.
  - Users who want their own templates write the files by hand and place them in the
    templates directory.
  - pgtc **always scans the templates directory at runtime** and offers every
    template it finds; it must not assume that only the templates installed by default
    are present.
- **Template file format** — goals: very flexible (very different kinds of SQL) and
  still simple to write by hand.
  - One `.pgtemplate` file, two parts separated by `+++` lines:
    ```
    +++
    <parameter declarations — TOML>
    +++
    <SQL body — Jinja template (minijinja)>
    ```
  - **Header (TOML):** template metadata (`name`, `category`, `description`) and the
    parameters. pgtc builds the input dialog automatically from it.
  - **Body (Jinja):** standard Jinja syntax — `{{ value }}`, `{% if %}`, `{% for %}`,
    filters, macros — rendered with minijinja (same engine as D-010).
  - **Parameter declaration:** minimum is `name` + `type`. Optional: `label` (defaults
    to `name`), `required`, `default`, `help`, plus type-specific keys.
  - **`fill`** (`sql` only, not in `list` fields): the editor box grows to every row of
    the form left after the other fields (never below its normal 5 rows); several
    `fill` boxes share the free rows; a form that has to scroll keeps normal sizes.
    The built-in function, procedure, trigger, view and materialized view templates
    use it for their body / query.
  - **Parameter types:**

    | Type | Dialog shows | Example use |
    |------|--------------|-------------|
    | `text` | single-line input | return type, comment |
    | `identifier` | input validated as a PostgreSQL name | table, function, column names |
    | `number` | numeric input | fill factor, sequence start |
    | `bool` | checkbox | `IF NOT EXISTS`, `UNIQUE`, `SECURITY DEFINER` |
    | `choice` | dropdown — fixed `options`, live `source` (`schemas`, `tables`, `types`, `roles`, ..., D-050) or custom `query = "SELECT ..."` | schema, data type, owner |
    | `sql` | multi-line editor; `fill = true`: it takes the free rows of the form | function body, CHECK expression, WHERE clause |
    | `list` | table where the user adds/removes rows; own `fields` (sub-parameters) | function arguments, table columns, index columns |

  - **Context variables** available in every body: server version and feature checks
    (`{% if has("NullsNotDistinct") %}`, D-011) and the current database (D-046).
    Templates do not refer to the tree selection; pgtc fills choice defaults from it
    automatically (D-046).
  - **Filters:** `ident` quotes identifiers correctly (plus the filters from D-010),
    so template authors do not handle quoting themselves.
  - **Deliberately not supported (for now):** conditional show/hide of fields,
    cross-field validation, multi-page wizards — `{% if %}` in the body covers most cases.
  - **Result:** the generated SQL is shown as a live, read-only preview in the dialog;
    *OK* inserts it at the cursor of the query tab's editor (D-046). It is never
    executed automatically.
  - **Example — "Function" template:**
    ```
    +++
    name        = "Function"
    category    = "Functions"
    description = "Create a PL/pgSQL function"

    [[param]]
    name    = "schema"
    type    = "choice"
    source  = "schemas"
    default = "public"

    [[param]]
    name     = "name"
    label    = "Function name"
    type     = "identifier"
    required = true

    [[param]]
    name  = "args"
    label = "Arguments"
    type  = "list"
      [[param.fields]]
      name = "arg_name"
      type = "identifier"
      [[param.fields]]
      name   = "arg_type"
      type   = "choice"
      source = "types"

    [[param]]
    name    = "returns"
    label   = "Return type"
    type    = "text"
    default = "void"

    [[param]]
    name    = "language"
    type    = "choice"
    options = ["plpgsql", "sql"]
    default = "plpgsql"
    +++
    CREATE OR REPLACE FUNCTION {{ schema|ident }}.{{ name|ident }}(
    {%- for a in args %}
        {{ a.arg_name|ident }} {{ a.arg_type }}{% if not loop.last %},{% endif %}
    {%- endfor %}
    )
    RETURNS {{ returns }}
    LANGUAGE {{ language }}
    AS $$
    BEGIN
        -- TODO
    END;
    $$;
    ```

### D-014 — Menu bar and colour schemes
- **Status:** Accepted (initial version, may be refined)
- **Menu bar:** `File` · `View` · `Query` (D-041) · `Tools` · `Settings` · `Help`
- **File**
  - New connection…
  - Connect / Disconnect
  - New query
  - Open SQL file… / Save SQL file…
  - Export database definition (`.pgdef`)… (D-012)
  - Load database definition (`.pgdef`)… (D-012)
  - Quit
- **View**
  - Refresh object tree
  - Edit in new query tab (`Ctrl+E`, D-049)
  - Select top 100 rows (`Ctrl+B` in the tree, D-056)
  - Find object… (`Ctrl+F` in the tree, D-055)
  - Next tab / Previous tab / Close tab
- **Tools**
  - Generate SQL from developer template… (D-013)
  - psql command… (D-054)
  - Server dashboard (D-057)
- **Settings**
  - Settings… — dialog with icons on/off (D-005), colour scheme, ... (contents: D-051)
  - Install templates — same as `--install-templates` (D-013)
  - Run setup — same as `--setup`: every installation step (D-047)
  - Show templates folder — displays the path of the templates folder (a terminal
    app cannot open a file manager, especially over SSH)
- **Help**
  - Keyboard shortcuts — a scrolling window (Close button, `↑↓` / `PgUp` / `PgDn` /
    `Home` / `End` / mouse wheel) listing every bound key of the **active keymap**
    (user overrides show), grouped Everywhere · Object tree · Editor and object view ·
    Results · Dialogs and lists; each row is the key(s) and a short description of
    the command. Then "Other keys" (`Esc` + digit, `F9` + letter, `Alt` + letter),
    "If a key does not work" (D-053) and the path of `keymap.toml`. The menu item opens it at the top; **`F1` opens it
    scrolled to the section of the focused frame** (dialogs → Dialogs and lists).
  - Third-party licences — the same kind of window: the pgAdmin SQL licence (D-010)
    with a *Used by* note (pgtc's catalog queries adapted from pgAdmin's SQL templates),
    then the licences of all Rust crates in the program, each licence text with the
    crates it applies to. The crate list is generated with **`cargo-about`** (a
    build-time tool, not a dependency) into `THIRD_PARTY_LICENSES/rust-crates.txt`
    (config `about.toml`, template `about.hbs` beside it), committed and embedded;
    regenerated when dependencies change. It accepts only the D-017 licences, so a
    non-permissive dependency makes generation fail.
  - About pgtc — name, version, copyright, licence (PostgreSQL Licence) and a pointer
    to Third-party licences. No links until the project has a public address.
- **Rule:** every command-line action (e.g. `--setup`, `--install-templates`) also
  has a menu entry.
- **Colour schemes:**
  - Selected in the Settings dialog with a **combo box**; applied immediately and saved
    to the configuration.
- **Frame style (mc look):** windows (the object tree pane and the tab pane) have
  **double-line frames**; separators inside a window are **single lines** joined to
  the frame (`╟───╢`), e.g. the editor / results split and the grid column lines.
  Both windows start on the row under the menu bar. The tree window has the title
  *Connections* in its top frame line. The tab window has no title (the tab strip
  names the tab, the tab status says where it comes from); inside it, top to bottom:
  **tab strip**, `─`, editor, `─`, **Results / Messages labels**, `─`, results, `─`,
  **tab status row** (D-019, D-043). Labels and status are rows of their own between
  separator lines, not text in the frame lines. The window status line under the
  windows uses the window colour (`core.normal`), like mc's hint line, so it stands
  apart from the key-hint bar. Dialogs, menus and combo lists keep
  single-line frames. Same for every colour scheme.
- **Combo boxes** (drop-downs) work the same in every dialog and in the developer
  template form (one shared implementation):
  - Closed: `Enter` / `Space` / click opens the list; `Left` / `Right` change the value.
  - Open: a framed list under the field (above it when there is no room);
    `Up` / `Down` / `PgUp` / `PgDn` / `Home` / `End` move, `Enter` / `Space` / click
    picks, `Esc` or a click outside closes, `Tab` closes and moves on, the wheel
    scrolls. Lists longer than the list box are filtered by typing.
  - Built-in schemes: **Classic** (default — Midnight Commander look, D-016),
    **Dark**, **Light**, and **Terminal default** (uses only the terminal's own 16
    colours — for limited terminals, D-002); plus **Tokyo Night** (its "Night"
    palette) and **Dracula**, which use exact 24-bit colours and need a terminal with
    true colour support.
  - **Readability rules** (checked by a test for every built-in scheme on the
    Windows Terminal "Campbell", xterm and VGA palettes; "Terminal default" on dark and
    light terminals): text roles have contrast ≥ 3 (quiet roles such as hints, borders,
    comments ≥ 2); selected / focused items, inputs, the menu bar, status line, grid
    header and cursor differ visibly from their surroundings; hotkey letters differ by
    colour **and are underlined** in the 16-colour schemes.
  - The text cursor is also drawn in reverse video (editor, dialog inputs), so it is
    visible whatever cursor colour the terminal uses.
  - Scheme files use mc-like sections (core, dialog, menu, `fkey` for the key-hint
    bar, editor, ...).
  - User schemes: `.toml` files in a user directory
    (e.g. Linux `~/.local/share/pgtc/themes`, equivalent per-OS paths via
    `pgc-core`, D-039), scanned at runtime. No scheme editor in the app — same rule as
    developer templates (D-013).
- **Menu activation:** `F9`, then arrows and highlighted hotkey letters (as in
  Midnight Commander, D-016).

### D-015 — SQL editor widget: own widget on `ropey`, own SQL lexer
- **Status:** Accepted
- **Requirements:** user friendly, and fast with **very large SQL scripts**.
- **Choice:** pgtc's own editor widget (on ratatui), with:
  - **Text buffer: `ropey`** — B-tree rope storing byte/char/line counts in every node;
    fast edits and fast jumps to any line/offset even in files of hundreds of MB.
    Same approach as Zed's own rope (SumTree), but MIT-licensed, published and mature
    (used by the Helix editor).
  - **Syntax highlighting: own SQL lexer (pure Rust)** — only one highlighter, for
    PostgreSQL SQL. No general multi-language highlighting.
    - Tokens: keywords, identifiers (incl. quoted), strings (incl. `E''`), numbers,
      comments (`--`, `/* */`), PostgreSQL dollar quoting (`$$ ... $$`, `$tag$`).
    - **Function and `DO` bodies are code:** a dollar quote whose previous word is
      `AS` or `DO` (also on the line before, comments between) is a body — its text
      is lexed as SQL (keywords incl. PL/pgSQL ones such as `DECLARE`, `ELSIF`,
      `LOOP`, `RAISE`, `PERFORM`, strings, comments, numbers), its `$tag$` drawn like
      a string. The first closing `$tag$` of an open body always ends it (also inside
      an unclosed string or comment of the body, and the bodies inside it — as
      PostgreSQL reads it); a body inside a body (`CREATE FUNCTION … AS $g$` in a
      `DO` block) and dollar-quoted strings with another tag work inside. Every other
      dollar quote (`SELECT $$text$$`) stays a string. The language is not known at
      the `$$`, so a body in another language (`plpython3u`) is also drawn as SQL.
    - Only the **visible lines** are lexed on each screen refresh.
    - Lexer state at the start of lines (inside block comment / dollar quote / string)
      is cached at checkpoints, built by a background scan when a file is opened; after an
      edit, re-lexing runs from the edited line until the state matches the cache again.
  - **Keys: standard editor keys** for text editing (as in Notepad / VS Code):
    Ctrl+C/X/V, Ctrl+Z/Y, Shift+arrows to select, Ctrl+F find, Ctrl+Home/End,
    Tab indent. No Vim/Emacs modes.
    - Additionally mc / Windows alternatives: `Ctrl+Insert` copy, `Shift+Insert` paste,
      `Shift+Delete` cut.
    - Commands (execute, save, ...) use mc-style F-keys (D-016).
  - Colours come from the colour schemes (D-014).
  - Planned functions: selection, undo/redo, find/replace, clipboard, highlighting.
- **Rejected alternatives:**
  - `ratatui-textarea` (fork of `tui-textarea`) — no syntax highlighting; line-list
    storage (`Vec<String>`) slows down on huge files.
  - `edtui` — Vim modal editing (not user friendly for most users); `syntect`
    highlighting slows down on big files.
  - `ratatui-code-editor` — right design (ropey + tree-sitter) but very early (0.0.3)
    and no SQL support.
  - Zed's `rope` crate — GPL-3.0 licence, not published, depends on Zed internals.
  - `tree-sitter` — full parse of large files is slower and uses several times the file
    size in memory; needs a C compiler for grammars. **Possible later addition** if
    features need a real syntax tree (autocomplete, statement-under-cursor, error
    marking).

### D-016 — General style: Midnight Commander look and keys
- **Status:** Accepted
- **Principle:** the general style of the application — screen layout, menus, dialogs,
  colours and shortcuts — follows **Midnight Commander (mc)**.
  Reference source: `F:\razno\mc` (`misc/mc.default.keymap`, `misc/skins/default.ini`).
- **Licence:** mc is GPL-3.0 — only its look, layout and key conventions are reused,
  **no source code**.
- **Screen:**
  - Overall layout as in mc (menu bar, two panels, key-hint bar), **with one deliberate
    difference: the right panel holds tabs** (default tab, object tabs, query tabs —
    D-009). mc's right panel has no tabs.
  - Menu bar always visible at the top (D-014).
  - **Key-hint bar** at the bottom (in place of mc's `1Help 2Menu …` F-key bar): the
    **most important keys of the focused frame, whatever the key is** — F-keys and
    others alike, e.g. `F5 Exec  F7 Template  ^R Results  ^F Find  F6 Panel`.
    - Each entry is the key (F-key number colour) followed by a short label, only as
      wide as it needs; `Ctrl+` is shown as `^`. Entries that do not fit are left out
      from the right, so the order is the priority.
    - Which commands are shown is a fixed list per frame; the **keys come from the
      active keymap** (user bindings show; an unbound command is left out). Commands
      that cannot run now are hidden (e.g. *Cancel* only while a query runs, *Fetch
      all* only while rows wait, `F8` *Delete* only on a server row).
    - Lists: object tree — Help, Menu (`F2`), New tab (`F3`), New query, Refresh,
      Search, Delete (`F8`, server rows), Panel (`F6`), Menu bar (`F9`), Quit; query
      editor — Help, Exec, Cancel, Template, psql (`F4`, D-054), Find, Results, Close tab, Panel, Menu bar,
      Quit; read-only object view — Help, Find, Find next, Copy, New query, Panel, Menu
      bar, Quit; results — Help, Exec, Cancel, Messages, Next result (more than one
      set), Fetch all, Menu, Copy, Hide, Panel, Quit; dialogs and the template form —
      OK, Cancel, Next field, Toggle; Help windows (D-014) — `↑↓` Scroll, `PgUp/PgDn`
      Page, Esc Close; open menu bar — `←→` Menus, Enter Choose, Esc Close (context
      menu: Enter, Esc).
    - A click on an entry presses its key; `Esc`+digit still works for F-keys.
- **Keys (mc meanings, adapted):**

  | Key | mc | pgtc |
  |-----|----|-------------|
  | `F1` | Help | Keyboard shortcuts of the focused frame (D-014 Help) |
  | `F2` | User menu | Context menu for the selected object |
  | `F3` | View | Show definition in a new tab |
  | `F4` | Edit | psql-style command line in a query tab (D-054) |
  | `F5` | Copy | **Execute** (in a query tab; as in pgAdmin / SSMS) |
  | `Alt+Shift+C` | — | **Cancel running query** (as in pgAdmin, D-019) |
  | `F7` | Make dir | Query tab: insert SQL from a developer template (D-013, D-046) |
  | `F8` | Delete | Object tree: delete the selected saved connection (with confirmation, D-035). Database objects are dropped with the DROP developer templates (D-050) |
  | `F9` | Menu | Menu (D-014) |
  | `F10` | Quit | Quit; closes a dialog when one is open |
  | `Ctrl+Tab` / `F6` | Switch panel | Cycle focus: object tree → editor → results (D-032) |
  | `Ctrl+R` | Re-read | Object tree: refresh tree · Query tab: show/hide results pane |
  | `Ctrl+N` | — | New query tab (global) |
  | `Alt+S` / `Ctrl+S` | Quick search | Quick search in the tree |
  | `Ctrl+F` | Find object | Find an object below the selected tree row (D-055) |
  | `Ctrl+B` | — | Object tree: select the top 100 rows of the table / view in a new query tab (D-056) |
  | `Esc` + digit | F-key substitute | Same — for SSH sessions and macOS where F-keys often do not reach the program |

- **Text editing keys:** standard keys + mc/Windows `Ctrl+Insert` / `Shift+Insert` /
  `Shift+Delete` (D-015).
- **Colours and dialogs:** the default colour scheme **Classic** reproduces the mc look —
  blue panels, cyan selection, grey dialogs with cyan focus, yellow hotkey letters,
  `[< OK >]`-style buttons; uses only the 16 basic colours, so it works on any terminal.
- **OK from any field:** `Ctrl+Enter` (`dialog_ok` in `[dialog]`) presses the default
  button of a dialog (*OK*, *Run*, …) whatever has the focus — also a focused other
  button, the template form's `sql` box and lists; an open combo list first takes its
  highlighted item. Dialogs without a default button ignore it. It works only where
  the terminal reports Ctrl+Enter (Windows natively, terminals with the kitty
  keyboard protocol, D-053); elsewhere it arrives as plain `Enter`. No second key by
  default; another one can be bound in `keymap.toml`.
- **Context-dependent keys (as in mc's keymap sections):**
  - **Global keys** — same everywhere, never redefined by a context: `F1` Help,
    `F9` Menu, `F10` Quit/Close, `Ctrl+Tab` / `F6` switch panel, `Esc` + digit F-key
    substitute.
  - **Context keys** — meaning depends on the focused panel, e.g. `Ctrl+R`:
    - object tree: refresh tree (mc meaning);
    - query tab: show / hide the results pane (SSMS meaning).
  - The **key-hint bar and menus always show the keys of the current context**.
  - Actions of another context are reached by switching focus (`Ctrl+Tab` / `F6`) or via the menu
    (e.g. View → Refresh object tree from a query tab).
- **Configurable keymap:** built-in mc-style defaults; users can override keys in a
  `keymap.toml` file in their user directory (same pattern as templates and colour
  schemes: no editor in the app, file scanned at startup). The file has one section per
  context: `[global]`, `[tree]`, `[editor]`, `[results]`, `[dialog]`.

### D-017 — pgtc licence: PostgreSQL Licence
- **Status:** Accepted
- **Requirement:** as compatible with pgAdmin as possible.
- **Choice:** the **PostgreSQL Licence** (SPDX: `PostgreSQL`) — the same licence as
  pgAdmin 4 and PostgreSQL itself. Short, permissive (BSD/MIT-like).
  Copyright holder in `LICENSE`: **Darko Prenosil**.
- **Consequences:**
  - Reused pgAdmin SQL (D-010) and pgtc's own code are under the same licence;
    code can flow in both directions without friction.
  - **Dependencies must be permissively licensed** (MIT, Apache-2.0, BSD, ISC, Zlib,
    PostgreSQL, BSL-1.0, Unicode-3.0). BSL-1.0 is used by `clipboard-win` /
    `error-code` (`arboard` on Windows); Unicode-3.0 by the Unicode data tables in
    `unicode-ident` (derive macros) and `unicode-normalization` / `unicode-properties`
    (`stringprep`, needed for SCRAM login in `postgres-protocol`). Both only require
    their notices to be shipped. This list is the `accepted` list of
    `THIRD_PARTY_LICENSES/about.toml`. Copyleft code (GPL/LGPL/AGPL) must not be included or linked —
    e.g. Zed's `rope` (D-015) and Midnight Commander's source (D-016) are excluded for
    this reason.
  - All third-party licence notices are shipped and shown under
    Help → Third-party licences (D-014): pgAdmin's and every Rust crate's, the crate
    list generated with `cargo-about`.
  - Licence compliance of dependencies is checked automatically in CI
    (e.g. `cargo-deny` with an allow-list of licences) — *deferred* with CI (D-051);
    until then `cargo about generate` (D-014) fails on a licence that is not accepted.
- **Current dependencies:** every crate in the program with its licence is listed in
  `THIRD_PARTY_LICENSES/rust-crates.txt` (generated, D-014).

### D-018 — Project structure: Cargo workspace (7 crates)
- **Status:** Accepted
- **Choice:** a Cargo workspace with seven crates. Package names use hyphens
  (`pgc-core`); in Rust code they are referenced with underscores (`pgc_core`).

| Crate | Contains | Depends on |
|-------|----------|------------|
| `pgtc` | Binary: command-line arguments (`clap`), startup, `--setup` / `--install-templates` | all |
| `pgc-ui` | ratatui application: layout, object tree, tabs, menus, dialogs, key-hint bar, colour schemes, keymap | `pgc-editor`, `pgc-db`, `pgc-model`, `pgc-sql`, `pgc-core` |
| `pgc-editor` | SQL editor widget: ropey buffer, SQL lexer, editing keys (D-015) | ratatui, `pgc-core` |
| `pgc-db` | Connections (tokio-postgres), `.pgpass`, catalog queries, object definitions, reading the structure model from a database | `pgc-sql`, `pgc-model`, `pgc-core` |
| `pgc-sql` | Versioned template resolver, minijinja setup and filters, embedded SQL files (D-010), developer templates `.pgtemplate` (D-013) | `pgc-core` |
| `pgc-model` | Database structure model, `.pgdef` read/write, compare (diff), sync script generation (D-012, D-048) | `pgc-core` |
| `pgc-core` | Shared basics used by several crates (see below) | small utility crates only |

```
pgtc (binary)
  └─ pgc-ui ─┬─ pgc-editor ──────────────────┐
             ├─ pgc-db ──┬─ pgc-sql ──────────┤
             │           └─ pgc-model ────────┤
             └────────────────────────────────┴─ pgc-core
```

- **`pgc-core` contents:**
  - `ServerVersion` and the feature table (D-011).
  - Identifier / literal quoting (`quote_ident`, `quote_literal`) — single implementation
    used by `pgc-sql` filters and `pgc-model` DDL generation.
  - Per-OS paths (own code, D-039) for config, templates, themes, keymap; settings
    structures.
  - Common error types (`thiserror`) and logging setup (`tracing`).
- **`pgc-core` rule:** only small, stable types and functions used by **at least two**
  crates; **no heavy dependencies** (no tokio-postgres, ratatui, minijinja). It stays at
  the bottom of the dependency graph so it does not become a "junk drawer".
- **Rationale:** faster incremental builds (parallel crates, UI changes do not recompile
  the database layer); layering enforced by the compiler (no circular dependencies —
  the UI depends on the database layer, never the reverse); crates such as the editor,
  the model and the `.pgpass` parser can be tested independently.
- **Conventions:** shared dependency versions in `[workspace.dependencies]`; release
  builds use LTO so cross-crate code is optimised as one unit.
- **Known costs:** crate boundaries must be planned (moving code later is possible but
  tedious); cross-crate items must be `pub`; avoid splitting into more crates without a
  clear reason.

### D-019 — Query tab: layout, execution, cancellation, results and messages
- **Status:** Accepted
- **What is executed (`F5`):**
  - If text is **selected**, only the selection is executed.
  - If nothing is selected, the **whole script** is executed.
  - Same behaviour as pgAdmin ("Execute script") and SSMS.
- **Cancel:**
  - Action placed next to "Execute" (menu / context), shortcut **`Alt+Shift+C`**
    (same as pgAdmin). Configurable via keymap (D-016).
  - While a query runs, the status line shows that it is running and how to cancel it.
  - Sends PostgreSQL's real cancel request (`CancelToken`, D-006). If the server does
    not react within a few seconds, pgtc offers to close the tab's connection;
    because each query tab has its own connection (D-011), only that tab is affected.
- **Large result sets:**
  - **Streaming:** rows are displayed as soon as they arrive; the first rows appear
    immediately, even for slow queries.
  - **Batches:** **1,000 rows** per batch by default (same as pgAdmin's
    "Data result rows per page"), configurable in Settings.
  - When the user scrolls near the end of the loaded rows, the next batch is fetched
    automatically. A **"Fetch all"** action loads the rest. The status line shows e.g.
    "1,000 rows loaded – more available".
  - **Mechanism:** between batches pgtc stops reading from the connection and the
    server waits (backpressure). Works for any statement and script; no transaction
    or server-side cursor needed.
  - **Side effect:** while a result is paused, the query stays open on the server
    (snapshot, locks). It ends when all rows are fetched, the query is cancelled, the
    tab is closed or the next query is run. The status line makes this visible.
  - **Memory / speed:** values are kept as text in a compact form; the result grid
    draws only the visible cells.
  - **Export:** every result set can be exported to a file — see D-021.
- **Layout (model: SQL Server Management Studio):**
  ```
  +------------------------------------------------------------+
  | SQL editor                                                 |
  |                                                            |
  +---------------------- (resizable split) -------------------+
  | [Results] [Messages]                                       |
  |  [1: customers 120 rows] [2: orders 1,000+] [3: ...]       |
  |  +------------------------------------------------------+  |
  |  | result grid of the selected result set (full height) |  |
  |  +------------------------------------------------------+  |
  +------------------------------------------------------------+
  ```
  - Editor at the top, **results pane** below, with a resizable split. The split is
    resized with the keys or by **dragging the results bar with the mouse**. The
    results bar is a row of labels (`Results │ Messages (n) │ 1: …`, the active one
    highlighted) between two single separator lines; the line above it is the drag
    handle (D-014).
  - Results pane has two tabs: **Results** and **Messages**.
- **Multiple result sets:**
  - Each statement returning rows produces one result set.
  - Shown as **numbered sub-tabs inside Results**:
    `[1: customers 120 rows] [2: orders 1,000+] [3: ...]` — one grid at a time, using the
    full height of the pane. The label shows the row count (`1,000+` while more rows are
    available, D-019 batches).
  - **After each execution the first result set is always the one shown (expanded).**
- **Messages tab** collects, in order of arrival:
  - server notices: `RAISE NOTICE` / `INFO` / `WARNING` (delivered immediately while
    the query runs — PostgreSQL equivalent of SSMS `PRINT`);
  - the command tag of each statement (`INSERT 0 5`, `UPDATE 3`, `CREATE TABLE`, ...) —
    equivalent of "(n rows affected)";
  - errors with DETAIL and HINT, and the line number derived from the error position;
    selecting an error jumps to that line in the editor;
  - total execution time at the end.
  - On error the Messages tab is activated automatically (the Results tab still shows
    result set 1 when switched back).
  - A run that ends **without any result set** (only command tags and notices:
    `INSERT`, DDL, `DO` with `RAISE NOTICE`, also `F4` commands) activates the
    Messages tab too. A `SELECT` with 0 rows is a result set: Results stays (the empty
    grid with its columns). A hidden results pane stays hidden; `Alt+M` switches.
- **Keys:** `Ctrl+R` shows / hides the results pane (context key, D-016).
- **Unsaved query text:** if `query.ask_save` is on (Settings, default **on**, D-043),
  closing a query tab whose text was changed since it was
  opened or last saved asks **"Save changes?"** with *Yes* / *No* / *Cancel*. *Yes* saves
  to the tab's file, or opens the Save dialog if the tab has none; the tab closes only
  after a successful save. This question comes before the open-transaction question.
  **Quitting** asks the same question for each query tab with unsaved text, one tab
  at a time (the tab is shown); *Cancel* stops quitting. Then come the open
  transactions.
- **Transactions — same as pgAdmin (simple):**
  - **Auto commit: ON** by default (Settings), toggle per query tab.
  - **Auto rollback on error: OFF** by default (Settings), toggle per query tab.
  - **Commit** and **Rollback** actions in the query tab (menu / context).
  - The executed text (selection or whole script) is sent to the server as **one
    request**, as pgAdmin does. PostgreSQL then runs a multi-statement script as one
    implicit transaction (an error rolls back the whole script; commands such as
    `VACUUM` or `CREATE DATABASE` must be run on their own).
  - **Transaction status** of the tab's connection is tracked and shown (status line and
    tab label), including the aborted state after an error.
  - Closing a tab or quitting with an open transaction asks
    **Commit / Rollback / Cancel**.

### D-020 — Query editor modes: SQL and PL/pgSQL
- **Status:** Accepted
- **Modes** (per query tab, shown in the tab label and status line):
  1. **SQL** — **default**. Executed as described in D-019.
  2. **PL/pgSQL** — procedural code with variables, `IF`, loops, etc. (PostgreSQL's
     counterpart of T-SQL; T-SQL syntax itself is not supported).
- **Switching:** *Query → PL/pgSQL mode* or `Ctrl+P` (`toggle_plpgsql` in `[editor]`
  and `[results]`) — for the active query tab only.
- **PL/pgSQL mode execution:**
  - The editor text is wrapped in an anonymous **`DO` block** and executed:
    `DO $pgc$ <editor text> $pgc$;` — if the text has no `BEGIN ... END`, it is added.
  - No `CREATE` permission is needed; nothing is created or has to be dropped.
  - `RAISE NOTICE` / `INFO` / `WARNING` output appears in the **Messages tab** (D-019),
    like `PRINT` in SSMS.
  - Limitation: a `DO` block cannot return result rows.
- **Multi-statement execution:** unchanged — same as pgAdmin (D-019): the text is sent as
  one request.
- **Possible later third mode:** procedural code that **returns result sets**, via a
  temporary function in `pg_temp` (needs only the default TEMP privilege) returning
  cursors, displayed in the numbered result sub-tabs (D-019).

### D-021 — Result export: CSV, XML, JSON
- **Status:** Accepted
- **Every result table** (each result set sub-tab, D-019) is exportable in three formats:
  **CSV**, **XML**, **JSON**. Available from the menu / context menu of the result grid.
- **Complete result:** the export contains the whole result, not only the rows loaded
  into the grid. Rows already loaded are written first; the remaining rows are read
  directly from the paused result stream (D-019) and written straight to the file
  without loading them into the grid. The query is **not re-executed**.
- **Streaming:** rows are written to the file as they arrive, so very large results do
  not need to fit in memory.
- **Formats** (UTF-8):
  - **CSV** — RFC 4180, header row with column names, quoting where needed;
    `NULL` written as an empty field.
    - **Separator configurable:** `,` (default), `;` (common in European Excel
      settings), `Tab`. Quoting follows the chosen separator.
    - **Optional UTF-8 BOM** at the start of the file, so Excel displays non-ASCII
      letters (č, ć, š, ž, ...) correctly.
    - Both options are offered in the export dialog and remembered in the settings.
  - **JSON** — array of objects, one object per row, keyed by column name; `NULL` as
    JSON `null`. Values are **typed** (D-042).
  - **XML** — layout in D-042: column names once in a `<columns>` header as escaped
    `name` attributes (column names are not always valid XML element names), one
    `<row>` per row with `<value>` children in column order; `NULL` marked with
    `null="true"` (same approach as pgAdmin).
    - XML 1.0 forbids most control characters (0x00–0x08, 0x0B, 0x0C, 0x0E–0x1F) even
      when escaped. A value containing such characters is written **base64-encoded**
      with the attribute `encoding="base64"` — the file stays valid and no data is lost.
  - Duplicate column names in a result (e.g. `SELECT 1 AS a, 2 AS a`) are made unique
    for JSON keys (e.g. `a`, `a_2`).
- **Escaping:** all values are written through the crates' APIs, never assembled by
  hand, so escaping is automatic:
  - `csv` quotes fields containing the separator, quotes or line breaks and doubles
    embedded quotes;
  - `serde_json` escapes `"`, `\` and control characters;
  - `quick-xml` escapes `<`, `>`, `&` (and `"`, `'` in attributes).
- **Crates:** `csv`, `serde_json`, `quick-xml` (all permissive, D-017).
- **Note:** over SSH the file is written on the machine running pgtc (the
  server), not on the user's local machine.

### D-022 — Distribution: source only
- **Status:** Accepted (for now)
- pgtc is distributed **as source code only** — no binary packages
  (`.deb`, `.rpm`, Homebrew, winget, ...) and no installers for now.
- Users build it themselves with Cargo, e.g. `cargo build --release` or
  `cargo install --path crates/pgtc`.
- **Consequences:**
  - The templates that come with pgtc are embedded in the binary and installed into
    the user's templates folder with `--setup` / `--install-templates` (D-013).
  - CI builds and tests on Windows, Linux and macOS (D-003) but does not publish
    binaries (*deferred* with CI, D-051).

### D-023 — Clipboard: arboard + OSC 52, internal buffer
- **Status:** Accepted
- **Goal:** clipboard that works in as many situations as possible — locally on
  Windows / macOS / Linux (X11, Wayland) and over SSH.
- **Crates:**
  - `arboard` — system clipboard locally (Windows, macOS, Linux X11 and Wayland).
  - **OSC 52** via crossterm's `CopyToClipboard` (crossterm already used, D-004) — the
    terminal escape sequence that puts text into the clipboard of the machine where
    the **terminal** runs; works over SSH in terminals that support it (Windows
    Terminal, iTerm2, kitty, WezTerm, Alacritty, ...).
- **Copy:**
  - Text always goes into an **internal buffer** (copy/paste inside pgtc always
    works).
  - If the system clipboard is available (`arboard`), it is set there too; one
    `arboard` handle is kept alive for the whole session (required on X11/Wayland).
  - If an SSH session is detected (`SSH_CONNECTION` / `SSH_TTY`) or `arboard` is not
    available, text is also sent via **OSC 52**.
  - OSC 52 and system clipboard can each be switched on/off in Settings.
- **Paste:**
  - System clipboard (`arboard`) first; if unavailable or empty, the internal buffer.
  - **Terminal paste** (the terminal's own paste, e.g. `Ctrl+Shift+V` / right click) is
    always accepted via bracketed paste (crossterm `Event::Paste`) — this is how paste
    works over SSH, since reading the clipboard through OSC 52 is rarely supported.

### D-024 — Testing: deferred
- **Status:** Deferred — to be decided later.
- Discussed proposal (not yet accepted), kept here as a starting point:
  unit tests (`cargo test`), property tests (`proptest`), snapshot tests (`insta` +
  ratatui `TestBackend`), database integration tests against PostgreSQL 11–18 in Docker
  (skipped when no test database is configured), benchmarks (`criterion`) for the editor
  and lexer; CI checks `cargo fmt`, `cargo clippy`, `cargo-deny`.

### D-025 — Source code hosting
- **Status:** Accepted (for now)
- Until the first alpha release the source lives **only on the developer's local
  machine** (`F:\razno\rust\pgtc`).
- It will probably be published on **GitHub** later; CI system to be chosen then
  (affects D-022 CI builds and D-024 testing).
- The folder is a **local git repository** (no remote); commits are made only on the
  owner's request.

### D-027 — Rust toolchain and edition
- **Status:** Accepted
- The toolchain is pinned in `rust-toolchain.toml` to **Rust 1.98.1** (with `rustfmt`
  and `clippy`), so the build does not depend on the developer's global default.
- **Edition 2024**, workspace resolver 3; `rust-version = "1.86"` (minimum required by
  ratatui 0.30, D-004) declared in the workspace.
- Upgrading the pinned toolchain is a deliberate change (edit the file, note it here).

### D-028 — User directories and file locations
- **Status:** Accepted
- **Flat layout** — one `pgtc` folder under the OS base directories (computed in
  `pgc-core`, D-039), no extra `config\` / `data\` sub-folders:

  | Platform | Config dir | Data dir |
  |----------|------------|----------|
  | Linux    | `~/.config/pgtc` | `~/.local/share/pgtc` |
  | macOS    | `~/Library/Application Support/pgtc` | same |
  | Windows  | `%APPDATA%\pgtc` | same |

- **Files:**
  - Config dir: `settings.toml` (settings), `keymap.toml` (key overrides, D-016).
  - Data dir: `developer_templates/` (D-013), `themes/` (D-014), `logs/` (D-029).
- All paths are computed only in `pgc-core` (CLAUDE.md).

### D-029 — Log file
- **Status:** Accepted
- Log files in `<data dir>/logs/` (D-028), **rolled daily**:
  `pgtc.YYYY-MM-DD.log`; the **last 7** files are kept, older ones deleted.
- Default level **`info`**; overridable with the environment variable
  **`PGTC_LOG`** (`tracing-subscriber` filter syntax, e.g. `debug` or
  `pgc_db=trace`).
- Crates: `tracing`, `tracing-appender`, `tracing-subscriber` (MIT, D-017).
- Never logs to stdout/stderr (TUI, D-007).

### D-030 — Names
- **Status:** Accepted
- Executable, binary crate, user/system folders and environment variable use the short
  name **`pgtc`** (`crates/pgtc`, `~/.config/pgtc`, `PGTC_LOG`, `pgtc.YYYY-MM-DD.log`).
- Long application name (shown in `--help`, About): **Postgres terminal commander**.
- Internal library crates keep the `pgc-` prefix (D-018).

### D-031 — Command-line arguments: psql-compatible connection options
- **Status:** Accepted
- Connection arguments use the **same syntax as `psql`**:
  `pgtc [OPTION]... [DBNAME [USERNAME]]`

  | Option | Meaning |
  |--------|---------|
  | `-h`, `--host=HOSTNAME` | server host or socket directory |
  | `-p`, `--port=PORT` | server port |
  | `-U`, `--username=USERNAME` | user name |
  | `-d`, `--dbname=DBNAME` | database name |
  | `-w`, `--no-password` | never prompt for a password |
  | `-W`, `--password` | always prompt for a password |
  | `-?`, `--help` | help (as in psql; `-h` is the host) |
  | `-V`, `--version` | version |

- Positional `DBNAME` and `USERNAME` are used when the options are not given (as psql).
- Plus pgtc's own actions: `--setup` (all installation steps, D-047),
  `--install-templates` (D-013).
- **Defaults from environment variables (as libpq / psql):** `PGHOST`, `PGPORT`,
  `PGUSER`, `PGDATABASE`, `PGSSLMODE`, `PGSSLROOTCERT`, `PGPASSWORD`; `PGPASSFILE` for
  `.pgpass` (D-008). Built-in defaults: host `localhost`, port `5432`, user = OS user,
  database = user, SSL mode `prefer`.
- **Connection URIs:** `DBNAME` (positional or `-d`) may be
  `postgresql://[user[:password]@][host][:port][/dbname][?sslmode=...&sslrootcert=...]`
  (also `postgres://`). Precedence: URI > options > environment > defaults.
- **Startup connection:** when connection arguments are given, the connection is
  **saved automatically** (unless an identical one — same host, port, database, user,
  SSL mode — already exists) and connected at once. A password from the URI or
  `PGPASSWORD` is used for the session only (not written anywhere).

### D-033 — TLS (SSL modes)
- **Status:** Accepted
- libpq SSL modes: `disable`, `prefer` (**default**), `require`, `verify-ca`,
  `verify-full`; stored per saved connection.
  - `require` encrypts but does not check the certificate (libpq semantics);
  - `verify-ca` checks the chain; `verify-full` also the host name.
- Trust: the **OS certificate store** (`rustls-native-certs`), or only the file given as
  the connection's root certificate (`sslrootcert`), as libpq.
- Implemented with `rustls` (crypto provider `ring`, avoids a CMake/NASM build on
  Windows) via `tokio-postgres-rustls`. Client certificates: later.

### D-034 — Supported server versions
- **Status:** Accepted (refines D-010, D-011)
- Servers **older than PostgreSQL 11 are refused** with a message after connecting.
- Newest verified version: **18**. Newer servers connect with the non-blocking
  "PostgreSQL N – not yet verified with pgtc" warning. The number is one constant in
  `pgc-core`, raised when a release has been verified.

### D-035 — Managing saved connections; passwords; connect timeout
- **Status:** Accepted (refines D-008, D-011, D-014)
- Saved connections live in `settings.toml` (`[[servers]]`): name, host, port,
  maintenance database, user name, SSL mode, root certificate file (D-011, D-033).
  In the New / Edit connection dialog the SSL mode is a combo box (drop-down).
- **Context menu (`F2` / right click) on a server:** Connect, Disconnect, Refresh,
  Edit connection…, Delete connection (with confirmation) (D-049). `F8` (Drop) on a server
  also deletes it (with confirmation). File → Connect / Disconnect act on the server
  selected in the tree.
- **Passwords (D-008):** the New / Edit connection dialog has **Password** and
  **Save password**; if connecting needs a password that is not known (dialog, URI,
  `PGPASSWORD`, `.pgpass`), a **login dialog** asks for it, also with **Save password**.
  On a wrong password the login dialog is shown again with the error. Saved entries use
  the maintenance database; the password is kept in memory for the session and reused
  for that server's per-database connections.
- **Connect timeout:** **10 s** by default, `connect_timeout_secs` in `settings.toml`,
  editable in the Settings dialog.

### D-032 — Key details: Tab, right-pane tabs, Esc + digit, mouse
- **Status:** Accepted (refines D-016)
- **`Tab` is used only by the editor and dialogs:**
  - dialogs, menus and the developer template form: `Tab` / `Shift+Tab` move between
    fields (mc);
  - SQL editor: `Tab` / `Shift+Tab` indent / unindent (D-015, D-038);
  - in the object tree and the results `Tab` does nothing.
- **Switching panels:** `Ctrl+Tab` cycles the focus object tree → editor → results (the
  results only while shown) → tree, from every panel. `F6` does the same, because many
  terminals never pass `Ctrl+Tab` on (the classic Windows console and SSH send a plain
  `Tab`, Windows Terminal uses it for its own tabs).
- **Right-pane tab keys** (`[global]`, configurable): next tab `Alt+Right`, previous
  tab `Alt+Left`, close tab `Ctrl+W`. The default tab cannot be closed.
- **`Esc` + digit (mc style):** `Esc` opens a **1-second** window; a digit pressed in it
  becomes `F1`..`F10` (`0` = `F10`). If nothing follows, `Esc` acts as `Esc` after the
  timeout; if another key follows, `Esc` is processed first, then that key.
- **Mouse (basic, from Phase 1):** left click on menu titles and items, right-pane tabs,
  dialog buttons / checkboxes / lists / inputs, key-hint bar entries and panes (focus).
  Right click on the tree = context menu (`F2`). Mouse capture is on; terminals still
  allow text selection with `Shift`+drag.

### D-036 — Object tree contents and behaviour
- **Status:** Accepted (refines D-009, D-016)
- **Object types** (community PostgreSQL only; no EDB-only types):
  - Server: Databases, Login/Group Roles, Tablespaces.
  - Database (alphabetical, as in pgAdmin): Casts, Catalogs, Event Triggers,
    Extensions, Foreign Data Wrappers, Languages, Publications, Schemas, Subscriptions.
  - **Catalogs** (pgAdmin rules): *PostgreSQL Catalog (pg_catalog)* and *ANSI
    (information_schema)*, **always shown** whatever the system objects setting is,
    and never listed under Schemas. Each catalog has the same groups as a schema and
    the same object view and context menu. pgAdmin's *pgAgent* catalog is not used
    (`pgagent` stays an ordinary schema).
  - Schema: Tables, Views, Materialized Views, Functions, Procedures, Trigger Functions,
    Sequences, Types, Domains, Aggregates, Collations, Operators, FTS Configurations,
    FTS Dictionaries, FTS Parsers, FTS Templates, Foreign Tables.
  - Table: Columns, Constraints, Indexes, Triggers, Rules, Policies, Partitions
    (partitioned tables only; partitions have the same children as tables).
  - View: Columns, Rules, Triggers. Materialized view: Columns, Indexes. Foreign table:
    Columns.
- **System objects** (Settings → *Show system objects in the tree*; no default key,
  D-044): pgAdmin rules — `pg_toast` / `pg_temp` schemas (the catalogs have their own
  group, see above), template databases, `pg_*` roles and objects that
  belong to extensions are **hidden by default**; the toggle applies to all trees and is
  saved in `settings.toml`.
- **Quick search** (`Alt+S` / `Ctrl+S`), mc style: typed letters move the cursor to the
  next visible row whose name starts with them (case-insensitive); the same key again
  finds the next match; `Esc`, arrows or `Enter` end it; the search text is shown in the
  status line.
- **Refresh** (`Ctrl+R`): reloads the selected node's subtree, keeping the expanded state
  of objects that still exist. Group nodes show counts once loaded, e.g. `Tables (12)`.
- Tree queries are versioned templates (D-010) adapted from pgAdmin's `nodes.sql`; object
  OIDs are passed as bind parameters.

### D-037 — Object view (SQL definitions)
- **Status:** Accepted (refines D-009, D-010)
- **Content — like pgAdmin's SQL tab:** header comment (`-- Table: public.x`), the
  commented `DROP`, the `CREATE` statement(s), `OWNER`, `COMMENT ON` and `GRANT`
  statements. Tables (and partitions) also include their indexes, triggers, rules and
  policies; views their rules and triggers; materialized views their indexes.
- Definition queries are versioned templates `sql/<object>/<version>/definition.sql`
  returning `kind`, `name`, `drop`, `body`; the parts come from the `pg_get_*`
  functions where PostgreSQL has them (D-010). Identifiers inside these queries are
  quoted by the server (`quote_ident`), because they come from the catalog.
- **Rows that are not objects** show info text as SQL comments: a server shows name,
  host:port, maintenance database, user, SSL mode, version and state; a group lists the
  objects in it.
- **Server configuration:** below the version, a connected server also lists **all**
  configuration settings it shows the user (`pg_settings`, query
  `sql/server/<version>/settings.sql`), sorted by name: name, value as `SHOW` prints it
  (`current_setting`), source and — where the server reports it — the file and line it
  was set in; settings waiting for a restart are marked `(restart needed)`. Read on the
  server's maintenance connection the first time the server row is selected, then kept
  until the server is disconnected; `Ctrl+R` on the server row reads them again.
  "loading…" is shown meanwhile, an error as a comment line.
- **Keys:** the object view is read-only text and uses the `[editor]` keymap context
  (scrolling keys). Keys that edit do nothing there; to edit the text, copy it into a
  new query tab (D-049).
- **Default tab follows the tree:** the definition is requested when the cursor has
  rested for **200 ms**; older answers are dropped; "loading…" is shown meanwhile.
- **Context menu (`F2`) on tree rows:** depends on the row kind, see D-049.

### D-038 — SQL editor details
- **Status:** Accepted (refines D-015, D-016, D-032)
- **Leaving the editor:** `F6` and `Ctrl+Tab` (where the terminal delivers it) switch
  focus back to the object tree; `Tab` indents.
- **Indentation:** real tab characters; display width configurable
  (`editor.tab_width` in `settings.toml`, default 4). With a selection (also over
  several lines) `Tab` / `Shift+Tab` indent / unindent **all selected lines**; without
  a selection `Tab` inserts a tab and `Shift+Tab` unindents the current line (one tab
  or up to tab-width leading spaces).
- **Line numbers:** gutter on the left, on by default (`editor.line_numbers`).
- **Find / replace:** mc-style dialog (`Ctrl+F`): Find, Replace with, Case sensitive,
  Whole words, Regular expression; buttons Find / Replace / Replace all / Cancel.
  Replace asks per match (mc): Replace / All / Skip / Cancel. `F3` / `Shift+F3` find
  the next / previous match with the last options. Matches do not span lines.
- **Object view** (default / object tabs, D-037) uses the same widget **read-only**:
  highlighting, line numbers, cursor, selection, copy and find work; edits are ignored.
- **Keys** (`[editor]`, configurable): arrows, `Ctrl+Left/Right` word, `Home` (first
  non-blank, then column 1) / `End`, `PgUp` / `PgDn`, `Ctrl+Home` / `Ctrl+End`; `Shift`
  + any of these selects; `Ctrl+A` select all; `Ctrl+C` / `Ctrl+Insert` copy, `Ctrl+X` /
  `Shift+Delete` cut, `Ctrl+V` / `Shift+Insert` paste; `Ctrl+Z` undo, `Ctrl+Y` /
  `Ctrl+Shift+Z` redo; `Ctrl+Backspace` / `Ctrl+Delete` delete word. Mouse: click places
  the cursor, drag selects, wheel scrolls.
- **Syntax colours:** `[editor]` roles in the colour schemes (text, keyword, string,
  number, comment, quoted identifier, operator, selection, line number); mc-like
  defaults in all four built-in schemes.
- **Lexer cache:** lexer state stored every 512 lines; visible lines are lexed from the
  nearest checkpoint; a background scan fills the cache after a text is loaded; an edit
  drops the checkpoints after the edited line, which are rebuilt on demand / in the
  background.

### D-039 — No `directories` crate: own per-OS path code
- **Status:** Accepted (changes D-007, D-028)
- `directories` pulled in `option-ext` (MPL-2.0), which is not on the D-017 allow-list.
  The few rules are implemented in `pgc-core` instead, from environment variables:
  - Windows: `APPDATA`, else `USERPROFILE\AppData\Roaming` (config = data);
  - Linux / other Unix: `XDG_CONFIG_HOME` / `XDG_DATA_HOME` when set to an absolute
    path (as the XDG spec says), else `~/.config` / `~/.local/share` (`HOME`);
  - macOS: `~/Library/Application Support` (config = data);
  - `.pgpass` (D-008): `PGPASSFILE`, else `%APPDATA%\postgresql\pgpass.conf` /
    `~/.pgpass` — the same rules as libpq.
- Every compiled dependency (all target platforms) is checked to be permissively
  licensed (D-017).

### D-040 — Patched tokio-postgres for the query tool
- **Status:** Accepted (refines D-006)
- tokio-postgres 0.7.18's simple-query stream drops the **command tag text**, the
  **transaction status** (ReadyForQuery) and **column type OIDs** — all needed by D-019.
- A copy of tokio-postgres 0.7.18 (MIT / Apache-2.0) lives in `vendor/tokio-postgres`
  and is used through `[patch.crates-io]`; the change is small and marked `pgtc patch`:
  - the codec records the status byte of every ReadyForQuery;
    `Client::transaction_status()` returns Idle / InTransaction / Failed (also after an
    error, when the result stream was dropped);
  - `SimpleQueryStream::command_tag()` returns the tag of the last CommandComplete;
  - `SimpleColumn::type_oid()` returns the column's type OID.
- `vendor/` is excluded from the workspace (no fmt / clippy on it). If upstream adds
  these, the patch is dropped; an upstream pull request is possible later.

### D-041 — Query tab details
- **Status:** Accepted (refines D-011, D-014, D-016, D-019, D-020)
- **Menu bar:** File · View · **Query** · Tools · Settings · Help. Query: Execute,
  Cancel, Fetch all, Commit, Rollback, Auto commit [x], Auto rollback [x], PL/pgSQL
  mode [x], Show / hide results, Results / Messages, Next / Previous result set
  (D-053). Items are disabled when no query tab is active.
- **Binding:** New query and Open SQL file use the database of the selected tree node
  (the database itself or an object below it); they are not available elsewhere
  (D-049). The tab's own connection (D-011) is opened when the tab is created; the tab
  label shows the database (`Query 1 – mydb`). A tab that lost its connection is
  unbound and F5 asks to select a database in the tree.
- **Focus:** `F6` / `Ctrl+Tab` cycle editor → results → object tree → editor.
- **Keys** (configurable, also in the Query menu): Results / Messages `Alt+M`; next /
  previous result set `Ctrl+PgDn` / `Ctrl+PgUp` (also `Alt+PgDn` / `Alt+PgUp`, D-053); Fetch all `Alt+End`; results pane
  smaller / larger `Alt+Shift+Up` / `Alt+Shift+Down`; PL/pgSQL mode `Ctrl+P` (D-020).
  Commit, Rollback and the auto toggles: menu only (no default key).
- **Result grid (pgAdmin-like):** NULL as dimmed `[null]`; line breaks and tabs in
  values shown as `↵` / `→` (copied unchanged); column width = widest of header and
  loaded values, capped at 40 (longer values cut with `…`); numbers right-aligned;
  row-number column; cell cursor with arrows; `Ctrl+C` copies the cell, `Ctrl+A` then
  `Ctrl+C` copies the result set tab-separated with a header row.
- **File → Open / Save SQL file:** mc-style dialog with a path input and a directory
  listing (directories first, `*.sql` highlighted); Enter opens a directory or picks a
  file; Save asks before overwriting; files are UTF-8.
- **Cancel:** if the server has not reacted after `query.cancel_wait_secs` seconds
  (settings, default **5**), pgtc asks whether to close the tab's connection.

### D-042 — Result export details
- **Status:** Accepted (refines D-021)
- **Where:** Query → *Export result…* and the context menu of the results pane
  (`F2` / right click on the grid). No default key; bindable in `keymap.toml`. Exports
  the result set shown in the grid.
- **Dialog:** the mc-style file dialog of File → Save (folder, file name, listing)
  plus *Format* combo (CSV / JSON / XML), *Separator* combo and *UTF-8 BOM* checkbox
  (both only for CSV). Default name `result_N.<ext>`; the extension follows the
  format. Format, separator and BOM are remembered in the settings. Asks before
  overwriting.
- **JSON values:** numbers (`int2`, `int4`, `int8`, `oid`, `float4`, `float8`,
  `numeric`) as JSON numbers written exactly as PostgreSQL sent them (no float
  conversion; `NaN` / `Infinity` as strings), `bool` as `true` / `false`, `json` /
  `jsonb` embedded as JSON, everything else as strings.
- **XML layout:**
  ```xml
  <?xml version="1.0" encoding="UTF-8"?>
  <result>
    <columns><column name="id"/><column name="name"/></columns>
    <row><value>1</value><value>Čaša</value></row>
    <row><value>2</value><value null="true"/></row>
  </result>
  ```
  A value or column name with characters XML 1.0 forbids gets `encoding="base64"`.
- **Progress / cancel:** the status line shows the rows written; `Alt+Shift+C`
  cancels the export. On cancel or error the incomplete file is **deleted** and a
  message says why. If rows were already taken from the paused stream, the query is
  cancelled too (those rows are gone from the stream).
- **Grid afterwards:** the grid keeps the rows it had loaded; the sub-tab label and the
  status line show e.g. `1,000 of 1,234,567 rows (rest exported)`. The query has then
  finished, so its snapshot / locks are released.
- **Line endings:** CSV uses CRLF (RFC 4180); JSON and XML use LF.

### D-043 — Per-tab status bar; disconnect closes the server's tabs
- **Status:** Accepted (refines D-009, D-011, D-019, D-041)
- **Per-tab status bar:** each tab has its own status row, the **last row inside the
  tab's frame**, below a separator line (below the results pane of a query tab;
  D-014): the connection and transaction part on the left, progress and `Ln, Col` on
  the right, both as labels in the status colour. Each query tab has its own connection (D-011), so the user
  sees at a glance where every tab is connected.
  - Query tab: `server · user@database`, or the connection state (*connecting…*,
    *not connected*, *no database — select one in the tree*); *Executing…* /
    *n rows loaded – more available* / *Exporting…*; after a run, until the next one,
    *Execution finished in …* / *Execution failed after …* (errors and cancel count
    as failed); transaction state; *PL/pgSQL*;
    `Ln, Col`.
  - Object tabs (default tab, "Show in new tab"): at the same place as a query tab's
    connection, where the object comes from followed by *read only*
    (`server · user@database · read only`; server-level rows without the database),
    and `Ln, Col`. The default tab follows the tree selection. No tab has a title in
    its top frame line (the tab strip names the tab, D-014).
- **Close mark:** every tab except the default tab shows a `×` at the right of its
  name; clicking it closes the tab exactly like `Ctrl+W` (same questions). It is a
  plain character, shown with icons on or off (D-005).
- **Window status line:** only messages (e.g. *Saved …*, *Disconnected*, hints, the
  `Esc` digit prompt). It no longer shows tab state. After a run of the active tab it
  replaces *Executing…* with *Execution finished in …* / *Execution failed after … —
  see Messages*.
- **Disconnect server** closes everything of that server: all query tabs bound to it
  and all object tabs showing its objects; the default tab is cleared. Afterwards no
  connection to that server stays open.
  - Before closing, the same questions as for quitting: *Save changes?* for each
    unsaved query tab of the server (if `query.ask_save` is on), then the
    open-transaction question (*Commit all* / *Rollback all* / *Cancel*). *Cancel*
    at any point stops the disconnect and nothing is closed.
- **Delete connection** on a server does the same after its own confirmation: the
  same questions, then the server's tabs and connections close and the server leaves
  the tree and the settings. *Cancel* at any point keeps the server and its tabs.
- **Setting `query.ask_save`** (Settings dialog checkbox, default **on**): whether
  pgtc asks to save unsaved query text when a query tab is closed — by `Ctrl+W`, by
  quitting or by disconnecting its server. Off: unsaved text is discarded without a
  question.

### D-044 — Grid lines, New query key, system objects in Settings
- **Status:** Accepted (refines D-016, D-036, D-041)
- **Result grid:** a vertical line `│` (border colour) separates the row-number
  column and every column, in the header and in the rows.
- **New query:** `Ctrl+N` (global context).
- **Show system objects** moved from the View menu into the **Settings dialog**
  (checkbox); the `Alt+.` key is removed (the command stays bindable in
  `keymap.toml`).

### D-045 — Resizable tree pane
- **Status:** Accepted (refines D-009)
- The border between the object tree and the tab area is dragged with the mouse to
  change the tree's width. Default: a third of the window; each pane keeps at least
  20 / 30 columns. The width is remembered in `settings.toml` (`ui.tree_width`).

### D-046 — Developer templates: picker, context, result
- **Status:** Accepted (refines D-013)
- **Only from a query tab:** *Generate SQL from developer template* (Tools menu, `F7` in the
  editor) works on the active query tab. The tab must have an **active connection**;
  otherwise an error is shown and nothing is generated.
- **Picker:** a tree built from the **folder structure** of the template directories,
  read inside the version folders (`default/Functions/function.pgtemplate` →
  *Functions › Function*) of the templates folder (version fallback as in D-013).
  The folder is the category, so the
  header key `category` is not needed; the label is the header `name` (else the file
  name). A **search box** at the top filters the tree by name and description; the
  selected template's description is shown below the tree.
- **Parameters** are fixed by the template and all shown in the dialog (no adding or
  removing of parameters). Widgets per type as in D-013:
  - `list`: the rows of the value are shown as a list with *Add*, *Edit*, *Delete*,
    *Up*, *Down*; *Add* / *Edit* open a small dialog with the row's `fields`.
  - `sql`: a multi-line SQL editor box in its own single frame, the parameter name
    as the frame's title; 5 rows, or the free rows of the form with `fill` (D-013).
    Autocomplete works in it as in a query tab (D-052).
  - `identifier`: validated as a PostgreSQL name.
- **Live choice lists** (`source`, `query`) are read through the **query tab's own
  connection**, so they list the objects of the tab's database. Sources: see D-050.
  Objects are schema-qualified; a value of such a choice has `schema` and `name`, and
  `{{ value|ident }}` gives the quoted qualified name (`"sales"."orders"`).
- **Defaults from the object browser:** if the object selected in the tree belongs to
  the **same server and database** as the tab, it pre-fills choice parameters of the
  matching source (a selected table → `tables`; its schema → `schemas`). Otherwise
  it is treated as if nothing were selected and the template's own `default` is used.
  The user can always pick another value in the combo.
- **Preview and result:** the dialog shows the generated SQL in a read-only box that
  updates as the fields change. Errors (missing required value, invalid identifier,
  template error) are shown in the preview. *OK* inserts the SQL **at the cursor** of
  the tab's editor (replacing a selection); it is not executed.
- **Window:** drawn like every dialog (margin of dialog colour, single frame, shadow),
  as large as the screen between the menu bar and the F-key bar. Form on the left,
  preview on the right, divided by a vertical line joined to the frame (`┬ … ┴`); the
  window title is centred over the form, the preview has its title row (*Preview*)
  under its own line (`├───┤`). On narrow screens the preview is below the form,
  divided by a horizontal line with the title (`├─ Preview ──┤`). A line across the
  window (`├───┤`) separates the help of the focused field and the *OK* / *Cancel*
  row.

### D-047 — `--setup`; no font installation
- **Status:** Accepted
- **`pgtc --setup`** runs every installation step, one after another. Today that is
  only install templates (D-013); later steps are added to the same list.
  - One line per step says what was done or why it failed; a failed step does not stop
    the next one; the exit code is non-zero if any step failed.
  - The single-step switch `--install-templates` stays.
  - Installing templates into an existing templates folder asks first (D-013); *No*
    skips only that step.
  - Menu entry (rule "every command-line action has a menu entry"): **Settings → Run
    setup**; the result of each step is shown in a message.
- **Fonts are not bundled or installed** (D-005 updated): a terminal app cannot choose
  the terminal's font, so installing fonts would still leave per-terminal, per-platform
  steps to the user. `--install-font` and Settings → Install fonts are removed.

### D-048 — Definition export and load: details
- **Status:** Accepted (refines D-012)
- **Both commands need a database selected in the tree** (menu items disabled
  otherwise); that database is the source (export) or the target (load). No selection
  dialogs besides the file dialog.
- **Where:** File menu, and the context menu (`F2` / right click) of a database in the
  tree, below a separator after the common items (D-037).
- **Export database definition:** exports **everything** in the selected database (the
  object types below); asks only for the file name.
- **Load database definition:** asks for a `.pgdef` file, reads the **live structure** of
  the selected (existing) database, compares the two and generates **only the statements
  that are needed** (`CREATE` / `ALTER`, and drop + create where an object must be
  replaced, e.g. a changed view or function signature). The script opens in a new query
  tab connected to that database for review; it is never run automatically. If nothing
  differs, a message says so and no tab opens.
  - **No drops of objects that exist only in the database** (tables, columns, functions,
    ...): the load adds and changes, it never removes.
  - **Replacing:** objects that cannot be altered are dropped and created again
    (constraints, indexes, triggers, rules, policies, materialized views, functions
    whose result type changed, generated columns whose expression changed).
  - **Tables are never dropped without a copy.** A table is re-created only when its
    partitioning changed. The script first copies it with its data into
    `_TableCopy_<name>` (same schema), drops it, creates it as in the file (plus
    columns and parts that exist only in the database), loads the rows back
    (identity values continue after them) and re-attaches owned sequences. Its
    partitions are dropped with it and created again (with their parts that exist
    only in the database) — also a partition whose own partitioning changed; their
    rows are in the parent's copy and come back through the parent. At the end
    of the script a commented-out `DROP TABLE _TableCopy_<name>` line lists each copy,
    so the user drops the copies once sure the migration went well.
  - **No `CREATE DATABASE`:** the target is always an existing database.
  - Renamed objects cannot be recognised; they appear as a new object.
- **Object types** (first alpha): extensions, schemas, types (enum, composite, domain,
  range), sequences, tables (columns with type / NOT NULL / default / identity /
  generated; constraints PK / unique / FK / check / exclude; indexes; partitioning),
  views, materialized views, functions and procedures, triggers, rules, policies,
  comments. Objects belonging to an extension are covered by its `CREATE EXTENSION`.
  Not included for now: roles, tablespaces, foreign data wrappers / foreign tables,
  publications, subscriptions, event triggers, casts, operators, aggregates,
  collations, full-text search objects.
- **Structured vs. definition text:** tables, sequences and types are fully structured;
  views, functions, triggers, rules and policies keep name and key properties plus the
  server's definition text (`pg_get_*`), and are replaced as a whole when it differs.
- **Not in the file:** owners and privileges (never set by the load), server version,
  pgtc version, timestamps or any other header — only the structure.
- **Ordering:** the script is ordered by a dependency graph (topological sort) built
  from `pg_depend` and from references in the model (column types, foreign keys,
  sequences, functions used by defaults, ...); dependency cycles between tables are
  broken by adding foreign keys afterwards with `ALTER TABLE … ADD CONSTRAINT`.
- **Menus:** Tools → *Create database from definition* and *Compare & sync databases*
  are removed; File keeps *Export database definition…* and *Load database
  definition…*.

### D-049 — Menu and context menu rules
- **Status:** Accepted (refines D-009, D-035, D-037, D-041, D-048)
- **New query, Open SQL file… (`Ctrl+N` too):** only when the selected tree row is a
  database or an object below it, on a connected server; the new tab is bound to that
  database. Disabled on servers, server-level rows (Databases group, roles,
  tablespaces, …) and without a selection. A key for a disabled command only shows a
  hint in the status line.
- **Save SQL file…:** only when the active tab is a query tab (not the object view).
- **Edit in new query tab** (`Ctrl+E`, `edit_in_query_tab` in `[editor]` and `[tree]`;
  View menu; context menu): the whole definition shown in the read-only view (the
  default tab — from the tree, the one of the selected object — or an `F3` object tab)
  goes into a **new query tab bound to the same server and database**, with its own
  connection (D-011), as unsaved text (closing asks to save, D-019); SQL mode, cursor
  at the start. In a query tab the key does nothing. Refused with a status-line hint
  (same rules as *New query*): while the definition loads, on info texts and errors
  (no definition), on server-level objects (roles, tablespaces — no database, no
  maintenance-database fallback), on a server that is not connected. (It replaces the
  earlier *Open in query tab* on `F4`, removed on 2026-11-09.)
- **Context menus (`F2` / right click), by row kind** (items that do not apply are left
  out; `│` = separator):

  | Row | Items |
  |-----|-------|
  | Server | Connect, Disconnect │ Refresh │ Edit connection…, Delete connection |
  | Server-level groups and objects | Refresh |
  | Database | New query, Edit in new query tab, Open SQL file… │ Refresh │ Export / Load database definition… |
  | Objects below a database | New query, Edit in new query tab, Select top 100 rows (tables, views and rows below them, D-056), Open SQL file… │ Refresh |
- **Show in new tab** is not in any context menu; `F3` opens the selected object in a
  new tab.
- The File menu keeps its layout; its items are disabled by the same rules.

### D-050 — DROP templates and more choice sources
- **Status:** Accepted (refines D-013, D-046)
- **Built-in DROP templates**, each in the folder of its CREATE template
  (`Tables/drop_table.pgtemplate`, ...): Drop schema, table, view, materialized view,
  function, procedure, index, trigger, sequence, type, domain, role.
- **Names:** built-in templates are named by their action — "Create table", "Drop table",
  ... (file names stay `table.pgtemplate`, `drop_table.pgtemplate`, ...).
- **Options** of every DROP template: **IF EXISTS** (checkbox, off) and **Dependent
  objects** RESTRICT / CASCADE (default RESTRICT, so nothing else is dropped by
  accident). DROP ROLE has no CASCADE.
- **Drop role:** optional **Reassign owned objects to** (a role); when set, the script
  starts with `REASSIGN OWNED BY … TO …;` and `DROP OWNED BY …;` and a comment that
  both act on the current database only.
- **Choice sources** (built into pgtc, so the tree selection pre-fills them, D-046):

  | Source | Lists | Value (`{{ v|ident }}`) |
  |--------|-------|-------------------------|
  | `schemas` | schemas | `"s"` |
  | `tables` | tables, partitioned and foreign tables | `"s"."t"` |
  | `views` | plain views | `"s"."v"` |
  | `materialized_views` | materialized views | `"s"."m"` |
  | `sequences` | sequences | `"s"."q"` |
  | `functions` | functions with their argument types | `"s"."f"(integer, text)` |
  | `procedures` | procedures with their argument types | `"s"."p"(integer)` |
  | `types` | data types for columns (common built-in types + own types) | `integer`, `"s"."t"` |
  | `user_types` | own types (enum, composite, range, base), no domains | `"s"."t"` |
  | `domains` | domains | `"s"."d"` |
  | `indexes` | indexes that can be dropped alone (not those of constraints or partitions) | `"s"."i"` |
  | `triggers` | triggers with their table (not partition clones) | `"trg" ON "s"."t"` |
  | `roles` | roles | `"r"` |

  System schemas and extension members are left out. Function values also have
  `args`, trigger values `table`.
- **Defaults from the tree** (D-046) for every source above: the selected object, or
  the nearest one above it (a selected trigger also gives its table).

### D-051 — Settings dialog contents, ASCII frames, notifications, deferred CI
- **Status:** Accepted (refines D-002, D-005, D-006, D-014, D-019, D-023, D-041)
- **Settings dialog** (one column, groups separated by an empty row, fits a 24-row
  terminal):
  - *Appearance:* Show icons (with the sample row, D-005), Colour scheme (combo,
    D-014), **ASCII frames**.
  - *Tree and connections:* Connect timeout (s) (D-035), Show system objects (D-044).
  - *Query tabs:* Rows per batch (`query.batch_rows`, default 1,000, D-019), Cancel
    wait (s) (`query.cancel_wait_secs`, default 5, D-041), Auto commit and Auto
    rollback on error **for new query tabs** (`query.auto_commit` /
    `query.auto_rollback`, D-019; open tabs keep their own toggles), Ask to save
    query tabs when closing (D-043).
  - *Clipboard:* System clipboard (`clipboard.system`) and OSC 52
    (`clipboard.osc52`) on/off (D-023), applied at once.
  - Numbers are checked on OK (batch 1–1,000,000, waits 1–3,600 s); a wrong value
    shows an error and keeps the dialog open.
  - Only in `settings.toml`: `editor.tab_width`, `editor.line_numbers` (D-038),
    `ui.tree_width` (D-045), export options (remembered by the export dialog, D-042).
- **ASCII frames** (`ui.ascii_frames`, default off, D-002): for terminals or fonts
  without box-drawing characters. Window (double) frames use `+` corners, `=`
  horizontal and `|` vertical lines; single frames (dialogs, menus, combo lists) and
  inner separators use `+`, `-`, `|` (`+---+`; also where inner lines join a single
  frame, `├ ┤ ┬ ┴`, through the theme's junction helper); grid and results-bar column lines
  `|`. Other symbols (`…`, `×`, `▼`, `►`, `↵`,
  arrows) are not box drawing and stay.
- **LISTEN / NOTIFY** (D-006): a notification received on a query tab's connection
  is added to that tab's Messages tab, like a notice:
  `NOTIFY:  channel "name" (pid 1234): payload` (payload left out when empty). It
  arrives whenever the server sends it, also between runs.
- **COPY** to / from the client (`COPY … FROM STDIN`, `TO STDOUT`) is *later*; the
  server's error is shown as usual.
- **CI is deferred** until the code is published and a CI system chosen (D-025): the
  CI parts of D-003, D-010, D-011, D-017 and D-022 wait for it.

### D-052 — Autocomplete in the SQL editor
- **Status:** Accepted (refines D-015, D-019)
- **Own completion inside pgtc**, no language server: an external server (e.g. the
  Postgres Language Server) would be a second program to install (D-022, also on the
  SSH host), brings C code (libpg_query) and needs its own database login (D-008,
  D-035). Completion uses pgtc's own lexer, the query tab's connection and versioned
  catalog SQL (D-010). It is a heuristic, not a parser: subqueries, CTE columns and
  unusual syntax may not resolve. A real syntax tree (tree-sitter, D-015) stays a
  possible later addition.
- **Only in query tabs** (not the read-only object view) and in the `sql` boxes of the
  developer template form of a query tab (D-046) — only on a connected tab, never
  inside strings, comments or quoted identifiers — but inside a function or `DO` body
  (D-015), which is SQL: there a statement is the text between `;` inside the body,
  and the body's `$tag$` ends a statement too (in PL/pgSQL `SELECT … INTO v` it still
  offers tables after `INTO`; a heuristic). In the form the list uses the query
  tab's connection and names, analyses the text of the box itself, is drawn over the
  form at the cursor, and `Esc` closes only the list (the form stays open).
- **Trigger:** `Ctrl+Space` and `Alt+/` (`complete` in `[editor]`; some terminals do
  not pass `Ctrl+Space` on), and **automatically** after typing `.` and after typing the
  **3rd letter** of a name. Opened automatically, the list appears only when it has
  entries; opened by key with nothing to offer, the status line says *No suggestions*.
- **Context** (from the lexer, in the statement around the cursor, between `;`):
  - after `FROM` / `JOIN` / `UPDATE` / `INTO` / `TABLE` (and after `,` in a `FROM`
    list) → schemas and tables, views, materialized views, foreign tables;
  - after `name.` → if `name` is a table or alias of the statement
    (`FROM orders o` → `o.`), its columns; if it is a schema, the tables, views,
    functions and types in it; otherwise the columns of a table `name` on the search path;
  - elsewhere → columns of the statement's tables, tables / views and functions on the
    search path, and keywords.
  - The typed prefix filters the list (case-insensitive, starts with).
- **List:** a popup at the cursor (below, above when there is no room), entries with
  their kind (`column`, `table`, `view`, `function`, `schema`, `type`, `keyword`, …),
  ordered columns → tables / views → functions → schemas → types → keywords, then by
  name; duplicates (overloaded functions) once. `Up` / `Down` / `PgUp` / `PgDn` move;
  `Enter` or `Tab` insert (while the list is open `Tab` does not indent); `Esc` closes
  at once (no `Esc` + digit wait while the list is open); a mouse click closes it;
  typing goes on into the editor and filters the list; `Backspace` refilters; any other
  key closes the list and does its normal job.
- **Inserted text:** replaces the typed prefix. **Keywords in UPPER CASE**; names quoted
  only when needed (`quote_ident`, `"Order Items"`).
- **Data:** names are read through the **query tab's own connection** (D-011): objects
  once per tab, when first needed, columns per table on demand (bind parameters);
  cached in the tab. Until they arrive the list shows *loading…* and updates when they
  do. The cache is dropped after the tab runs DDL (a `CREATE` / `ALTER` / `DROP`
  command tag) and on *Refresh* in the tree. System schemas (`pg_toast`, temp
  schemas) are left out; `pg_catalog` objects are offered like others.
  - **Not while a result is running or paused** (D-019): another request on the tab's
    connection would end that result, so no names are read then. The list offers what
    is already cached (and keywords); opened by key with names missing, the status
    line says that new names are read after the result ends.
- **Colours:** theme roles of the menu (popup) and the editor (D-014).

### D-053 — Keys that terminals may keep for themselves
- **Status:** Accepted (refines D-016, D-032, D-038, D-041)
- **Problem:** some default keys do not reach pgtc in some terminals (default settings;
  versions differ): `Ctrl+Tab` (most terminals); `Ctrl+Shift+letter` arrives as
  `Ctrl+letter` in legacy key encoding (so `Ctrl+Shift+Z` would undo); `Ctrl+Shift+Home`
  / `End` (Windows Terminal scrolls); `Shift+PgUp` / `PgDn` (GNOME, xterm, Konsole
  scroll); `Ctrl+PgUp` / `PgDn` (GNOME Terminal, xfce switch tabs); `Alt+Shift+arrows`
  (Windows Terminal panes); `Shift+F3` (may be read as a cursor-position report);
  `Ctrl+Backspace` (often the same code as `Backspace`); `Ctrl+Space` (macOS input
  source, Windows input methods); `Ctrl+A` (GNU screen); `F1` / `F10` (GNOME
  Terminal); all `Alt` keys in macOS Terminal / iTerm2 unless *Use Option as Meta key*
  is on.
- **Enhanced keyboard mode:** at start pgtc asks the terminal for unambiguous key
  reports (kitty keyboard protocol, crossterm `PushKeyboardEnhancementFlags` with
  *disambiguate escape codes*) when the terminal says it supports them (kitty, WezTerm,
  foot, Ghostty, recent Alacritty, iTerm2 when enabled); then `Ctrl+Shift+Z`,
  `Ctrl+Backspace`, `Ctrl+Tab`, `Shift+F3` arrive as pressed. It is switched off again on
  exit and on panic. Other terminals and Windows are unchanged.
- **Second keys** (configurable): select to start / end of text also `Alt+Shift+Home` /
  `Alt+Shift+End`; previous / next result set also `Alt+PgUp` / `Alt+PgDn`; find
  previous also `Alt+F3`. Existing alternatives: `F6` (switch panel), `Ctrl+Y` (redo),
  `Alt+/` (autocomplete), mouse drag (results size), `Esc` + digit (F-keys). *Select a
  page* and *delete word left* get no second key.
- **Query menu:** also *Next result set* / *Previous result set* (enabled with more than
  one result set).
- **Help → Keyboard shortcuts:** a section *If a key does not work* (terminals keep some
  keys; second keys and menus; macOS *Option as Meta*; GNU screen and `Ctrl+A`;
  `keymap.toml`).

### D-054 — psql-style commands (`F4`)
- **Status:** Accepted (refines D-016, D-019, D-046)
- **Command line:** `F4` (`Esc 4`; *Tools → psql command…*; `psql_command` in
  `[editor]` / `[results]`) opens a one-line dialog in a **connected query tab**.
  Commands are psql's backslash commands **without the backslash** — `F4` already
  opened the command line (`dt public.*` = psql's `\dt public.*`). pgtc never shows
  the backslash (list, `?`, result labels, messages); one typed out of psql habit is
  ignored.
  Under the input a **list of every command** (command, argument, description;
  pgtc's own commands after a separator), as tall as the screen allows, scrolled with
  `Up` / `Down` / `PgUp` / `PgDn` (also while typing in the input) and the mouse
  wheel. Choosing a row (keys or click) puts its command into the input (with a space
  when it takes an argument); typing a command moves the list to it — the list is never
  filtered. `Enter` / *Run* runs the input (an empty input: the selected command); a
  generate command without its table keeps the cursor in the input; `Esc` closes.
  No command history. In SQL scripts a `\` line is **not** a command (psql-style script
  lines are a possible later addition).
- **Commands (psql meanings, pgtc's own simpler queries):**
  - Lists — `l` databases, `dn` schemas, `dt` tables, `dv` views, `dm`
    materialized views, `di` indexes, `ds` sequences (letters combine: `dtv`),
    `df` functions, `dT` types, `dD` domains, `du` / `dg` roles, `dx`
    extensions, `db` tablespaces, `dp` access privileges; `d` without a name lists
    tables, views, materialized views, sequences and foreign tables.
  - Pattern (psql rules): `*` any text, `?` one character, `schema.name`, unquoted
    parts compared in lower case, `"..."` exact; passed as **bind parameters**. Without
    a pattern system schemas are left out; `S` (`dtS`) includes them; `+` adds columns
    (size, description, ...).
  - `d name` / `d+ name` — describe one table, view, materialized view, sequence or
    foreign table: result sets *Relation*, *Columns*, *Indexes*, *Constraints*,
    *Triggers*, *Definition* (views); empty sections are left out; not found → a
    message. A pattern (`*`, `?`) is refused with a hint to use `dt` & co.
  - `conninfo` — connection of the tab, in Messages. `?` — the commands, as a result
    set. Errors of these queries carry no editor position (no line jump).
  - **Not included** (psql's): `\c`, `\i`, `\o`, `\h`, `\x`, `\copy`, `\password`, `\set`,
    `\g…`, `\watch`, `\if` (no switching connections, no files, no SQL help).
- **Output:** a run of the tab like `F5` (results, Messages, time, status), one result
  set per list or section, labelled with the command or section
  (`1: dt public.* · 12 rows`). Read through the tab's own connection.
- **pgtc's generate commands** (listed separately by `?`):
  `gts`, `gti`, `gtu`, `gtd` *table* — SELECT / INSERT / UPDATE / DELETE for a
  table (view, foreign table), **inserted at the cursor** of the editor, never run.
  - Reading the table uses the tab's connection: while a result is paused (D-019) or
    an export runs, the command is refused with the same message as developer
    templates (D-046) — finish or close the result first. `F4` is not available
    while an export runs.
  - *Table:* `orders`, `sales.orders`, `"Order Items"`; without a schema the table on
    the search path (normally `public`). The statement always names it schema-qualified.
  - *Placeholders:* `[column]` (the plain column name) for every value — not valid
    SQL, so the statement cannot run until every value is filled in; each value line
    has a comment with the type and `NOT NULL`.
  - *SELECT:* every column, no `WHERE`.
  - *INSERT:* leaves out only the columns the database fills by itself: generated
    columns, identity columns and columns whose default takes a value from a sequence
    (`nextval(...)`, e.g. `serial`). Columns with any other default are listed; their
    comment also shows the default (`-- integer, NOT NULL, default 0`), so the user
    sees which lines can be deleted.
  - *UPDATE / DELETE:* `WHERE` on the **primary key** columns (`AND` for several);
    without one, on a unique constraint whose columns are all `NOT NULL` (comment says
    so); without either, `WHERE [condition]  -- no primary key: write a condition`.
    UPDATE sets every other column except generated and identity `ALWAYS` ones.

### D-055 — Find object in the tree (`Ctrl+F`)
- **Status:** Accepted (refines D-009, D-036)
- **Key:** `Ctrl+F` in `[tree]` (`find_object`; *View → Find object…*). In `[editor]`
  `Ctrl+F` stays *Find / replace* in the text (also in the read-only object view).
  The tree loads lazily, so the search asks the database; quick search (`Alt+S`,
  D-036) still moves among the visible rows.
- **Scope — everything below the selected row:**
  - server row → databases, roles and tablespaces (server connection) **and
    everything in every database of the server**; the *Databases* group → the
    databases and everything in them; *Roles* / *Tablespaces* → only their kind; a
    role or tablespace has nothing below it;
  - **all databases:** each database is searched through its tree connection, all at
    the same time; results appear as each database answers (*1 of 20 databases*).
    Databases that are not connected are opened, at most 8 at a time, already when
    the dialog opens; they are closed again when the dialog closes, except the one
    the tree opens into (OK), which stays connected like an expanded database
    (D-011). Databases already connected keep their connection. A database that
    cannot be opened (password needed, login failed, no permission, connections not
    allowed) is skipped without a login dialog and listed in the count line (*not
    searched: db3 (password needed)*). Templates follow *Show system objects*, as in
    the tree;
  - database → everything in that database (its tree connection; a database that
    is not connected yet is connected and the search waits);
  - a group below it (*Tables* of a schema, *Indexes* of a table, …) → only that kind
    there; an object → its sub-objects (schema → its objects, table → columns,
    indexes, constraints, triggers, rules, policies, partitions); an object without
    child groups (a column, …) has nothing below it — the status line says so.
- **Dialog:** input *Name*, combo **Match** — *Contains* (default), *Starts with*,
  *Ends with* — checkbox **Show columns** (default **off**: columns are searched only
  when checked), the selected row as *Below: …*, a count line, and the list of
  matches with a header and the columns **Database**, **Schema**, **Name**, **Type**
  — *Database* and *Schema* are empty for objects that are not in a database
  (databases, roles, tablespaces) or not in a schema (schemas, extensions, languages,
  …); *Name* is the object's own name, sub-objects with their table
  (`orders.customer_id`), functions with their arguments. The list follows the
  input after a 200 ms pause; `Up` / `Down` / `PgUp` / `PgDn` move in it while
  typing.
  - Case-insensitive, as chosen in *Match* (`%text%`, `text%`, `%text`); the text is a
    **bind parameter** (`ILIKE`, `%` `_` `\` escaped); changing *Match* or *Show
    columns* searches again. Order: by type in tree order (schemas, tables, partitions, views,
    materialized views, foreign tables, sequences, functions, procedures, trigger
    functions, aggregates, types, domains, collations, indexes, constraints, triggers,
    rules, policies, database-level objects; columns last; on a server row the
    databases, roles and tablespaces first), then by database, schema and name. At
    most 200 rows in all (*200+ objects — type more to narrow the search*).
  - Object kinds: those above plus extensions, event triggers, publications,
    languages, foreign data wrappers; not casts, operators, FTS objects, subscriptions.
  - System objects follow *Show system objects* (`Alt+.`, D-036), as the tree's
    queries do; with it off, the Catalogs (`pg_catalog`, `information_schema`) are not
    searched.
- **OK / Enter:** the dialog closes and the tree opens **down to exactly that
  object**: every level on the way is expanded and loaded when needed (database
  connection, groups), one after the other; then the cursor is on the object, scrolled
  into view, the tree has the focus (the default tab shows its definition). An object
  that is gone, or a level that cannot be loaded, ends it with a status-line message;
  the cursor stays on the deepest level reached.
- **SQL:** versioned catalog queries `sql/find/` (`database.sql`, `server.sql`, D-010):
  text columns type, schema, name, the **tree path**
  (`schemas:2200/tables:16384/columns:3` — one `folder:oid` step per level, columns
  by attnum, partitions below their parent table) and the type order (to merge the
  answers of several databases); the scope is a path prefix (bind parameter).

### D-056 — Select top 100 rows (`Ctrl+B`)
- **Status:** Accepted (refines D-009, D-019, D-049)
- **Key:** `Ctrl+B` in `[tree]` (`select_top_rows`); *View → Select top 100 rows*;
  context menu of the rows it applies to.
- **Rows:** a table, view, materialized view, foreign table or partition, **or any row
  below it** — its groups (*Columns*, *Indexes*, *Constraints*, *Triggers*, *Rules*,
  *Policies*, *Partitions*) and their objects. The nearest such relation at or above
  the row is used (a column → its table, a partition → the partition itself). Other
  rows: refused with a status-line hint; the menu item is disabled and left out of the
  context menu (D-049).
- **Query:** `SELECT * FROM schema.name LIMIT 100;` — names quoted with the `pgc-core`
  quoting. A partition in another schema than its parent table uses its own schema.
- **Tab:** a **new query tab bound to the same server and database** with its own
  connection (D-011), the query as unsaved text (closing asks to save, D-019), focus on
  the tab. The query is **executed as soon as the tab's connection is open** (as `F5`,
  with the tab's auto commit / auto rollback); a failed connection is reported in the
  tab as usual and nothing runs. Refused when the server is not connected.

### D-057 — Server dashboard tab
- **Status:** Accepted (like pgAdmin's dashboard; refines D-009, D-014, D-016, D-043)
- **Opening:** *Tools → Server dashboard* and the context menu of server rows,
  database rows and every row inside a database (`dashboard`, no default key). Needs a
  connected server. A server row (or a server-level group / object) shows the **whole
  server**; a database or anything inside it shows **that database** (every query is
  filtered by the database name, a bind parameter). Opening it again for the same
  server and database switches to the open tab.
- **A tab, not a dialog:** a dashboard tab (`Dashboard: server` /
  `Dashboard: server · database`) stays open next to the query tabs and keeps
  refreshing; it closes like a query tab (`Ctrl+W`, `×`) and with its server
  (disconnect / delete, D-043).
- **Connection:** its own connection (D-011) to the maintenance database (whole server)
  or to the database, with the server's password. The dashboard's own backend is left
  out of every count and list. A lost connection is shown in the tab; `F5` connects
  again.
- **Refresh:** every **2 s** by default; the next refresh is sent only after the answer
  of the previous one (no pile-up). *Pause / resume* stops and restarts it, *Refresh
  interval* cycles 1 → 2 → 5 → 10 → 30 s, `F5` refreshes at once. Rates are computed by
  pgtc from two samples (server clock), as pgAdmin does.
- **Graphs (top):** five boxes — *Sessions* (total / active / idle, client backends),
  *Transactions per second* (total / commits / rollbacks), *Tuples in* (inserts /
  updates / deletes per s), *Tuples out* (fetched / returned per s), *Block I/O* (reads
  / hits per s). Each box: title, a sparkline of its total over the last samples (newest
  right), current values as numbers. Five boxes in a row on wide windows, else 3 + 2.
- **Lists (bottom):** sub-tabs *Sessions · Locks · Prepared transactions ·
  Configuration* in the result grid (D-041). Sessions: PID, database, user, application,
  client, backend start, state, wait event, blocked by, transaction start, query start,
  running for, backend type, query; blocked sessions in the theme role
  `dashboard.blocked`. *Hide idle sessions* toggles sessions in state `idle`. The cursor
  stays on the same row (first column) across refreshes. `Enter` shows the whole row
  (with the full query text) in a scrolling window.
- **Cancel / terminate** (Sessions list): *Cancel query* (`pg_cancel_backend`) and
  *Terminate session* (`pg_terminate_backend`) of the row under the cursor, each after a
  Yes / No question naming the PID, user and database; the PID is a bind parameter. The
  result (or the server's error, e.g. missing permission) is reported in the status line
  / an error dialog.
- **SQL:** `sql/dashboard/` (versioned, D-010), adapted from pgAdmin's dashboard
  queries (copyright notice kept); all columns as text.
- **Keys** (`[dashboard]` context, D-016): `F5` refresh now, `Ctrl+P` pause / resume,
  `Ctrl+T` refresh interval, `Alt+I` hide idle sessions, `Ctrl+PgDn` / `Ctrl+PgUp`
  (`Alt+PgDn` / `Alt+PgUp`) next / previous list, `Alt+Shift+C` cancel query, `F8`
  terminate session, `Enter` show row, `Ctrl+C` copy, arrows / `PgUp` / `PgDn` / `Home`
  / `End` in the list. Commands also in the tab's context menu (`F2`).
- **Colours:** theme roles `dashboard.graph` (sparklines) and `dashboard.blocked`.
- **Later:** pgAdmin's system statistics (`system_stats` extension), replication, logs.

### D-026 — Scope of the first alpha
- **Status:** Accepted
- **All features described in this document go into the first alpha.**
  Items explicitly marked "later" / "possible later" (e.g. modifying objects from the
  object view, tree-sitter, PL/pgSQL mode returning result sets) and deferred decisions
  (D-024) are not part of it.
- Implementation order and implementation guidelines are kept in **`CLAUDE.md`**, not in
  this document.

---

## 3. Change log

| Date | Change |
|------|--------|
| 2026-11-08 | Document created; D-001..D-003 accepted. |
| 2026-11-08 | D-004 accepted: ratatui + crossterm. |
| 2026-11-08 | D-005 accepted: Nerd Fonts, two icon modes, `--install-font`. |
| 2026-11-08 | D-005 updated: icons off by default, toggle in Settings, bundle Symbols Nerd Font Mono + JetBrainsMono NF, "Install fonts" action in Settings. |
| 2026-11-08 | D-006 accepted: tokio-postgres on tokio runtime. |
| 2026-11-08 | D-007 accepted: core supporting crates; D-006 TLS set to rustls. |
| 2026-11-08 | D-008 accepted: password storage via .pgpass only; keyring removed from D-007. |
| 2026-11-08 | D-008 updated: "Save password" checkbox in login dialog writes to .pgpass. |
| 2026-11-08 | D-009 accepted: main window layout (menu, tree pane, object view). |
| 2026-11-08 | D-009 updated: tabs in right pane (default tab, "Show in new tab", "New query"). |
| 2026-11-08 | D-010 accepted: versioned SQL templates (minijinja), reuse pgAdmin SQL. |
| 2026-11-08 | D-010: minimum supported PostgreSQL version set to 11 (same as pgAdmin). |
| 2026-11-08 | D-010 updated: definitions via pg_get_* functions, pgAdmin reuse limited to read queries. D-011 accepted: connection design. |
| 2026-11-08 | D-012 accepted: database structure export, clone wizard, compare & sync wizard. |
| 2026-11-08 | D-012 updated: JSON format, shared structure model. |
| 2026-11-08 | D-012 updated: model save/load as portable file. |
| 2026-11-08 | D-012 updated: file extension `.pgdef`. |
| 2026-11-08 | D-013 accepted: developer templates with version fallback. |
| 2026-11-08 | D-013 updated: no template editor; templates directory scanned at runtime, hand-made templates supported. |
| 2026-11-08 | D-013 updated: template file extension `.pgtemplate`. |
| 2026-11-08 | D-013 updated: template directories, precedence, embedded fallback, `--install-templates`. |
| 2026-11-08 | D-014 accepted: menu bar and colour schemes. |
| 2026-11-08 | D-015 accepted: own SQL editor widget on ropey with own SQL lexer. |
| 2026-11-08 | D-013 updated: template file format (TOML header + Jinja body), parameter types. |
| 2026-11-08 | D-016 accepted: Midnight Commander style. D-009 F-key bar, D-014 F9 menu + Classic default scheme, D-015 extra mc copy/paste keys. |
| 2026-11-08 | D-016 updated: mc layout with tabs in the right panel. |
| 2026-11-08 | D-017 accepted: pgtc licensed under the PostgreSQL Licence. |
| 2026-11-08 | D-018 accepted: Cargo workspace with 7 crates incl. pgc-core. |
| 2026-11-08 | D-019 accepted: query tab execution (selection/whole script), cancel Alt+Shift+C, batched streaming of results. |
| 2026-11-08 | D-019 updated: query tab layout (SSMS model), result sets as numbered sub-tabs (first always shown), Messages tab. |
| 2026-11-08 | D-016 updated: context-dependent keys, keymap sections; D-019 Ctrl+R toggles results pane. |
| 2026-11-08 | D-019 updated: transactions handled as in pgAdmin. |
| 2026-11-08 | D-020 accepted: query editor modes SQL (default) and PL/pgSQL (DO block). |
| 2026-11-08 | D-021 accepted: result export to CSV, XML, JSON. |
| 2026-11-08 | D-021 updated: CSV separator + BOM options, XML control characters, escaping rule. |
| 2026-11-08 | D-022 accepted: source-only distribution. D-023 accepted: clipboard (arboard + OSC 52). D-007: winreg. |
| 2026-11-08 | D-024 testing deferred; D-025 source local until first alpha, GitHub later. |
| 2026-11-08 | D-026 accepted: all features in first alpha; implementation order in CLAUDE.md. |
| 2026-11-08 | D-027 accepted: toolchain pinned to 1.98.1, edition 2024. D-028 accepted: flat user directories. D-029 accepted: daily rolling log, keep 7, `pgtc_LOG`. D-025: local git repository. |
| 2026-11-08 | D-030 accepted: short name `pgtc` for executable, crate, folders, `PGTC_LOG`, log file; long name "Postgres terminal commander". D-025 path is `F:\razno\rust\pgtc`. |
| 2026-11-08 | D-031 accepted: psql-compatible connection arguments (parsed in Phase 0, used from Phase 2). D-017: copyright holder Darko Prenosil. |
| 2026-11-08 | D-032 accepted: Tab owned by dialogs/editor, tab keys Alt+Right/Alt+Left/Ctrl+W, mc-style Esc + digit with 1 s timeout, basic mouse clicks from Phase 1. |
| 2026-11-08 | D-031 updated: PG* environment variables, connection URIs, startup connection saved automatically. D-033 accepted: libpq SSL modes with rustls/ring, OS trust store. D-034 accepted: refuse < 11, verified up to 18. D-035 accepted: edit/delete connections via context menu, password in connection + login dialogs, connect timeout 10 s setting. |
| 2026-11-08 | D-036 accepted: object tree types (core + common), system objects hidden by default (saved toggle), mc-style quick search, Ctrl+R refreshes the selected subtree, group counts. D-035: login dialog shows server, database and user name. |
| 2026-11-08 | D-036 updated: View → Show system objects shows its state as `[x]` / `[ ]`. |
| 2026-11-08 | D-037 accepted: object view like pgAdmin's SQL tab (with table dependents), info text for servers and groups, [editor] keymap context, 200 ms delay before loading. |
| 2026-11-08 | D-014 updated: colour scheme chosen with a combo box (drop-down) in the Settings dialog. |
| 2026-11-08 | D-035 updated: SSL mode in the connection dialog is a combo box too. |
| 2026-11-08 | D-038 accepted: editor leaves with F6 / Ctrl+Tab, real tabs with block indent / unindent, line numbers, mc-style find / replace with F3 / Shift+F3, read-only editor for the object view, editor colour roles, lexer checkpoints every 512 lines. |
| 2026-11-08 | D-039 accepted: `directories` replaced by own path code in `pgc-core` (it pulled in MPL-2.0 `option-ext`); D-007, D-013, D-017, D-028 updated. |
| 2026-11-08 | D-014 updated: built-in colour schemes Tokyo Night and Dracula (24-bit colours). |
| 2026-11-08 | D-040 accepted: vendored, patched tokio-postgres (command tag, transaction status, column type OIDs). D-041 accepted: Query menu, query tab binding, F6 focus cycle, query keys, result grid look, mc-style file dialog, cancel wait setting (default 5 s). |
| 2026-11-08 | D-014 updated: readability rules for colour schemes (contrast, visible highlights, underlined hotkeys), enforced by a test; all built-in schemes fixed; text cursor also drawn in reverse video. |
| 2026-11-08 | D-019 updated: split between editor and results resizable with the mouse; closing a query tab with unsaved text asks to save it. |
| 2026-11-08 | D-019 updated: quitting asks to save each query tab with unsaved text. |
| 2026-11-08 | D-042 accepted: result export details (menu / context menu, dialog, typed JSON, XML layout, cancel deletes the partial file, grid shows "n of total rows"); D-021 updated. |
| 2026-11-08 | D-043 accepted: per-tab status bar (window status line only for messages); disconnecting a server closes its query and object tabs after save / transaction questions; setting `query.ask_save` (default on). D-019 updated. |
| 2026-11-08 | D-043 updated: `×` close mark on tabs. |
| 2026-11-09 | D-044 accepted: grid column lines, `Ctrl+N` new query, Show system objects in Settings (View menu item and `Alt+.` removed). D-043 updated: tab status bar on the bottom row of the tab. |
| 2026-11-09 | D-045 accepted: tree pane width changed by dragging its border with the mouse, remembered in the settings. |
| 2026-11-09 | D-046 accepted: developer templates only from a connected query tab; folder-tree picker with search; choice lists through the tab's connection; defaults from the tree selection of the same database; live read-only preview; result inserted at the cursor. D-013 updated. |
| 2026-11-09 | D-047 accepted: `--setup` runs every installation step (fonts, templates, later more), menu Settings → Run setup; bundled font files (Nerd Fonts v3.5.1, JetBrains Mono in the Mono variant, four styles); `windows-sys` instead of `winreg` (D-007 updated). |
| 2026-11-09 | D-047 and D-005 updated: fonts are no longer bundled or installed (`--install-font`, Settings → Install fonts, `windows-sys` removed); icons checkbox "Show icons (needs a Nerd Font)" with a sample row of icons; `--setup` currently installs templates only. |
| 2026-11-09 | D-048 accepted: export / load need a database selected in the tree; export everything, no selection dialogs; load compares the file with the live database and generates only the needed statements (no drops of objects only in the database, no CREATE DATABASE); no owners, privileges or header in the file; dependency-ordered script; Tools → Create database from definition and Compare & sync removed. D-012 updated. |
| 2026-11-09 | D-048 updated: objects that cannot be altered are replaced; a table is re-created only when its partitioning changed, after copying it with its data into `_TableCopy_<name>`; the data is loaded back and the copies are listed as commented-out `DROP TABLE` lines at the end of the script. |
| 2026-11-09 | D-048 and D-037 updated: Export / Load database definition also in the context menu of a database in the tree. |
| 2026-11-09 | D-049 accepted: New query / Open SQL file only on a database or below it (no maintenance-database fallback), Save SQL file only for query tabs, Open in query tab (`F4`) removed, context menus by row kind (no Show in new tab / New query on servers). D-009, D-016, D-035, D-037, D-041 updated. |
| 2026-11-09 | D-049 updated: Show in new tab removed from all context menus (`F3` only). D-009 updated. |
| 2026-11-09 | D-050 accepted: built-in DROP templates (IF EXISTS, RESTRICT / CASCADE, Drop role with optional REASSIGN / DROP OWNED); new choice sources `materialized_views`, `procedures`, `user_types`, `domains`, `indexes`, `triggers`; `views` lists plain views only, `functions` carries the argument types; tree defaults for all sources. D-013, D-046 updated. |
| 2026-11-09 | D-050: built-in CREATE templates renamed to "Create …" to match "Drop …". |
| 2026-11-09 | D-013 updated: installing templates also creates the empty user templates directory. |
| 2026-11-09 | D-014: one combo box behaviour for dialogs and the template form (`Enter` opens the list, framed list, mouse and wheel, typing filters long lists). |
| 2026-11-09 | D-032 updated: `Tab` only in the editor and dialogs; `Ctrl+Tab` / `F6` switch panels everywhere (tree → editor → results). D-016 updated. |
| 2026-11-09 | D-014: frame style like mc — double-line window frames, single-line separators inside windows. D-019: results bar is a separator line in the frame. D-043: tab status written into the bottom frame line. |
| 2026-11-09 | D-014 frame style revised to the owner's mockup: tab strip inside the tab window; tab strip, results labels and tab status are rows of their own between single separator lines; no frame title for query tabs. D-019, D-043 updated. |
| 2026-11-09 | D-014: the window status line uses the window colour (`core.normal`) instead of the status colour. |
| 2026-11-10 | D-036: Catalogs group (pg_catalog, information_schema) under each database, always shown, with schema groups; database groups in alphabetical order. |
| 2026-11-10 | D-043: the tab status keeps *Execution finished in …* / *Execution failed after …* after a run instead of going empty. |
| 2026-11-10 | D-013 / D-046: Tools menu item renamed to *Generate SQL from developer template…*. |
| 2026-11-10 | D-043: object tabs show `server · user@database · read only` in the tab status; D-014: no tab has a title in the top frame line. |
| 2026-11-10 | D-043: the window status line also shows how a run ended instead of going empty. |
| 2026-11-10 | D-009 / D-016: the F-key bar becomes a key-hint bar — the most important keys of the focused frame (any key), keys from the active keymap, unavailable commands hidden. |
| 2026-11-10 | D-014 / D-016 / D-017: Help menu — Keyboard shortcuts window from the active keymap (`F1` opens the focused frame's section), Third-party licences window (pgAdmin + Rust crates via `cargo-about`), About pgtc without links. |
| 2026-11-10 | D-017: BSL-1.0 and Unicode-3.0 added to the accepted dependency licences. |
| 2026-11-10 | Design review: key-hint bar wording everywhere (D-009, D-014, D-016, D-018), Query menu in D-014, `F8` row of D-016 matches D-035, Help-window hints in D-016, D-017 dependency list points to `rust-crates.txt`. |
| 2026-11-10 | D-051 accepted: Settings dialog contents, ASCII frames, NOTIFY in Messages, COPY to/from the client later, CI deferred (D-003, D-010, D-011, D-017, D-022 updated). |
| 2026-11-10 | D-052 accepted: autocomplete in the SQL editor (own completion, no language server; `Ctrl+Space` / `Alt+/` and automatic after `.` and 3 letters; keywords in upper case; names from the query tab's connection). |
| 2026-11-10 | D-053 accepted: keys terminals may keep — enhanced keyboard mode (kitty protocol) where supported, second keys (`Alt+Shift+Home/End`, `Alt+PgUp/PgDn`, `Alt+F3`), next / previous result set in the Query menu, Help note. D-014, D-041 updated. |
| 2026-11-10 | D-054 accepted: psql-style commands on `F4` (lists, `\d`, `\conninfo`, `\?`; own simpler queries) and pgtc's generate commands `\gts` / `\gti` / `\gtu` / `\gtd` with `[column]` placeholders and primary-key `WHERE`. |
| 2026-11-10 | Object tree expand markers changed from `>` / `v` to `►` / `▼` (D-009). |
| 2026-11-10 | D-043 updated: *Delete connection* closes the server's tabs like *Disconnect* (same questions first). D-048 updated: partitions of a re-created table are created again and get their rows back through the parent, also when their own partitioning changed. |
| 2026-11-10 | D-052, D-054 updated: nothing is looked up on a query tab's connection while its result is running or paused — autocomplete offers cached names only, `\gts` / `\gti` / `\gtu` / `\gtd` are refused (as developer templates are); `F4` is off while an export runs. |
| 2026-11-11 | D-054 updated: the generated INSERT leaves out only generated, identity and sequence-filled (`nextval`) columns; columns with another default are listed, with the default in their comment. |
| 2026-11-12 | D-054 updated: `F4` commands are typed and shown without psql's backslash (`dt`, `d name`, `?`, `gts`, …) — the list, `?`, result labels and messages never show it; a typed backslash is ignored. |
| 2026-11-12 | D-046 updated: the developer template window is drawn like the other dialogs (margin, single frame, shadow) with lines joined to the frame between form and preview and above the help / button rows; the `sql` box has its own frame. D-051: junctions of inner lines with single frames (`├ ┤ ┬ ┴`) follow ASCII frames. |
| 2026-11-12 | D-016 updated: `Ctrl+Enter` (`dialog_ok`) presses the default button of a dialog from any field, also in the template form; only where the terminal reports it (Windows, kitty keyboard protocol), no second key by default. |
| 2026-11-12 | D-013 simplified: the user's templates folder is the only one pgtc reads — no system-wide folder, no merging by name, the embedded templates are only the source for installing (none installed → the picker points to Settings → Install templates). Installing into an existing folder asks first; Yes overwrites the templates that come with pgtc, No writes nothing; pgtc never deletes template files. Files left in the old system folder (e.g. `%ProgramData%\pgtc\developer_templates`) are ignored. D-022, D-028, D-046, D-047 updated. |
| 2026-11-12 | D-020 updated: `Ctrl+P` (`toggle_plpgsql`, editor and results) switches PL/pgSQL mode of the active query tab. |
| 2026-11-12 | D-014, D-054 updated: *psql command…* (`F4`) moved from the Query menu to Tools, under *Generate SQL from developer template…* (`F7`). |
| 2026-11-12 | D-013, D-046 updated: `fill = true` on `sql` parameters — the editor box takes the free rows of the form (several share them; at least 5 rows); used by the built-in function, procedure, trigger and view templates. D-052 updated: autocomplete also in the template form's `sql` boxes, with the query tab's connection and names; `Esc` closes only the list. |
| 2026-11-12 | D-015 updated: function and `DO` bodies (a dollar quote after `AS` / `DO`) are lexed as SQL — highlighted, the first closing tag of an open body ends it, nested bodies and other-tag dollar strings inside; other dollar quotes stay strings; more PL/pgSQL keywords. D-052 updated: autocomplete works inside such bodies (statements between `;` and the body tags). |
| 2026-11-12 | D-049 updated: *Edit in new query tab* (`Ctrl+E` in the editor and tree, View menu, context menu) puts the definition of the read-only view into a new query tab bound to the same server and database, as unsaved text; refused while loading, without a definition or database, or when the server is not connected. D-014 updated (View menu). |
| 2026-11-12 | D-019 updated: a run that ends without any result set switches to the Messages tab (an empty `SELECT` is a result set and stays on Results). |
| 2026-11-12 | D-055 accepted: Find object (`Ctrl+F` in the tree) — searches everything below the selected row through the tree connection (server rows: databases, roles, tablespaces), *Match* combo (Contains — default, Starts with, Ends with), "Show columns" checkbox (default off), matches with name and type, OK opens the tree level by level down to exactly that object. D-014, D-016 updated. |
| 2026-11-12 | D-055 updated: a server row (and the Databases group) also searches everything in every database of the server — databases not connected are opened (8 at a time) and closed with the dialog, failures listed without a login dialog; the list has the columns Database, Schema, Name, Type. |
| 2026-11-12 | D-056 accepted: Select top 100 rows (`Ctrl+B` in the tree, View menu, context menu) on a table, view, materialized view, foreign table or partition or any row below it — `SELECT * … LIMIT 100` in a new query tab on the same server and database, executed once the tab is connected. D-014, D-016, D-049 updated. |
| 2026-11-12 | D-037 updated: a connected server row also lists all configuration settings below the version (value, source, file and line, restart needed); read once per connection, `Ctrl+R` on the server row reads them again. |
| 2026-11-12 | D-057 accepted: server dashboard as a tab with its own connection — graphs (sessions, TPS, tuples in / out, block I/O), Sessions / Locks / Prepared transactions / Configuration lists, cancel query and terminate session after a question, 2 s refresh with pause and interval, `[dashboard]` keys. D-014 updated (Tools menu). |
