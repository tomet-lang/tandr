//! Pattern matching engine for tandr.
//!
//! Matches structural patterns against tomet AST elements and extracts variable bindings.

use std::collections::BTreeMap;
use tomet_ast::{Document, Element, ElementValue, Entry, Span, Value};
use tomet_semantics::classify_std_lenient;
use tomet_tree::for_each_element;

use crate::cst::{ElementIndex, EntrySource, Parts};
use crate::pattern::{
    ArgEntryPattern, ElementPattern, Pattern, SigilKind, SlotPattern, TagPattern, ValuePattern,
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

/// Finds all matches for `pattern` in `doc`. `index` is the CST of the same `src`: a
/// capture is the text as written there, not a rendering of the AST.
pub fn find_matches(
    doc: &Document,
    pattern: &Pattern,
    src: &str,
    index: &ElementIndex,
) -> Vec<MatchResult> {
    let mut results = Vec::new();

    match pattern {
        Pattern::Element(el_pat) => {
            for_each_element(doc, |el| {
                if let Some(bindings) =
                    match_element(el_pat, el, src, index.get(el.span.start..el.span.end))
                {
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
/// `parts` says where the element's entries are written; without it a capture is
/// the AST value rendered again, which loses how it was written.
pub fn match_element(
    pat: &ElementPattern,
    el: &Element,
    src: &str,
    parts: Option<&Parts>,
) -> Option<Bindings> {
    let mut bindings = Bindings::new();

    // 1. Sigil and tag name matching: a pattern matches only the sigil it is written with,
    // so an `@name` pattern never matches a `::name` declaration about the document, and
    // a `::name` pattern never matches content.
    if el.sigil.is_document() != (pat.sigil == SigilKind::Document) {
        return None;
    }
    let tag_name: String = match el.sigil.name() {
        Some(name) => name.name.clone(),
        None => classify_std_lenient(el).as_str().to_string(),
    };

    match &pat.tag {
        TagPattern::Exact(expected) => {
            if !tag_name.eq_ignore_ascii_case(expected) {
                return None;
            }
        }
        TagPattern::Var(var_name) => {
            bindings.insert(var_name.clone(), tag_name);
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

        let written = parts.map_or(&[][..], |p| p.data_entries.as_slice());
        let mut matched_keys = Vec::new();

        for (field_key, val_pat) in &data_pat.fields {
            let found = entries.iter().find_map(|e| match e {
                Entry::Pair(k, v) if k == field_key => Some(v),
                _ => None,
            });

            if !match_value(
                val_pat,
                found?,
                text_of(src, written, Some(field_key)),
                &mut bindings,
            ) {
                return None;
            }
            matched_keys.push(field_key.as_str());
        }

        // Rest capture for `{ ...?rest }`
        if let Some(rest_var) = &data_pat.rest_var {
            let mut remaining = Vec::new();
            for e in entries {
                if let Entry::Pair(k, v) = e
                    && !matched_keys.contains(&k.as_str())
                {
                    remaining.push(match entry_of(written, Some(k)) {
                        Some(e) => src[e.whole.clone()].to_string(),
                        None => format!("{k}: {}", value_to_source(v)),
                    });
                }
            }
            bindings.insert(rest_var.clone(), remaining.join(", "));
        }
    }

    // 4. Args `(...)` matching
    if let Some(args_pat) = &pat.args {
        let actual_args = el.args.as_ref()?;
        let written = parts.map_or(&[][..], |p| p.args_entries.as_slice());
        for entry_pat in &args_pat.entries {
            match entry_pat {
                ArgEntryPattern::Named(key, val_pat) => match actual_args {
                    Value::Map(entries) => {
                        let found = entries.iter().find(|(k, _)| k == key).map(|(_, v)| v);
                        if !match_value(
                            val_pat,
                            found?,
                            text_of(src, written, Some(key)),
                            &mut bindings,
                        ) {
                            return None;
                        }
                    }
                    _ => return None,
                },
                ArgEntryPattern::Positional(val_pat) => {
                    // Single scalar value or call argument
                    if !match_value(
                        val_pat,
                        actual_args,
                        text_of(src, written, None),
                        &mut bindings,
                    ) {
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

/// The entry of a group written with `key`; `None` is the first positional one.
fn entry_of<'a>(written: &'a [EntrySource], key: Option<&str>) -> Option<&'a EntrySource> {
    written.iter().find(|e| e.key.as_deref() == key)
}

/// The value of that entry exactly as written.
fn text_of<'a>(src: &'a str, written: &[EntrySource], key: Option<&str>) -> Option<&'a str> {
    entry_of(written, key).map(|e| &src[e.value.clone()])
}

/// `text` is how `actual` is written in the source, when known.
fn match_value(
    pat: &ValuePattern,
    actual: &Value,
    text: Option<&str>,
    bindings: &mut Bindings,
) -> bool {
    match pat {
        ValuePattern::Any => true,
        ValuePattern::Var(v) => {
            bindings.insert(
                v.clone(),
                text.map_or_else(|| value_to_source(actual), str::to_string),
            );
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
    if let (Some(s), Some(e)) = (start, end)
        && s <= e
        && e <= src.len()
    {
        return src[s..e].trim().to_string();
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
        let pat = parse_pattern("@meta{ type: ?(kind), ?(...rest) }").unwrap();

        let matches = find_matches(
            &doc,
            &pat,
            src,
            &ElementIndex::new(&tomet_parser::parse_cst(src)),
        );
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
        let pat = parse_pattern("@link(url: ?(u))[ ?(text) ]").unwrap();

        let matches = find_matches(
            &doc,
            &pat,
            src,
            &ElementIndex::new(&tomet_parser::parse_cst(src)),
        );
        assert_eq!(matches.len(), 1);
        let b = &matches[0].bindings;
        assert_eq!(b.get("u").unwrap(), "\"https://example.com\"");
        assert_eq!(b.get("text").unwrap(), "Visit Here");
    }
}
