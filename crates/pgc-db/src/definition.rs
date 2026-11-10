//! Object definitions (D-037): the text shown in the object view, assembled from the
//! row returned by a `definition.sql` template (`kind`, `name`, `drop`, `body`).

/// pgAdmin-style layout:
///
/// ```text
/// -- Table: public.t
///
/// -- DROP TABLE IF EXISTS public.t;
///
/// CREATE TABLE ...
/// ```
pub fn format(kind: &str, name: &str, drop: Option<&str>, body: &str) -> String {
    let mut text = format!("-- {kind}: {name}\n\n");
    if let Some(drop) = drop.filter(|d| !d.is_empty()) {
        text.push_str(&format!("-- {drop}\n\n"));
    }
    text.push_str(body.trim_end());
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout() {
        assert_eq!(
            format(
                "Table",
                "public.t",
                Some("DROP TABLE IF EXISTS public.t;"),
                "CREATE TABLE t ();\n"
            ),
            "-- Table: public.t\n\n-- DROP TABLE IF EXISTS public.t;\n\nCREATE TABLE t ();\n"
        );
        assert_eq!(format("Rule", "r", None, "x"), "-- Rule: r\n\nx\n");
    }
}
