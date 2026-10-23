//! Rule definitions for tandr (.tandr files).
//!
//! Style A: Map-based motem syntax:
//! ```tmt
//! rule_name: {
//!     description: "..."
//!     match: "..."
//!     where: "..."
//!     replace: "..."
//! }
//! ```

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

/// Definition of an individual rewrite rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDef {
    /// Human-readable explanation of what this rule does.
    pub description: Option<String>,

    /// Pattern to match in tomet structure (e.g. `@meta{type: ?kind}`).
    pub r#match: String,

    /// Optional condition / guard expression (e.g. `?kind != "raw"`).
    pub r#where: Option<String>,

    /// Replacement template (e.g. `@kind(?kind)`).
    pub replace: Option<String>,
}

/// A named rule with its identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedRule {
    pub name: String,
    pub def: RuleDef,
}

/// A collection of rules loaded from a `.tandr` document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuleSet {
    pub rules: Vec<NamedRule>,
}

impl RuleSet {
    /// Parses a `.tandr` string written in motem map syntax.
    pub fn from_motem_str(src: &str) -> anyhow::Result<Self> {
        let map: BTreeMap<String, RuleDef> = motem::from_str(src)
            .map_err(|e| anyhow::anyhow!("Failed to parse .tandr rules: {e}"))?;

        let rules = map
            .into_iter()
            .map(|(name, def)| NamedRule { name, def })
            .collect();

        Ok(Self { rules })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_style_a_motem_rules() {
        let src = r#"
            promote_meta_type: {
                description: "Promote meta.type to kind"
                match: "@meta{ type: ?kind, ...?rest }"
                where: "?kind != 'raw'"
                replace: "@kind(?kind) @meta{ ...?rest }"
            }

            clean_drafts: {
                match: "=[ ?title ] >> @task(?status)[ ?body ]"
                replace: "@task(draft)[ ?body | trim ]"
            }
        "#;

        let rule_set = RuleSet::from_motem_str(src).expect("should parse rules");
        assert_eq!(rule_set.rules.len(), 2);

        let r1 = rule_set.rules.iter().find(|r| r.name == "promote_meta_type").unwrap();
        assert_eq!(r1.def.description.as_deref(), Some("Promote meta.type to kind"));
        assert_eq!(r1.def.r#match, "@meta{ type: ?kind, ...?rest }");
        assert_eq!(r1.def.r#where.as_deref(), Some("?kind != 'raw'"));
        assert_eq!(r1.def.replace.as_deref(), Some("@kind(?kind) @meta{ ...?rest }"));

        let r2 = rule_set.rules.iter().find(|r| r.name == "clean_drafts").unwrap();
        assert_eq!(r2.def.description, None);
        assert_eq!(r2.def.r#where, None);
    }
}
