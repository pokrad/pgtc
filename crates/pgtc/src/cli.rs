//! Command-line arguments (D-031): psql-compatible connection options plus pgtc's own
//! actions (`--setup`, `--install-templates`).

use clap::{ArgAction, Parser};
use pgc_core::{APP_LONG_NAME, APP_NAME};

/// `pgtc [OPTION]... [DBNAME [USERNAME]]`
#[derive(Debug, Parser)]
#[command(
    name = APP_NAME,
    version,
    about = APP_LONG_NAME,
    override_usage = "pgtc [OPTION]... [DBNAME [USERNAME]]",
    //`-h` is the host (as in psql); help is `-?` / `--help`.
    disable_help_flag = true,
    disable_version_flag = true,
)]
pub struct Cli {
    /// Database server host or socket directory
    #[arg(
        short = 'h',
        long,
        value_name = "HOSTNAME",
        help_heading = "Connection options"
    )]
    pub host: Option<String>,

    /// Database server port
    #[arg(
        short = 'p',
        long,
        value_name = "PORT",
        help_heading = "Connection options"
    )]
    pub port: Option<u16>,

    /// Database user name
    #[arg(
        short = 'U',
        long,
        value_name = "USERNAME",
        help_heading = "Connection options"
    )]
    pub username: Option<String>,

    /// Database name to connect to
    #[arg(
        short = 'd',
        long,
        value_name = "DBNAME",
        help_heading = "Connection options"
    )]
    pub dbname: Option<String>,

    /// Never prompt for password
    #[arg(
        short = 'w',
        long,
        conflicts_with = "password",
        help_heading = "Connection options"
    )]
    pub no_password: bool,

    /// Force password prompt
    #[arg(short = 'W', long, help_heading = "Connection options")]
    pub password: bool,

    /// Run every installation step (currently: install templates, D-047)
    #[arg(long)]
    pub setup: bool,

    /// Copy the embedded developer templates into the system templates folder (D-013)
    #[arg(long)]
    pub install_templates: bool,

    /// Database name (if not given with -d), then user name (if not given with -U)
    #[arg(value_name = "DBNAME", num_args = 0..=2)]
    pub positional: Vec<String>,

    /// Show this help, then exit
    #[arg(short = '?', long, action = ArgAction::Help)]
    pub help: Option<bool>,

    /// Output version information, then exit
    #[arg(short = 'V', long, action = ArgAction::Version)]
    pub version: Option<bool>,
}

/// Connection parameters given on the command line; `None` = not given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConnectionArgs {
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub dbname: Option<String>,
    pub password_prompt: PasswordPrompt,
}

pub use pgc_db::conninfo::PasswordPrompt;

impl Cli {
    /// Combines options and positional arguments the way psql does: each positional
    /// argument fills `dbname` if it is still unset, otherwise `username`.
    pub fn connection_args(&self) -> ConnectionArgs {
        let mut dbname = self.dbname.clone();
        let mut username = self.username.clone();
        for arg in &self.positional {
            if dbname.is_none() {
                dbname = Some(arg.clone());
            } else if username.is_none() {
                username = Some(arg.clone());
            } else {
                tracing::warn!("extra command-line argument {arg:?} ignored");
            }
        }
        let password_prompt = if self.no_password {
            PasswordPrompt::Never
        } else if self.password {
            PasswordPrompt::Always
        } else {
            PasswordPrompt::Auto
        };
        ConnectionArgs {
            host: self.host.clone(),
            port: self.port,
            username,
            dbname,
            password_prompt,
        }
    }
}

impl ConnectionArgs {
    /// True if no connection parameter was given.
    pub fn is_empty(&self) -> bool {
        *self == ConnectionArgs::default()
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("pgtc").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn command_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn short_h_is_host() {
        let c = parse(&["-h", "db.example", "-p", "5433", "-U", "admin", "-d", "app"]);
        assert_eq!(
            c.connection_args(),
            ConnectionArgs {
                host: Some("db.example".into()),
                port: Some(5433),
                username: Some("admin".into()),
                dbname: Some("app".into()),
                password_prompt: PasswordPrompt::Auto,
            }
        );
    }

    #[test]
    fn positional_dbname_then_username() {
        let a = parse(&["mydb", "me"]).connection_args();
        assert_eq!(a.dbname.as_deref(), Some("mydb"));
        assert_eq!(a.username.as_deref(), Some("me"));
    }

    #[test]
    fn positional_fills_username_when_dbname_given() {
        let a = parse(&["-d", "mydb", "me"]).connection_args();
        assert_eq!(a.dbname.as_deref(), Some("mydb"));
        assert_eq!(a.username.as_deref(), Some("me"));
    }

    #[test]
    fn options_win_over_positional() {
        let a = parse(&["-d", "a", "-U", "u", "b"]).connection_args();
        assert_eq!(a.dbname.as_deref(), Some("a"));
        assert_eq!(a.username.as_deref(), Some("u"));
    }

    #[test]
    fn password_flags() {
        assert_eq!(
            parse(&["-w"]).connection_args().password_prompt,
            PasswordPrompt::Never
        );
        assert_eq!(
            parse(&["-W"]).connection_args().password_prompt,
            PasswordPrompt::Always
        );
        assert!(Cli::try_parse_from(["pgtc", "-w", "-W"]).is_err());
    }

    #[test]
    fn help_and_version_flags() {
        let e = Cli::try_parse_from(["pgtc", "-?"]).unwrap_err();
        assert_eq!(e.kind(), clap::error::ErrorKind::DisplayHelp);
        let e = Cli::try_parse_from(["pgtc", "-V"]).unwrap_err();
        assert_eq!(e.kind(), clap::error::ErrorKind::DisplayVersion);
    }

    #[test]
    fn bad_port_is_rejected() {
        assert!(Cli::try_parse_from(["pgtc", "-p", "99999"]).is_err());
    }

    #[test]
    fn no_args_means_no_connection() {
        assert!(parse(&[]).connection_args().is_empty());
        assert!(!parse(&["mydb"]).connection_args().is_empty());
    }
}
