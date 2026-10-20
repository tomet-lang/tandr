//! Rewriting and template expansion engine for tandr.
//!
//! Applies matched variable bindings to replacement templates and performs
//! non-destructive span replacements on the source text.

use anyhow::Result;
use tomet_parser::parse_document;

use crate::matcher::{Bindings, find_matches};
use crate::pattern::parse_pattern;
use crate::rule::RuleSet;

/// The result of applying rewrite rules to a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteResult {
    /// The resulting transformed document source text.
    pub output: String,
    /// Total number of transformations applied.
    pub changes_count: usize,
    /// Whether any modification was made to the original input.
    pub modified: bool,
}

/// Applies a single replacement template given variable bindings.
pub fn expand_template(template: &str, bindings: &Bindings) -> String {
    let mut result = template.to_string();

    // First replace rest patterns `...?var`
    for (k, v) in bindings {
        let rest_placeholder = format!("...?{k}");
        if result.contains(&rest_placeholder) {
            result = result.replace(&rest_placeholder, v);
        }
    }

    // Next replace standard variables `?var`
    for (k, v) in bindings {
        let var_placeholder = format!("?{k}");
        if result.contains(&var_placeholder) {
            result = result.replace(&var_placeholder, v);
        }
    }

    result
}

/// Applies all rules in `rule_set` to `src`.
pub fn apply_rule_set(src: &str, rule_set: &RuleSet) -> Result<RewriteResult> {
    let mut current_text = src.to_string();
    let mut total_changes = 0;

    for rule in &rule_set.rules {
        let pattern = parse_pattern(&rule.def.r#match)
            .map_err(|e| anyhow::anyhow!("Invalid pattern in rule '{}': {e}", rule.name))?;

        let Some(replace_tmpl) = &rule.def.replace else {
            continue;
        };

        // Parse document AST
        let doc = match parse_document(&current_text) {
            Ok(d) => d,
            Err(e) => {
                // If parsing fails mid-transformation, return error with context
                return Err(anyhow::anyhow!(
                    "Failed to parse document during rule '{}': {e}",
                    rule.name
                ));
            }
        };

        let matches = find_matches(&doc, &pattern, &current_text);
        if matches.is_empty() {
            continue;
        }

        // Apply replacements from back to front so spans remain valid
        let mut sorted_matches = matches;
        sorted_matches.sort_by_key(|m| std::cmp::Reverse(m.span.start));

        for m in sorted_matches {
            let replacement = expand_template(replace_tmpl, &m.bindings);
            let start = m.span.start;
            let end = m.span.end;

            if start <= end && end <= current_text.len() {
                current_text.replace_range(start..end, &replacement);
                total_changes += 1;
            }
        }
    }

    let modified = current_text != src;
    Ok(RewriteResult {
        output: current_text,
        changes_count: total_changes,
        modified,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_meta_type_to_kind() {
        let src = "@meta{ type: \"article\", title: \"My Note\" }\n\nContent here.\n";
        let rules_motem = r#"
            promote_meta: {
                match: "@meta{ type: ?k, ...?rest }"
                replace: "@kind(?k)\n@meta{ ...?rest }"
            }
        "#;
        let rule_set = RuleSet::from_motem_str(rules_motem).unwrap();
        let res = apply_rule_set(src, &rule_set).unwrap();

        assert!(res.modified);
        assert_eq!(res.changes_count, 1);
        assert_eq!(
            res.output,
            "@kind(\"article\")\n@meta{ title: \"My Note\" }\n\nContent here.\n"
        );
    }

    #[test]
    fn rewrites_links_in_prose() {
        let src = "Read @link(url: \"https://example.com\")[ Click Here ] now!\n";
        let rules_motem = r#"
            add_rel: {
                match: "@link(url: ?u)[ ?t ]"
                replace: "@link(url: ?u, rel: \"external\")[ ?t ]"
            }
        "#;
        let rule_set = RuleSet::from_motem_str(rules_motem).unwrap();
        let res = apply_rule_set(src, &rule_set).unwrap();

        assert!(res.modified);
        assert_eq!(
            res.output,
            "Read @link(url: \"https://example.com\", rel: \"external\")[ Click Here ] now!\n"
        );
    }
}
