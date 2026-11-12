//! `include_dir!` embeds `sql/` (D-010) and `templates/` (D-013) but does not tell
//! Cargo about the files, so a new or deleted file would not rebuild this crate. Cargo
//! scans a watched directory recursively for changes.

fn main() {
    println!("cargo::rerun-if-changed=../../sql");
    println!("cargo::rerun-if-changed=../../templates");
}
