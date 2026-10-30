mod targets;

use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;
use tandr_core::{RuleSet, apply_rule_set};

/// Exit codes: what a caller (CI, sisho) can rely on.
///
/// `0` nothing to change; `1` `--check` found something it would change; `2` an error (a bad
/// rule file, a target that could not be read, parsed or written, or a bad command line, which
/// clap already reports as `2`). `2` wins over `1`: a run that failed somewhere does not know
/// the result.
const EXIT_CHANGED: u8 = 1;
const EXIT_ERROR: u8 = 2;

/// How `run` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Format {
    /// One line per changed file, then a count.
    Text,
    /// One JSON document on standard output: `{ changed, failed, files: [{ path, changed,
    /// changes, error? }] }`. Needs `--check` or `--in-place`, which keep the rewritten
    /// documents off standard output.
    Json,
}

/// What happened to one target, for `--format json`.
#[derive(Serialize)]
struct FileReport {
    path: String,
    changed: bool,
    changes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct Report {
    /// Files that were (or, under `--check`, would be) changed.
    changed: usize,
    /// Files that could not be read, rewritten or written.
    failed: usize,
    files: Vec<FileReport>,
}

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

        /// Check mode: write nothing; exit 1 if any target would be modified (0 if none, 2 on
        /// an error)
        #[arg(long)]
        check: bool,

        /// How to report: `text` (default) or `json`
        #[arg(long, value_enum, default_value = "text", conflicts_with = "diff")]
        format: Format,

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

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
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
            format,
            diff,
            force,
        } => {
            let json = format == Format::Json;
            if json && !(check || in_place) {
                anyhow::bail!(
                    "--format json needs --check or --in-place: without them the rewritten \
                     document goes to standard output, where the report is"
                );
            }
            if json && targets.is_empty() && !check {
                anyhow::bail!("--format json on standard input needs --check");
            }
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

            let mut files: Vec<FileReport> = Vec::new();

            if targets.is_empty() {
                // Read from stdin if no targets given
                use std::io::Read;
                let mut buffer = String::new();
                std::io::stdin().read_to_string(&mut buffer)?;
                let res = apply_rule_set(&buffer, &rule_set)?;
                for w in &res.warnings {
                    eprintln!("warning: <stdin>: {w}");
                }
                if json {
                    // the report is the output; the rewritten document is not
                } else if diff {
                    print_diff("<stdin>", &buffer, &res.output);
                } else {
                    print!("{}", res.output);
                }
                files.push(FileReport {
                    path: "<stdin>".to_string(),
                    changed: res.modified,
                    changes: res.changes_count,
                    error: None,
                });
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

                for target in &resolved.targets {
                    let path = &target.path;
                    let shown = path.display().to_string();
                    let mut file = FileReport {
                        path: shown.clone(),
                        changed: false,
                        changes: 0,
                        error: None,
                    };
                    let src = match std::fs::read_to_string(path) {
                        Ok(src) => src,
                        Err(e) => {
                            eprintln!("error: {shown}: could not be read: {e}");
                            file.error = Some(format!("could not be read: {e}"));
                            files.push(file);
                            continue;
                        }
                    };
                    let res = match apply_rule_set(&src, &rule_set) {
                        Ok(res) => res,
                        Err(e) => {
                            eprintln!("error: {shown}: {e}");
                            file.error = Some(e.to_string());
                            files.push(file);
                            continue;
                        }
                    };
                    for w in &res.warnings {
                        eprintln!("warning: {shown}: {w}");
                    }
                    file.changed = res.modified;
                    file.changes = res.changes_count;
                    if res.modified {
                        if !json {
                            println!("~ {shown}: {} change(s)", res.changes_count);
                        }

                        if diff {
                            print_diff(&shown, &src, &res.output);
                        }

                        if in_place {
                            if let Err(e) = std::fs::write(path, &res.output) {
                                eprintln!("error: {shown}: could not be written: {e}");
                                file.error = Some(format!("could not be written: {e}"));
                            }
                        } else if !diff && !check {
                            print!("{}", res.output);
                        }
                    } else if target.named && !check && !diff && !json {
                        println!("= {shown} (unchanged)");
                    }
                    files.push(file);
                }
                if !json && (resolved.swept || resolved.targets.len() > 1) {
                    let changed = files.iter().filter(|f| f.changed).count();
                    println!("{changed} of {} file(s) changed", resolved.targets.len());
                }
            }

            let report = Report {
                changed: files.iter().filter(|f| f.changed).count(),
                failed: files.iter().filter(|f| f.error.is_some()).count(),
                files,
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            }
            if report.failed > 0 {
                eprintln!("error: {} file(s) failed", report.failed);
                return Ok(ExitCode::from(EXIT_ERROR));
            }
            if check && report.changed > 0 {
                return Ok(ExitCode::from(EXIT_CHANGED));
            }
        }
    }

    Ok(ExitCode::SUCCESS)
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
