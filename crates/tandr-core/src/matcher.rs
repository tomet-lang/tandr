//! Pattern matching engine for tandr.
//!
//! Matches structural patterns against tomet AST elements and extracts variable bindings.

use std::collections::BTreeMap;
use tomet_ast::{Document, Element, ElementValue, Entry, Span, Value};
use tomet_semantics::classify_std_lenient;
use tomet_tree::for_each_element;

use crate::pattern::{
    ArgEntryPattern, ElementPattern, Pattern, SlotPattern, TagPattern, ValuePattern,
};

/// Map of captured variable names to their extracted string representations.
pub type Bindings = BTreeMap<String, String>;

/// A matched occurrence in a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchResult {
    /// Byte span in the source text of the matched node.
    pub span: Span,
    /// Variables captured during matching.
    pub bindings: Bindings,
}

/// Finds all matches for `pattern` in `doc`.
pub fn find_matches(doc: &Document, pattern: &Pattern, src: &str) -> Vec<MatchResult> {
    let mut results = Vec::new();

    match pattern {
        Pattern::Element(el_pat) => {
            for_each_element(doc, |el| {
                if let Some(bindings) = match_element(el_pat, el, src) {
                    results.push(MatchResult {
                        span: el.span,
                        bindings,
                    });
                }
            });
        }
    }

    results
}

/// Checks if an individual AST element matches `pat` and extracts variable bindings.
pub fn match_element(pat: &ElementPattern, el: &Element, src: &str) -> Option<Bindings> {
    let mut bindings = Bindings::new();

    // 1. Tag name matching
    let kind = classify_std_lenient(el);
    let tag_name = if el.sigil.name().is_some() {
        el.sigil.name().unwrap().name.as_str()
    } else {
        kind.as_str()
    };

    match &pat.tag {
        TagPattern::Exact(expected) => {
            if !tag_name.eq_ignore_ascii_case(expected) {
                return None;
            }
        }
        TagPattern::Var(var_name) => {
            bindings.insert(var_name.clone(), tag_name.to_string());
        }
        TagPattern::Any => {}
    }

    // 2. ID matching
    if let Some(id_pat) = &pat.id {
        match (id_pat, &el.id) {
            (SlotPattern::Exact(expected), Some(id)) => {
                if &id.0 != expected {
                    return None;
                }
            }
            (SlotPattern::Var(v), Some(id)) => {
                bindings.insert(v.clone(), id.0.clone());
            }
            (SlotPattern::Any, Some(_)) => {}
            _ => return None,
        }
    }

    // 3. Data `{...}` matching
    if let Some(data_pat) = &pat.data {
        let entries = match &el.value {
            Some(ElementValue::Group(entries)) => entries,
            _ => return None,
        };

        let mut matched_keys = Vec::new();

        for (field_key, val_pat) in &data_pat.fields {
            let found = entries.iter().find_map(|e| match e {
                Entry::Pair(k, v) if k == field_key => Some(v),
                _ => None,
            });

            match found {
                Some(actual_val) => {
                    if !match_value(val_pat, actual_val, &mut bindings) {
                        return None;
                    }
                    matched_keys.push(field_key.as_str());
                }
                None => return None,
            }
        }

        // Rest capture for `{ ...?rest }`
        if let Some(rest_var) = &data_pat.rest_var {
            let mut remaining = Vec::new();
            for e in entries {
                if let Entry::Pair(k, v) = e {
                    if !matched_keys.contains(&k.as_str()) {
                        remaining.push(format!("{k}: {}", value_to_source(v)));
                    }
                }
            }
            bindings.insert(rest_var.clone(), remaining.join(", "));
        }
    }

    // 4. Args `(...)` matching
    if let Some(args_pat) = &pat.args {
        let actual_args = el.args.as_ref()?;
        for entry_pat in &args_pat.entries {
            match entry_pat {
                ArgEntryPattern::Named(key, val_pat) => match actual_args {
                    Value::Map(entries) => {
                        let found = entries.iter().find(|(k, _)| k == key).map(|(_, v)| v);
                        match found {
                            Some(v) => {
                                if !match_value(val_pat, v, &mut bindings) {
                                    return None;
                                }
                            }
                            None => return None,
                        }
                    }
                    _ => return None,
                },
                ArgEntryPattern::Positional(val_pat) => {
                    // Single scalar value or call argument
                    if !match_value(val_pat, actual_args, &mut bindings) {
                        return None;
                    }
                }
            }
        }
    }

    // 5. Content `[...]` matching
    if let Some(content_pat) = &pat.content {
        let content_blocks = el.content.as_ref()?;
        // Extract raw slice from source using element's span if available
        let content_text = extract_content_text(el, src, content_blocks);
        match content_pat {
            SlotPattern::Exact(expected) => {
                if content_text.trim() != expected.trim() {
                    return None;
                }
            }
            SlotPattern::Var(v) => {
                bindings.insert(v.clone(), content_text);
            }
            SlotPattern::Any => {}
        }
    }

    Some(bindings)
}

