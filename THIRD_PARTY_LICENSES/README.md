# Third-party licences

Licence notices of third-party material shipped with pgtc (D-017). They are also
shown in the application under Help → Third-party licences (D-014).

| Material | Used for | Licence | File |
|----------|----------|---------|------|
| pgAdmin 4 SQL templates | catalog queries (D-010) | PostgreSQL Licence | `pgadmin4-LICENSE.txt` |
| Rust crates in the program | libraries | MIT, Apache-2.0, ISC, BSD, Zlib, BSL-1.0, Unicode-3.0 | `rust-crates.txt` |

`rust-crates.txt` is generated with [cargo-about](https://github.com/EmbarkStudios/cargo-about)
(a build-time tool, not a dependency). Regenerate it after dependency changes, from the
workspace root:

```
cargo install cargo-about --locked --features cli
cargo about generate -c THIRD_PARTY_LICENSES/about.toml THIRD_PARTY_LICENSES/about.hbs -o THIRD_PARTY_LICENSES/rust-crates.txt
```

`about.toml` accepts only the permissive licences of D-017; generation fails on any other.
