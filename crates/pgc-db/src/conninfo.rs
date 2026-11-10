//! Command-line connection parameters (D-031): options, `PG*` environment variables,
//! `postgresql://` URIs. Precedence: URI > options > environment > defaults.

use pgc_core::settings::{SavedConnection, SslMode};

/// Connection values from the command line (`None` = not given).
#[derive(Debug, Clone, Default)]
pub struct ConnArgs {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    /// Database name or a `postgresql://` URI.
    pub dbname: Option<String>,
    pub password_prompt: PasswordPrompt,
}

/// `-w` / `-W` (psql semantics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PasswordPrompt {
    /// Prompt only when the server asks and no password is known.
    #[default]
    Auto,
    /// `-w`: never prompt; a missing password is an error.
    Never,
    /// `-W`: prompt before connecting.
    Always,
}

/// Result: connection to save plus a session-only password (URI / `PGPASSWORD`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub connection: SavedConnection,
    pub password: Option<String>,
    pub password_prompt: PasswordPrompt,
}

/// Parts of a connection URI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UriParts {
    pub user: Option<String>,
    pub password: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub dbname: Option<String>,
    pub ssl_mode: Option<SslMode>,
    pub ssl_root_cert: Option<String>,
}

pub fn is_uri(s: &str) -> bool {
    s.starts_with("postgresql://") || s.starts_with("postgres://")
}

/// Resolves with the real environment.
pub fn resolve(args: &ConnArgs) -> Result<Resolved, String> {
    resolve_with(args, &|name| {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    })
}

