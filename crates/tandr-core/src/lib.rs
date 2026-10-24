//! =[ tandr-core ]
//!
//! Core rule engine, pattern matcher, and rewriter for Tomet.
//!
//! Tandr searches and transforms Tomet document structures using patterns
//! defined in cotomet (`.tandr`) configuration files.

pub mod cst;
pub mod hole;
pub mod matcher;
pub mod pattern;
pub mod rewrite;
pub mod rule;
pub mod template;

pub use matcher::{Bindings, MatchResult, find_matches, match_element};
pub use pattern::{ElementPattern, Pattern, SigilKind, parse_pattern};
pub use rewrite::{RewriteResult, apply_rule_set};
pub use rule::{CompiledRule, NamedRule, PartEdits, Replace, RuleDef, RuleSet};
pub use template::Template;
