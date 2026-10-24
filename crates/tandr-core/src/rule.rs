//! Rule definitions for tandr (.tandr files).
//!
//! Style A: Map-based cotomet syntax:
//! ```tmt
//! rule_name: {
//!     description: "..."
//!     match: "..."
//!     where: "..."
//!     replace: "..."
//! }
//! ```
//!
//! `replace` is either a template, which takes the place of the whole match, or
//! a map of the element's parts, which replaces those parts and leaves every
//! other byte as it was:
//!
//! ```tmt
//! doc_kind: {
//!     match: "@kind"
//!     replace: { head: "::kind" }
//! }
//! ```

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::pattern::{Pattern, parse_pattern};
use crate::template::Template;

/// Definition of an individual rewrite rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleDef {
    /// Human-readable explanation of what this rule does.
    pub description: Option<String>,

    /// Pattern to match in tomet structure (e.g. `@meta{type: ?kind}`).
    pub r#match: String,

    /// Optional condition / guard expression (e.g. `?kind != "raw"`).
    pub r#where: Option<String>,

    /// What to put in place of the match (e.g. `@kind(?(kind))`).
    pub replace: Option<Replace>,
}

/// What a rule puts in place of what it matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replace {
    /// Takes the place of the whole match.
    Template(String),
    /// Replaces some parts of the matched element, the rest untouched.
    Parts(PartEdits),
}

/// The edits to the parts of a matched element. Each value is a template and
/// replaces the part *as written*, delimiters included (`args: "(x)"`,
/// `data: "{ a: 1 }"`); an empty string removes the part. A part the element
/// does not have cannot be replaced, and the match is skipped with a warning.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartEdits {
    /// Text put in front of the element.
    pub before: Option<String>,
    /// Text put after the element, its connects included.
    pub after: Option<String>,
    /// The sigil and name: `@kind`, `^`, `^footnote`.
    pub head: Option<String>,
    /// `(args)`
    pub args: Option<String>,
    /// `{data}`
    pub data: Option<String>,
    /// `[content]`
    pub content: Option<String>,
    /// `#(id)`
    pub id: Option<String>,
}

impl PartEdits {
    fn is_empty(&self) -> bool {
        self.before.is_none()
            && self.after.is_none()
            && self.head.is_none()
            && self.args.is_none()
            && self.data.is_none()
            && self.content.is_none()
            && self.id.is_none()
    }
}

