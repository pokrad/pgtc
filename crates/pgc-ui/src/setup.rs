//! Installation steps (D-047), used by `pgtc --setup`, `--install-templates` and the
//! Settings menu, so every entry point does the same. Today the only step is
//! install templates (D-013); later steps are added to [`run_setup`].

use pgc_core::paths::Paths;
use pgc_sql::devtemplate;

/// The outcome of one installation step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutcome {
    pub ok: bool,
    /// One or more lines, starting with the step name.
    pub text: String,
}

/// Every installation step, in order (D-047). A failed step does not stop the next.
pub fn run_setup(paths: &Paths) -> Vec<StepOutcome> {
    vec![templates_step(paths)]
}

/// Copy the embedded templates into the built-in directory and create the (empty)
/// user templates directory (D-013).
pub fn templates_step(paths: &Paths) -> StepOutcome {
    let dir = pgc_core::paths::system_templates_dir();
    let (mut ok, mut text) = match devtemplate::install(&dir) {
        Ok(n) => (
            true,
            format!("Templates: {n} installed into {}", dir.display()),
        ),
        Err(e) => (
            false,
            format!(
                "Templates: cannot install into {}: {e}\n  This directory usually needs administrator rights.",
                dir.display()
            ),
        ),
    };
    text.push('\n');
    text.push_str(&user_dir_line(&paths.user_templates_dir(), &mut ok));
    StepOutcome { ok, text }
}

/// Create the user templates directory; existing files in it are kept.
fn user_dir_line(user: &std::path::Path, ok: &mut bool) -> String {
    match std::fs::create_dir_all(user) {
        Ok(()) => format!("  User templates folder: {}", user.display()),
        Err(e) => {
            *ok = false;
            format!(
                "  Cannot create the user templates folder {}: {e}",
                user.display()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_templates_folder_is_created_and_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let user = tmp.path().join("data").join("developer_templates");
        let mut ok = true;
        assert!(user_dir_line(&user, &mut ok).contains("User templates folder"));
        assert!(ok && user.is_dir());
        std::fs::write(user.join("mine.pgtemplate"), "x").unwrap();
        user_dir_line(&user, &mut ok);
        assert!(ok && user.join("mine.pgtemplate").is_file());
    }
}
