//! The `.pgdef` file (D-012): pretty-printed JSON of a normalised [`Database`], with
//! the format number in front.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::Database;

/// The format this pgtc writes, and the newest it reads (D-012). It goes up only when
/// an older pgtc could no longer read a newer file correctly.
pub const FORMAT: u32 = 1;

#[derive(Debug, Error)]
pub enum FileError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid definition file: {0}")]
    Format(#[from] serde_json::Error),
    #[error("This definition file was written by a newer pgtc (format {0}).")]
    Newer(u32),
    #[error("not a definition file: it has \"{0}\", and nothing a definition file has")]
    Foreign(String),
}

/// The keys at the top of a definition file.
const TOP_KEYS: [&str; 3] = ["format", "extensions", "schemas"];

/// The file as written: the format number, then the database.
#[derive(Serialize)]
struct Output<'a> {
    format: u32,
    #[serde(flatten)]
    database: &'a Database,
}

/// The file as read; one without a format number is format 1.
#[derive(Deserialize)]
struct Input {
    #[serde(default = "first_format")]
    format: u32,
    #[serde(flatten)]
    database: Database,
}

fn first_format() -> u32 {
    1
}

/// The file text: normalised, pretty-printed, ending with a newline.
pub fn to_text(db: &Database) -> String {
    let mut db = db.clone();
    // of the roles of the cluster, those the database needs (D-048)
    db.keep_needed_roles();
    db.normalize();
    let file = Output {
        format: FORMAT,
        database: &db,
    };
    let mut text = serde_json::to_string_pretty(&file).expect("the model always serialises");
    text.push('\n');
    text
}

pub fn from_text(text: &str) -> Result<Database, FileError> {
    let file: Input = serde_json::from_str(text)?;
    if file.format > FORMAT {
        return Err(FileError::Newer(file.format));
    }
    // every part of a definition may be missing, so any JSON object would read as an
    // empty database (a package.json picked by mistake: "nothing differs"). A file
    // with keys, none of them a definition file's, is some other file. Unknown keys
    // beside known ones are ignored, as D-012 says (a newer pgtc may add some).
    let top: std::collections::BTreeMap<String, serde::de::IgnoredAny> =
        serde_json::from_str(text)?;
    if !top.keys().any(|k| TOP_KEYS.contains(&k.as_str()))
        && let Some(key) = top.into_keys().next()
    {
        return Err(FileError::Foreign(key));
    }
    let mut db = file.database;
    db.normalize();
    Ok(db)
}

pub fn save(db: &Database, path: &Path) -> Result<(), FileError> {
    pgc_core::fsutil::atomic_write(
        path,
        to_text(db).as_bytes(),
        pgc_core::fsutil::Access::Default,
    )?;
    Ok(())
}

pub fn load(path: &Path) -> Result<Database, FileError> {
    from_text(&std::fs::read_to_string(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    fn sample() -> Database {
        Database {
            extensions: vec![Extension {
                name: "pgcrypto".into(),
                schema: "public".into(),
            }],
            schemas: vec![Schema {
                name: "public".into(),
                tables: vec![
                    Table {
                        name: "b".into(),
                        columns: vec![
                            Column {
                                name: "z".into(),
                                data_type: "integer".into(),
                                not_null: true,
                                ..Default::default()
                            },
                            Column {
                                name: "a".into(),
                                data_type: "text".into(),
                                ..Default::default()
                            },
                        ],
                        constraints: vec![Constraint {
                            name: "b_pkey".into(),
                            kind: ConstraintKind::PrimaryKey,
                            definition: "PRIMARY KEY (z)".into(),
                            references: None,
                            comment: None,
                        }],
                        ..Default::default()
                    },
                    Table {
                        name: "a".into(),
                        ..Default::default()
                    },
                ],
                types: vec![Type {
                    name: "mood".into(),
                    kind: TypeKind::Enum {
                        labels: vec!["sad".into(), "happy".into()],
                    },
                    comment: None,
                    depends_on: vec![],
                    access: None,
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn round_trip_and_stable_output() {
        let db = sample();
        let text = to_text(&db);
        let back = from_text(&text).unwrap();
        assert_eq!(to_text(&back), text, "same structure, same file");
        let mut sorted = db.clone();
        sorted.normalize();
        assert_eq!(back, sorted);
        // tables sorted by name, columns and enum labels keep their order
        let t = &back.schemas[0].tables;
        assert_eq!((t[0].name.as_str(), t[1].name.as_str()), ("a", "b"));
        assert_eq!(t[1].columns[0].name, "z");
        assert!(text.contains("\"kind\": \"enum\""), "{text}");
        // empty and false values are left out
        assert!(!text.contains("unlogged"));
        assert!(!text.contains("\"comment\""));
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn bad_files_are_errors() {
        assert!(matches!(from_text("{"), Err(FileError::Format(_))));
        assert!(from_text("{}").unwrap().schemas.is_empty());
        // some other JSON file
        let e = from_text(r#"{"name": "app", "version": "1.0.0"}"#).unwrap_err();
        assert!(matches!(&e, FileError::Foreign(k) if k == "name"), "{e}");
        // an enum without labels, a composite type without attributes
        let text = r#"{"schemas": [{"name": "s", "types": [
            {"name": "e", "kind": "enum"}, {"name": "c", "kind": "composite"}]}]}"#;
        assert_eq!(from_text(text).unwrap().schemas[0].types.len(), 2);
    }

    #[test]
    fn format_number() {
        let db = sample();
        let text = to_text(&db);
        assert!(text.starts_with("{\n  \"format\": 1,\n"), "{text}");
        // a file from before the number existed is format 1
        let old = text.replacen("  \"format\": 1,\n", "", 1);
        assert_eq!(from_text(&old).unwrap(), from_text(&text).unwrap());
        // a newer format is refused, with its number
        let newer = text.replacen("\"format\": 1", "\"format\": 2", 1);
        let e = from_text(&newer).unwrap_err();
        assert!(matches!(e, FileError::Newer(2)), "{e}");
        assert_eq!(
            e.to_string(),
            "This definition file was written by a newer pgtc (format 2)."
        );
        // a field this pgtc does not know is ignored
        let extra = text.replacen("\"format\": 1", "\"format\": 1,\n  \"later\": true", 1);
        assert_eq!(from_text(&extra).unwrap(), from_text(&text).unwrap());
        // an empty database: only the number
        assert_eq!(to_text(&Database::default()), "{\n  \"format\": 1\n}\n");
    }
}