impl Serialize for Replace {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Replace::Template(t) => serializer.serialize_str(t),
            Replace::Parts(p) => p.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for Replace {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ReplaceVisitor;

        impl<'de> Visitor<'de> for ReplaceVisitor {
            type Value = Replace;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a template string, or a map of the element's parts to replace")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Replace, E> {
                Ok(Replace::Template(v.to_string()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Replace, E> {
                Ok(Replace::Template(v))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Replace, A::Error> {
                PartEdits::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(Replace::Parts)
            }
        }

        deserializer.deserialize_any(ReplaceVisitor)
    }
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

/// A rule whose pattern and templates have been parsed and checked.
#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub name: String,
    pub pattern: Pattern,
    pub action: Option<Action>,
}

/// What a compiled rule does with a match.
#[derive(Debug, Clone)]
pub enum Action {
    Whole(Template),
    Parts(PartTemplates),
}

/// [`PartEdits`], with each template parsed.
#[derive(Debug, Clone, Default)]
pub struct PartTemplates {
    pub before: Option<Template>,
    pub after: Option<Template>,
    pub head: Option<Template>,
    pub args: Option<Template>,
    pub data: Option<Template>,
    pub content: Option<Template>,
    pub id: Option<Template>,
}

impl RuleSet {
    /// Checks every rule and parses its pattern and templates, or says
    /// everything that is wrong with the set.
    ///
    /// A rule that cannot be applied exactly as written is refused here, rather
    /// than applied more broadly or less than it says: `where` is not
    /// implemented, so a rule that has one is an error, and so is a template
    /// that inserts a name the pattern does not bind.
    pub fn compile(&self) -> Result<Vec<CompiledRule>, Vec<String>> {
        let mut errors = Vec::new();
        let mut compiled = Vec::new();
        for rule in &self.rules {
            let name = &rule.name;
            let mut problems: Vec<String> = Vec::new();

            let pattern = match parse_pattern(&rule.def.r#match) {
                Ok(p) => Some(p),
                Err(e) => {
                    problems.push(format!("match: {e}"));
                    None
                }
            };
            if rule.def.r#where.is_some() {
                problems.push(
                    "`where` is not implemented yet, so the rule would match more than it says"
                        .to_string(),
                );
            }
            let bound = pattern
                .as_ref()
                .map(Pattern::bound_names)
                .unwrap_or_default();

            let template =
                |src: &str, what: &str, problems: &mut Vec<String>| match Template::parse(src) {
                    Ok(t) => {
                        for n in t.names() {
                            if pattern.is_some() && !bound.contains(n) {
                                problems.push(format!(
                                    "{what} inserts ?({n}), which the pattern does not bind"
                                ));
                            }
                        }
                        Some(t)
                    }
                    Err(e) => {
                        problems.push(format!("{what}: {e}"));
                        None
                    }
                };

            let action = match &rule.def.replace {
                None => None,
                Some(Replace::Template(src)) => {
                    template(src, "replace", &mut problems).map(Action::Whole)
                }
                Some(Replace::Parts(edits)) => {
                    if edits.is_empty() {
                        problems.push("replace names no part to replace".to_string());
                    }
                    let mut parts = PartTemplates::default();
                    for (src, slot, what) in [
                        (&edits.before, &mut parts.before, "replace.before"),
                        (&edits.after, &mut parts.after, "replace.after"),
                        (&edits.head, &mut parts.head, "replace.head"),
                        (&edits.args, &mut parts.args, "replace.args"),
                        (&edits.data, &mut parts.data, "replace.data"),
                        (&edits.content, &mut parts.content, "replace.content"),
                        (&edits.id, &mut parts.id, "replace.id"),
                    ] {
                        if let Some(src) = src {
                            *slot = template(src, what, &mut problems);
                        }
                    }
                    Some(Action::Parts(parts))
                }
            };

            errors.extend(problems.into_iter().map(|p| format!("rule '{name}': {p}")));
            if let Some(pattern) = pattern {
                compiled.push(CompiledRule {
                    name: name.clone(),
                    pattern,
                    action,
                });
            }
        }
        if errors.is_empty() {
            Ok(compiled)
        } else {
            Err(errors)
        }
    }

    /// Parses a `.tandr` string written in cotomet map syntax.
    pub fn from_cotomet_str(src: &str) -> anyhow::Result<Self> {
        let map: BTreeMap<String, RuleDef> = cotomet::from_str(src)
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
    fn parses_style_a_cotomet_rules() {
        let src = r#"
            promote_meta_type: {
                description: "Promote meta.type to kind"
                match: "@meta{ type: ?(kind), ?(...rest) }"
                where: "?(kind) != 'raw'"
                replace: "@kind(?(kind)) @meta{ ?(...rest) }"
            }

            clean_drafts: {
                match: "=[ ?(title) ] >> @task(?(status))[ ?(body) ]"
                replace: "@task(draft)[ ?(body) ]"
            }
        "#;

        let rule_set = RuleSet::from_cotomet_str(src).expect("should parse rules");
        assert_eq!(rule_set.rules.len(), 2);

        let r1 = rule_set
            .rules
            .iter()
            .find(|r| r.name == "promote_meta_type")
            .unwrap();
        assert_eq!(
            r1.def.description.as_deref(),
            Some("Promote meta.type to kind")
        );
        assert_eq!(r1.def.r#match, "@meta{ type: ?(kind), ?(...rest) }");
        assert_eq!(r1.def.r#where.as_deref(), Some("?(kind) != 'raw'"));
        assert_eq!(
            r1.def.replace,
            Some(Replace::Template(
                "@kind(?(kind)) @meta{ ?(...rest) }".to_string()
            ))
        );

        let r2 = rule_set
            .rules
            .iter()
            .find(|r| r.name == "clean_drafts")
            .unwrap();
        assert_eq!(r2.def.description, None);
        assert_eq!(r2.def.r#where, None);
    }

    #[test]
    fn replace_is_a_template_or_a_map_of_parts() {
        let rule_set = RuleSet::from_cotomet_str(
            r#"
            whole: { match: "@a", replace: "@b" }
            parts: { match: "@a", replace: { head: "::a", args: "", after: "\n" } }
        "#,
        )
        .unwrap();
        let get = |n: &str| {
            rule_set
                .rules
                .iter()
                .find(|r| r.name == n)
                .unwrap()
                .def
                .replace
                .clone()
        };
        assert_eq!(get("whole"), Some(Replace::Template("@b".to_string())));
        assert_eq!(
            get("parts"),
            Some(Replace::Parts(PartEdits {
                head: Some("::a".to_string()),
                args: Some(String::new()),
                after: Some("\n".to_string()),
                ..Default::default()
            }))
        );
    }

    #[test]
    fn a_part_that_is_not_one_is_refused_by_name() {
        let err = RuleSet::from_cotomet_str(r#"r: { match: "@a", replace: { tail: "x" } }"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("tail"), "{err}");
    }

    #[test]
    fn a_key_the_rule_does_not_have_is_refused_not_ignored() {
        // a typo or a feature not built must not turn a rewrite into a mere search
        for key in ["replce", "py_transform"] {
            let src = format!(r#"r: {{ match: "@a", {key}: "x" }}"#);
            let err = RuleSet::from_cotomet_str(&src).unwrap_err().to_string();
            assert!(err.contains(key), "{err}");
        }
    }

    #[test]
    fn compiling_reports_every_problem_in_the_set() {
        let rule_set = RuleSet::from_cotomet_str(
            r#"
            a: { match: "@a", where: "?(x) == 1", replace: "?(y)" }
            b: { match: "nonsense" }
        "#,
        )
        .unwrap();
        let errors = rule_set.compile().unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("rule 'a'") && e.contains("where")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.contains("rule 'a'") && e.contains("?(y)")),
            "{errors:?}"
        );
        assert!(errors.iter().any(|e| e.contains("rule 'b'")), "{errors:?}");
    }
}