/// Resolves with `env` as the environment (testable).
pub fn resolve_with(
    args: &ConnArgs,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Resolved, String> {
    let uri = match args.dbname.as_deref() {
        Some(d) if is_uri(d) => parse_uri(d)?,
        _ => UriParts::default(),
    };
    let plain_db = args.dbname.clone().filter(|d| !is_uri(d));

    let env_port = match env("PGPORT") {
        Some(p) => Some(
            p.parse::<u16>()
                .map_err(|_| format!("invalid PGPORT '{p}'"))?,
        ),
        None => None,
    };
    let env_ssl = match env("PGSSLMODE") {
        Some(m) => Some(SslMode::from_name(&m).ok_or_else(|| format!("invalid PGSSLMODE '{m}'"))?),
        None => None,
    };

    let host = uri
        .host
        .or_else(|| args.host.clone())
        .or_else(|| env("PGHOST"))
        .unwrap_or_else(|| "localhost".to_owned());
    let port = uri.port.or(args.port).or(env_port).unwrap_or(5432);
    let username = uri
        .user
        .or_else(|| args.username.clone())
        .or_else(|| env("PGUSER"))
        .or_else(|| env("USER"))
        .or_else(|| env("USERNAME"))
        .unwrap_or_else(|| "postgres".to_owned());
    let database = uri
        .dbname
        .or(plain_db)
        .or_else(|| env("PGDATABASE"))
        .unwrap_or_else(|| username.clone());
    let ssl_mode = uri.ssl_mode.or(env_ssl).unwrap_or_default();
    let ssl_root_cert = uri.ssl_root_cert.or_else(|| env("PGSSLROOTCERT"));
    let password = uri.password.or_else(|| env("PGPASSWORD"));

    Ok(Resolved {
        connection: SavedConnection {
            name: format!("{username}@{host}:{port}/{database}"),
            host,
            port,
            database,
            username,
            ssl_mode,
            ssl_root_cert,
        },
        password,
        password_prompt: args.password_prompt,
    })
}

/// Parses `postgresql://[user[:password]@][host][:port][/dbname][?param=value&...]`.
pub fn parse_uri(uri: &str) -> Result<UriParts, String> {
    let rest = uri
        .strip_prefix("postgresql://")
        .or_else(|| uri.strip_prefix("postgres://"))
        .ok_or_else(|| format!("not a postgresql:// URI: {uri}"))?;
    let (main, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (authority, path) = main.split_once('/').unwrap_or((main, ""));
    let mut parts = UriParts::default();

    let hostport = match authority.rsplit_once('@') {
        Some((userinfo, hostport)) => {
            let (user, password) = match userinfo.split_once(':') {
                Some((u, p)) => (u, Some(p)),
                None => (userinfo, None),
            };
            parts.user = non_empty(decode(user)?);
            parts.password = password.map(decode).transpose()?;
            hostport
        }
        None => authority,
    };
    if hostport.contains(',') {
        return Err("several hosts in one URI are not supported".to_owned());
    }
    let (host, port) = if let Some(v6) = hostport.strip_prefix('[') {
        let (h, after) = v6.split_once(']').ok_or("unclosed '[' in URI host")?;
        (h.to_owned(), after.strip_prefix(':'))
    } else {
        match hostport.split_once(':') {
            Some((h, p)) => (decode(h)?, Some(p)),
            None => (decode(hostport)?, None),
        }
    };
    parts.host = non_empty(host);
    if let Some(p) = port.filter(|p| !p.is_empty()) {
        parts.port = Some(
            p.parse()
                .map_err(|_| format!("invalid port '{p}' in URI"))?,
        );
    }
    parts.dbname = non_empty(decode(path)?);

    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let v = decode(v)?;
        match k {
            "sslmode" => {
                parts.ssl_mode =
                    Some(SslMode::from_name(&v).ok_or_else(|| format!("invalid sslmode '{v}'"))?);
            }
            "sslrootcert" => parts.ssl_root_cert = non_empty(v),
            "host" => parts.host = non_empty(v),
            "port" => parts.port = Some(v.parse().map_err(|_| format!("invalid port '{v}'"))?),
            "user" => parts.user = non_empty(v),
            "password" => parts.password = Some(v),
            "dbname" => parts.dbname = non_empty(v),
            other => tracing::warn!("URI parameter '{other}' is not supported; ignored"),
        }
    }
    Ok(parts)
}

fn non_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

/// Percent-decoding (UTF-8).
fn decode(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .ok_or_else(|| format!("invalid percent-encoding in '{s}'"))?;
            out.push(hex);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| format!("invalid UTF-8 in '{s}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
    }

    #[test]
    fn uri_full() {
        let p = parse_uri("postgresql://me:p%40ss@db.example:5433/my%20db?sslmode=verify-full&sslrootcert=/ca.pem")
            .unwrap();
        assert_eq!(p.user.as_deref(), Some("me"));
        assert_eq!(p.password.as_deref(), Some("p@ss"));
        assert_eq!(p.host.as_deref(), Some("db.example"));
        assert_eq!(p.port, Some(5433));
        assert_eq!(p.dbname.as_deref(), Some("my db"));
        assert_eq!(p.ssl_mode, Some(SslMode::VerifyFull));
        assert_eq!(p.ssl_root_cert.as_deref(), Some("/ca.pem"));
    }

    #[test]
    fn uri_minimal_and_ipv6() {
        assert_eq!(parse_uri("postgres://").unwrap(), UriParts::default());
        let p = parse_uri("postgresql://[::1]:6000/x").unwrap();
        assert_eq!(p.host.as_deref(), Some("::1"));
        assert_eq!(p.port, Some(6000));
        assert!(parse_uri("postgresql://a,b/x").is_err());
        assert!(parse_uri("postgresql://h/x?sslmode=bogus").is_err());
        assert!(parse_uri("postgresql://h:port/x").is_err());
    }

    #[test]
    fn defaults_from_environment() {
        let env = env_of(&[
            ("PGHOST", "envhost"),
            ("PGPORT", "6543"),
            ("USER", "osuser"),
            ("PGPASSWORD", "pw"),
        ]);
        let r = resolve_with(&ConnArgs::default(), &env).unwrap();
        assert_eq!(r.connection.host, "envhost");
        assert_eq!(r.connection.port, 6543);
        assert_eq!(r.connection.username, "osuser");
        assert_eq!(r.connection.database, "osuser");
        assert_eq!(r.connection.ssl_mode, SslMode::Prefer);
        assert_eq!(r.connection.name, "osuser@envhost:6543/osuser");
        assert_eq!(r.password.as_deref(), Some("pw"));
    }

    #[test]
    fn precedence_uri_over_options_over_env() {
        let env = env_of(&[("PGHOST", "envhost"), ("PGUSER", "envuser")]);
        let args = ConnArgs {
            host: Some("opthost".into()),
            port: Some(1111),
            username: Some("optuser".into()),
            dbname: Some("postgresql://uriuser@urihost/uridb".into()),
            password_prompt: PasswordPrompt::Auto,
        };
        let r = resolve_with(&args, &env).unwrap();
        assert_eq!(r.connection.host, "urihost");
        assert_eq!(r.connection.port, 1111);
        assert_eq!(r.connection.username, "uriuser");
        assert_eq!(r.connection.database, "uridb");
        let args = ConnArgs {
            dbname: Some("plain".into()),
            ..args
        };
        let r = resolve_with(&args, &env).unwrap();
        assert_eq!(r.connection.host, "opthost");
        assert_eq!(r.connection.database, "plain");
    }

    #[test]
    fn bad_environment_is_an_error() {
        assert!(resolve_with(&ConnArgs::default(), &env_of(&[("PGPORT", "x")])).is_err());
        assert!(resolve_with(&ConnArgs::default(), &env_of(&[("PGSSLMODE", "x")])).is_err());
    }
}
