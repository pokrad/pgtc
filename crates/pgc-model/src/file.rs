//! The `.pgdef` file (D-012): pretty-printed JSON of a normalised [`Database`].

use std::path::Path;

use thiserror::Error;

use crate::Database;

#[derive(Debug, Error)]
pub enum FileError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid definition file: {0}")]
    Format(#[from] serde_json::Error),
}

/// The file text: normalised, pretty-printed, ending with a newline.
pub fn to_text(db: &Database) -> String {
    let mut db = db.clone();
    db.normalize();
    let mut text = serde_json::to_string_pretty(&db).expect("the model always serialises");
    text.push('\n');
    text
}

pub fn from_text(text: &str) -> Result<Database, FileError> {
    let mut db: Database = serde_json::from_str(text)?;
    db.normalize();
    Ok(db)
}

pub fn save(db: &Database, path: &Path) -> Result<(), FileError> {
    std::fs::write(path, to_text(db))?;
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
                }],
                ..Default::default()
            }],
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
    }
}
