//! Installation steps (D-047), used by `pgtc --setup`, `--install-templates` and the
//! Settings menu, so every entry point does the same. Today the only step is
//! install templates (D-013); later steps are added to [`run_setup`].

use std::path::Path;

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
/// `overwrite`: the answer to [`overwrite_question`] (when there was one).
pub fn run_setup(paths: &Paths, overwrite: bool) -> Vec<StepOutcome> {
    vec![templates_step(paths, overwrite)]
}

/// The question to ask before installing: the templates folder exists, so the
/// templates that come with pgtc would overwrite files of the same name (D-013).
/// `None` when the folder does not exist yet (everything is installed, nothing asked).
pub fn overwrite_question(paths: &Paths) -> Option<String> {
    let dir = paths.user_templates_dir();
    dir.is_dir().then(|| {
        format!(
            "The templates folder already exists:\n{}\n\nOverwrite the templates that come with pgtc?\nYour own templates (other file names) are kept.",
            dir.display()
        )
    })
}

/// Copy the embedded templates into the user templates folder (D-013). An existing
/// folder is written only with `overwrite`; files of other names are never touched.
pub fn templates_step(paths: &Paths, overwrite: bool) -> StepOutcome {
    install_into(&paths.user_templates_dir(), overwrite)
}

fn install_into(dir: &Path, overwrite: bool) -> StepOutcome {
    let existed = dir.is_dir();
    if existed && !overwrite {
        return StepOutcome {
            ok: true,
            text: format!(
                "Templates: not installed, the folder is kept as it is:\n  {}",
                dir.display()
            ),
        };
    }
    match devtemplate::install(dir) {
        Ok(n) => StepOutcome {
            ok: true,
            text: format!(
                "Templates: {n} {} into\n  {}",
                if existed {
                    "installed (overwritten)"
                } else {
                    "installed"
                },
                dir.display()
            ),
        },
        Err(e) => StepOutcome {
            ok: false,
            text: format!("Templates: cannot install into {}: {e}", dir.display()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_creates_asks_and_overwrites_only_its_own_files() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::with_dirs(tmp.path().join("cfg"), tmp.path().join("data"));
        let dir = paths.user_templates_dir();
        // no folder yet: nothing to ask, everything installed
        assert!(overwrite_question(&paths).is_none());
        let s = templates_step(&paths, false);
        assert!(s.ok, "{}", s.text);
        let installed = devtemplate::embedded_files();
        let (first, _) = &installed[0];
        assert!(dir.join(first).is_file());
        // the folder exists: asked; No keeps everything
        assert!(overwrite_question(&paths).is_some());
        std::fs::write(dir.join(first), "changed").unwrap();
        std::fs::write(dir.join("mine.pgtemplate"), "mine").unwrap();
        let s = templates_step(&paths, false);
        assert!(s.ok && s.text.contains("not installed"), "{}", s.text);
        assert_eq!(std::fs::read_to_string(dir.join(first)).unwrap(), "changed");
        // Yes: pgtc's files are overwritten, the user's own file stays
        let s = templates_step(&paths, true);
        assert!(s.ok && s.text.contains("overwritten"), "{}", s.text);
        assert_ne!(std::fs::read_to_string(dir.join(first)).unwrap(), "changed");
        assert_eq!(
            std::fs::read_to_string(dir.join("mine.pgtemplate")).unwrap(),
            "mine"
        );
    }
}
