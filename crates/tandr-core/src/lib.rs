//! =[ tandr-core ]
//!
//! Core rule engine, pattern matcher, and rewriter for Tomet.
//!
//! Tandr searches and transforms Tomet document structures using patterns
//! defined in motem (`.tandr`) configuration files.

pub mod matcher;
pub mod pattern;
pub mod rewrite;
pub mod rule;

pub use matcher::{Bindings, MatchResult, find_matches, match_element};
pub use pattern::{ElementPattern, Pattern, parse_pattern};
pub use rewrite::{RewriteResult, apply_rule_set, expand_template};
pub use rule::{NamedRule, RuleDef, RuleSet};
