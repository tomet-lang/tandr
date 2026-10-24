mod targets;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tandr_core::{RuleSet, apply_rule_set};

#[derive(Parser, Debug)]
#[command(
    name = "tandr",
    version,
    about = "Structural search, rewrite, and formatting engine for Tomet",
    long_about = "tandr transforms Tomet document structures using patterns defined in cotomet (.tandr) rule files."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run rewrite rules from a .tandr file against target document(s)
    Run {
        /// Path to the .tandr rules file
        #[arg(short, long, value_name = "RULES")]
        rules: PathBuf,

        /// Tomet files or directories to transform. A directory is swept for
        /// `.tmt` files the way `tomet format` sweeps one: `.gitignore` and the
        /// project config's `workspace.ignore` / `workspace.unswept` apply.
        /// Reads standard input when none is given.
        #[arg(value_name = "TARGETS")]
        targets: Vec<PathBuf>,

        /// Apply modifications in-place to files
        #[arg(short = 'i', long)]
        in_place: bool,

        /// Check mode: exit with code 1 if any target would be modified
        #[arg(long)]
        check: bool,

        /// Show unified diff
        #[arg(long)]
        diff: bool,

        /// Also rewrite files and directories the project config excludes
        /// (`workspace.ignore`, `workspace.unswept`). `.gitignore` still applies.
        #[arg(long)]
        force: bool,
    },

    /// Validate the syntax of a .tandr rule file
    CheckRules {
        /// Path to the .tandr rules file
        #[arg(value_name = "RULES")]
        rules: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::CheckRules { rules } => {
            let content = std::fs::read_to_string(&rules).map_err(|e| {
                anyhow::anyhow!("Failed to read rule file {}: {e}", rules.display())
            })?;
            let rule_set = RuleSet::from_cotomet_str(&content)?;
            if let Err(errors) = rule_set.compile() {
                for e in &errors {
                    eprintln!("error: {e}");
                }
                anyhow::bail!("{} problem(s) in {}", errors.len(), rules.display());
            }
            println!(
                "✓ Successfully parsed {} rule(s) from {}",
                rule_set.rules.len(),
                rules.display()
            );
            for r in &rule_set.rules {
                println!("  - {}: match: {}", r.name, r.def.r#match);
            }
        }
        Commands::Run {
            rules,
            targets,
            in_place,
            check,
            diff,
            force,
        } => {
            let rule_content = std::fs::read_to_string(&rules).map_err(|e| {
                anyhow::anyhow!("Failed to read rule file {}: {e}", rules.display())
            })?;
            let rule_set = RuleSet::from_cotomet_str(&rule_content)?;
            // once, before any file: a bad rule is the same for every file
            if let Err(errors) = rule_set.compile() {
                for e in &errors {
                    eprintln!("error: {e}");
                }
                anyhow::bail!("{} problem(s) in {}", errors.len(), rules.display());
            }

            let mut any_modified = false;

            if targets.is_empty() {
                // Read from stdin if no targets given
                use std::io::Read;
                let mut buffer = String::new();
                std::io::stdin().read_to_string(&mut buffer)?;
                let res = apply_rule_set(&buffer, &rule_set)?;
                for w in &res.warnings {
                    eprintln!("warning: <stdin>: {w}");
                }
                if diff {
                    print_diff("<stdin>", &buffer, &res.output);
                } else {
                    print!("{}", res.output);
                }
                if res.modified {
                    any_modified = true;
                }
            } else {
                let resolved = targets::resolve(&targets, force)?;
                for path in &resolved.skipped {
                    eprintln!(
                        "skipped {}: excluded by the project config (workspace.ignore / \
                         workspace.unswept); pass --force to include it",
                        path.display()
                    );
                }
                if resolved.swept && !(in_place || check || diff) {
                    anyhow::bail!(
                        "a directory needs --in-place, --diff or --check: printing every \
                         rewritten document one after another is not a result"
                    );
                }

                let (mut changed, mut failed) = (0usize, 0usize);
                for target in &resolved.targets {
                    let path = &target.path;
                    let src = match std::fs::read_to_string(path) {
                        Ok(src) => src,
                        Err(e) => {
                            eprintln!("error: {}: could not be read: {e}", path.display());
                            failed += 1;
                            continue;
                        }
                    };
                    let res = match apply_rule_set(&src, &rule_set) {
                        Ok(res) => res,
                        Err(e) => {
                            eprintln!("error: {}: {e}", path.display());
                            failed += 1;
                            continue;
                        }
                    };
                    for w in &res.warnings {
                        eprintln!("warning: {}: {w}", path.display());
                    }
                    if res.modified {
                        any_modified = true;
                        changed += 1;
                        println!("~ {}: {} change(s)", path.display(), res.changes_count);

                        if diff {
                            print_diff(&path.display().to_string(), &src, &res.output);
                        }

                        if in_place {
                            if let Err(e) = std::fs::write(path, &res.output) {
                                eprintln!("error: {}: could not be written: {e}", path.display());
                                failed += 1;
                            }
                        } else if !diff && !check {
                            print!("{}", res.output);
                        }
                    } else if target.named && !check && !diff {
                        println!("= {} (unchanged)", path.display());
                    }
                }
                if resolved.swept || resolved.targets.len() > 1 {
                    println!("{changed} of {} file(s) changed", resolved.targets.len());
                }
                if failed > 0 {
                    anyhow::bail!("{failed} file(s) failed");
                }
            }

            if check && any_modified {
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

fn print_diff(filename: &str, old: &str, new: &str) {
    println!("--- a/{filename}");
    println!("+++ b/{filename}");
    for change in diff_lines(old, new) {
        println!("{change}");
    }
}

fn diff_lines(old: &str, new: &str) -> Vec<String> {
    let mut out = Vec::new();
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let mut i = 0;
    let mut j = 0;

    while i < old_lines.len() || j < new_lines.len() {
        if i < old_lines.len() && j < new_lines.len() && old_lines[i] == new_lines[j] {
            out.push(format!(" {}", old_lines[i]));
            i += 1;
            j += 1;
        } else if j < new_lines.len()
            && (i >= old_lines.len() || !old_lines.contains(&new_lines[j]))
        {
            out.push(format!("+{}", new_lines[j]));
            j += 1;
        } else if i < old_lines.len() {
            out.push(format!("-{}", old_lines[i]));
            i += 1;
        }
    }

    out
}
