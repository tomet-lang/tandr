//! Which files a `tandr run` works on.
//!
//! A directory is swept the way `tomet format` sweeps one: `.gitignore` applies,
//! and so do the project config's `workspace.ignore` and `workspace.unswept`
//! (the frozen test corpus is there to be left exactly as it is). A file named
//! on the command line is held to the same exclusions, so a shell glob or
//! `git ls-files` cannot reach what a sweep would not; `force` lifts them both.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

/// A file to rewrite, and whether the command line named it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    /// Named outright, not found by sweeping a directory.
    pub named: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    pub targets: Vec<Target>,
    /// Named files the project config leaves alone.
    pub skipped: Vec<PathBuf>,
    /// Whether any path was a directory.
    pub swept: bool,
}

/// Every `.tmt` file under `dir`; with `force` the config's exclusions
/// (`ignore`, `unswept`) are not applied. `.gitignore` still is.
fn collect_dir(dir: &Path, force: bool) -> Vec<PathBuf> {
    if !force {
        return tomet_indexer::collect_tm_files(dir);
    }
    let (mut config, _, root) = tomet_load_config::find_config_file(dir).unwrap_or_else(|| {
        (
            tomet_config::PrinterConfig::default(),
            dir.to_path_buf(),
            dir.to_path_buf(),
        )
    });
    config.workspace.ignore.clear();
    config.workspace.unswept.clear();
    tomet_indexer::collect_tm_files_with_config(dir, &config, &root)
}

fn is_excluded_by_config(path: &Path) -> bool {
    tomet_load_config::find_config_file(path)
        .is_some_and(|(config, _, root)| tomet_indexer::is_excluded_by_config(path, &root, &config))
}

pub fn resolve(paths: &[PathBuf], force: bool) -> Result<Resolved> {
    let mut resolved = Resolved::default();
    for path in paths {
        if path.is_file() {
            if !force && is_excluded_by_config(path) {
                resolved.skipped.push(path.clone());
            } else {
                resolved.targets.push(Target {
                    path: path.clone(),
                    named: true,
                });
            }
        } else if path.is_dir() {
            resolved.swept = true;
            resolved.targets.extend(
                collect_dir(path, force)
                    .into_iter()
                    .map(|path| Target { path, named: false }),
            );
        } else {
            bail!("path '{}' does not exist", path.display());
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A vault with a frozen directory, as tomet's own repository has.
    fn vault(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandr_targets_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("frozen")).unwrap();
        fs::create_dir_all(dir.join("notes")).unwrap();
        fs::write(
            dir.join("default.config.tmt"),
            "::kind(config)\n::config{ workspace: { unswept: list(\"frozen\") } }\n",
        )
        .unwrap();
        for f in ["a.tmt", "notes/b.tmt", "frozen/c.tmt"] {
            fs::write(dir.join(f), "text\n").unwrap();
        }
        fs::write(dir.join("notes/readme.md"), "# not a tomet document\n").unwrap();
        dir
    }

    fn names(r: &Resolved, root: &Path) -> Vec<String> {
        r.targets
            .iter()
            .map(|t| t.path.strip_prefix(root).unwrap().display().to_string())
            .collect()
    }

    #[test]
    fn a_directory_is_swept_for_documents_and_the_config_is_respected() {
        let root = vault("sweep");
        let r = resolve(std::slice::from_ref(&root), false).unwrap();
        assert!(r.swept);
        assert!(r.targets.iter().all(|t| !t.named));
        assert_eq!(
            names(&r, &root),
            ["a.tmt", "default.config.tmt", "notes/b.tmt"]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn force_reaches_what_the_config_leaves_alone() {
        let root = vault("force");
        let r = resolve(std::slice::from_ref(&root), true).unwrap();
        assert!(names(&r, &root).contains(&"frozen/c.tmt".to_string()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_named_file_is_held_to_the_same_exclusions_until_forced() {
        let root = vault("named");
        let frozen = root.join("frozen/c.tmt");
        let normal = root.join("a.tmt");

        let r = resolve(&[frozen.clone(), normal.clone()], false).unwrap();
        assert!(!r.swept);
        assert_eq!(r.skipped, [frozen.clone()]);
        assert_eq!(
            r.targets,
            [Target {
                path: normal.clone(),
                named: true
            }]
        );

        let r = resolve(std::slice::from_ref(&frozen), true).unwrap();
        assert!(r.skipped.is_empty());
        assert_eq!(r.targets.len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_path_that_is_not_there_is_an_error() {
        let err = resolve(&[PathBuf::from("/no/such/path.tmt")], false).unwrap_err();
        assert!(err.to_string().contains("does not exist"), "{err}");
    }
}
