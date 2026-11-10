//! `.pgpass` reading and writing (D-008), same rules as libpq:
//! `hostname:port:database:username:password`, `*` wildcard, `\:` and `\\` escapes,
//! `#` comments, first matching line wins. On Unix the file is ignored if its
//! permissions are wider than `0600`, and it is created with `0600`.

use std::path::Path;

use crate::DbError;

/// The four key fields of an entry.
#[derive(Debug, Clone, Copy)]
pub struct PgPassKey<'a> {
    pub host: &'a str,
    pub port: u16,
    pub database: &'a str,
    pub username: &'a str,
}

/// Splits a line into its fields, removing escapes. The password field (5th) takes
/// the rest of the line. Returns `None` for comments, blank lines and short lines.
fn parse_line(line: &str) -> Option<[String; 5]> {
    let line = line.trim_end_matches('\r');
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut fields: Vec<String> = vec![String::new()];
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    fields.last_mut()?.push(n);
                }
            }
            ':' if fields.len() < 5 => fields.push(String::new()),
            c => fields.last_mut()?.push(c),
        }
    }
    fields.try_into().ok()
}

fn field_matches(pattern: &str, value: &str) -> bool {
    pattern == "*" || pattern == value
}

/// Password for `key` from `.pgpass` text, if a line matches.
pub fn lookup(text: &str, key: PgPassKey<'_>) -> Option<String> {
    let port = key.port.to_string();
    text.lines()
        .filter_map(parse_line)
        .find_map(|[h, p, d, u, pw]| {
            (field_matches(&h, key.host)
                && field_matches(&p, &port)
                && field_matches(&d, key.database)
                && field_matches(&u, key.username))
            .then_some(pw)
        })
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace(':', "\\:")
}

/// Returns `text` with the entry for `key` set to `password`: a line with exactly the
/// same host, port, database and user is replaced, otherwise a line is appended.
/// Other lines and comments are kept.
pub fn upsert(text: &str, key: PgPassKey<'_>, password: &str) -> String {
    let port = key.port.to_string();
    let new_line = format!(
        "{}:{}:{}:{}:{}",
        escape(key.host),
        port,
        escape(key.database),
        escape(key.username),
        escape(password)
    );
    let mut replaced = false;
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            if !replaced
                && let Some([h, p, d, u, _]) = parse_line(line)
                && h == key.host
                && p == port
                && d == key.database
                && u == key.username
            {
                replaced = true;
                return new_line.clone();
            }
            line.to_owned()
        })
        .collect();
    if !replaced {
        lines.push(new_line);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Reads the password for `key` from the `.pgpass` file at `path`.
/// A missing file, or (on Unix) one with too wide permissions, gives `None`.
pub fn read_password(path: &Path, key: PgPassKey<'_>) -> Option<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!("cannot read {}: {e}", path.display());
            }
            return None;
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path)
            && meta.permissions().mode() & 0o077 != 0
        {
            tracing::warn!(
                "{} has group or world access; ignored (permissions should be 0600)",
                path.display()
            );
            return None;
        }
    }
    lookup(&text, key)
}

/// Saves `password` for `key` into the `.pgpass` file at `path` (D-008).
pub fn save_password(path: &Path, key: PgPassKey<'_>, password: &str) -> Result<(), DbError> {
    let err = |e: std::io::Error| DbError::Other(format!("{}: {e}", path.display()));
    let old = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(err(e)),
    };
    let new = upsert(&old, key, password);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(path).map_err(err)?;
    file.write_all(new.as_bytes()).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key<'a>(host: &'a str, port: u16, db: &'a str, user: &'a str) -> PgPassKey<'a> {
        PgPassKey {
            host,
            port,
            database: db,
            username: user,
        }
    }

    const SAMPLE: &str = "# comment\n\
        db1:5432:app:alice:secret\n\
        *:5433:*:bob:pa\\:ss\\\\word\n\
        localhost:*:*:*:fallback\r\n";

    #[test]
    fn lookup_rules() {
        assert_eq!(
            lookup(SAMPLE, key("db1", 5432, "app", "alice")).as_deref(),
            Some("secret")
        );
        assert_eq!(lookup(SAMPLE, key("db1", 5432, "other", "alice")), None);
        assert_eq!(
            lookup(SAMPLE, key("any", 5433, "x", "bob")).as_deref(),
            Some("pa:ss\\word")
        );
        assert_eq!(
            lookup(SAMPLE, key("localhost", 1, "x", "y")).as_deref(),
            Some("fallback")
        );
        assert_eq!(lookup("short:line\n", key("short", 1, "x", "y")), None);
    }

    #[test]
    fn password_may_contain_unescaped_colons() {
        assert_eq!(
            lookup("h:1:d:u:a:b\n", key("h", 1, "d", "u")).as_deref(),
            Some("a:b")
        );
    }

    #[test]
    fn upsert_replaces_or_appends() {
        let t = upsert(SAMPLE, key("db1", 5432, "app", "alice"), "new");
        assert!(t.contains("db1:5432:app:alice:new\n"));
        assert!(!t.contains("secret"));
        assert!(t.starts_with("# comment\n"));
        let t = upsert(&t, key("h:x", 5432, "d", "u"), "p:w");
        assert!(t.ends_with("h\\:x:5432:d:u:p\\:w\n"));
        assert_eq!(
            lookup(&t, key("h:x", 5432, "d", "u")).as_deref(),
            Some("p:w")
        );
        // A wildcard line is not "the same" entry: appended, not replaced.
        let t = upsert("*:5432:d:u:old\n", key("h", 5432, "d", "u"), "p");
        assert_eq!(t, "*:5432:d:u:old\nh:5432:d:u:p\n");
    }

    #[test]
    fn save_and_read_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("pgpass.conf");
        let k = key("h", 5432, "postgres", "me");
        assert_eq!(read_password(&path, k), None);
        save_password(&path, k, "one").unwrap();
        save_password(&path, k, "two").unwrap();
        assert_eq!(read_password(&path, k).as_deref(), Some("two"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