fn match_value(pat: &ValuePattern, actual: &Value, bindings: &mut Bindings) -> bool {
    match pat {
        ValuePattern::Any => true,
        ValuePattern::Var(v) => {
            bindings.insert(v.clone(), value_to_source(actual));
            true
        }
        ValuePattern::String(expected) => match actual {
            Value::String(s) => s == expected,
            _ => false,
        },
        ValuePattern::Number(expected) => match actual {
            Value::Int(n) => n == expected,
            _ => false,
        },
        ValuePattern::Bool(expected) => match actual {
            Value::Bool(b) => b == expected,
            _ => false,
        },
        ValuePattern::Ident(expected) => match actual {
            Value::String(s) => s == expected,
            _ => false,
        },
    }
}

fn value_to_source(v: &Value) -> String {
    match v {
        Value::String(s) => format!("\"{s}\""),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Call(name, args) => {
            let rendered: Vec<_> = args.iter().map(value_to_source).collect();
            format!("{name}({})", rendered.join(", "))
        }
        Value::Seq(items) => {
            let rendered: Vec<_> = items.iter().map(value_to_source).collect();
            format!("list({})", rendered.join(", "))
        }
        Value::Map(entries) => {
            let rendered: Vec<_> = entries
                .iter()
                .map(|(k, val)| format!("{k}: {}", value_to_source(val)))
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
        _ => String::new(),
    }
}

fn extract_content_text(_el: &Element, src: &str, blocks: &[tomet_ast::Block]) -> String {
    // If blocks have spans, take the union of their spans
    let start = blocks.first().map(|b| b.span().start);
    let end = blocks.last().map(|b| b.span().end);
    if let (Some(s), Some(e)) = (start, end) {
        if s <= e && e <= src.len() {
            return src[s..e].trim().to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::parse_pattern;
    use tomet_parser::parse_document;

    #[test]
    fn matches_meta_and_captures_fields() {
        let src = "@meta{ type: \"article\", title: \"My Post\", author: \"Alice\" }\n";
        let doc = parse_document(src).unwrap();
        let pat = parse_pattern("@meta{ type: ?kind, ...?rest }").unwrap();

        let matches = find_matches(&doc, &pat, src);
        assert_eq!(matches.len(), 1);
        let b = &matches[0].bindings;
        assert_eq!(b.get("kind").unwrap(), "\"article\"");
        assert!(b.get("rest").unwrap().contains("title: \"My Post\""));
        assert!(b.get("rest").unwrap().contains("author: \"Alice\""));
    }

    #[test]
    fn matches_link_with_args_and_content() {
        let src = "Check @link(url: \"https://example.com\")[ Visit Here ] today.\n";
        let doc = parse_document(src).unwrap();
        let pat = parse_pattern("@link(url: ?u)[ ?text ]").unwrap();

        let matches = find_matches(&doc, &pat, src);
        assert_eq!(matches.len(), 1);
        let b = &matches[0].bindings;
        assert_eq!(b.get("u").unwrap(), "\"https://example.com\"");
        assert_eq!(b.get("text").unwrap(), "Visit Here");
    }
}
