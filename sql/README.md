# Versioned catalog SQL (D-010)

`sql/<object>/<version folder>/<file>.sql` — embedded into the binary by `pgc-sql`.

- `default/` works for every supported server (PostgreSQL 11+, D-034).
- `NN_plus/` holds only files that differ from major version `NN` on; for a server the
  newest folder not newer than the server that contains the file is used.
- Templates are minijinja (Jinja2). Context: `server_version`, `show_system`;
  functions `has("Feature")`, `_()`; filters `qtIdent`, `qtLiteral`, `qtTypeIdent`,
  `hasAny`, `ident`.
- Values (OIDs, names typed by the user) are bind parameters (`$1`), never template text.

Many queries are adapted from pgAdmin 4 — Copyright (C) 2013 - 2026, The pgAdmin
Development Team, released under the PostgreSQL Licence; see
`THIRD_PARTY_LICENSES/pgadmin4-LICENSE.txt`. Each adapted file names its source.

## Tree queries (`nodes.sql`, D-036)

Columns: `oid` (oid), `name` (text), optional `label` (text, shown instead of the name)
and `partitioned` (bool, tables). `$1` is the OID of the parent object (schema or table)
where the file header says so.

## Structure model queries (`model/`, D-048)

Read the structure model for definition export / load. Every query returns `schema`,
`kind` and `data` (JSON in the shape of the `pgc-model` structs; `json_strip_nulls`
leaves missing values out). `depends.sql` returns the dependencies between model
objects. They run in one read-only transaction with `search_path = ''`, so every name
in the output is schema-qualified. Shared parts are in `macros/model.sql`.

## Autocomplete queries (`completion/`, D-052)

Read through a query tab's own connection. `objects.sql` returns `schema` (NULL for
schemas), `name`, `kind` and `visible` (on the search path) for schemas, relations,
functions and types; `columns.sql` the columns of one table (`$1` schema or NULL,
`$2` table name, both bind parameters).
