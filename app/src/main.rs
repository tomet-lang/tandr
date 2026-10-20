use std::path::PathBuf;
use clap::{Parser, Subcommand};
use tandr_core::{RuleSet, apply_rule_set};

#[derive(Parser, Debug)]
#[command(
    name = "tandr",
    version,
    about = "Structural search, rewrite, and formatting engine for Tomet",
    long_about = "tandr transforms Tomet document structures using patterns defined in motem (.tandr) rule files."
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

        /// Target tomet files to transform
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
            let content = std::fs::read_to_string(&rules)
                .map_err(|e| anyhow::anyhow!("Failed to read rule file {}: {e}", rules.display()))?;
            let rule_set = RuleSet::from_motem_str(&content)?;
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
        } => {
            let rule_content = std::fs::read_to_string(&rules)
                .map_err(|e| anyhow::anyhow!("Failed to read rule file {}: {e}", rules.display()))?;
            let rule_set = RuleSet::from_motem_str(&rule_content)?;

            let mut any_modified = false;

            if targets.is_empty() {
                // Read from stdin if no targets given
                use std::io::Read;
                let mut buffer = String::new();
                std::io::stdin().read_to_string(&mut buffer)?;
                let res = apply_rule_set(&buffer, &rule_set)?;
                if diff {
                    print_diff("<stdin>", &buffer, &res.output);
                } else {
                    print!("{}", res.output);
                }
                if res.modified {
                    any_modified = true;
                }
            } else {
                for target_path in &targets {
                    let src = std::fs::read_to_string(target_path).map_err(|e| {
                        anyhow::anyhow!("Failed to read target {}: {e}", target_path.display())
                    })?;

                    let res = apply_rule_set(&src, &rule_set)?;
                    if res.modified {
                        any_modified = true;
                        println!(
                            "~ {}: {} change(s)",
                            target_path.display(),
                            res.changes_count
                        );

                        if diff {
                            print_diff(&target_path.display().to_string(), &src, &res.output);
                        }

                        if in_place {
                            std::fs::write(target_path, &res.output).map_err(|e| {
                                anyhow::anyhow!(
                                    "Failed to write to {}: {e}",
                                    target_path.display()
                                )
                            })?;
                        } else if !diff {
                            print!("{}", res.output);
                        }
                    } else if !check && !diff {
                        println!("= {} (unchanged)", target_path.display());
                    }
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
        } else if j < new_lines.len() && (i >= old_lines.len() || !old_lines.contains(&new_lines[j]))
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
