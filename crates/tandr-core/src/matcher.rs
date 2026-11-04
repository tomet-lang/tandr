//! Pattern matching engine for tandr.
//!
//! Matches structural patterns against tomet AST elements and extracts variable bindings.

use std::collections::BTreeMap;
use tomet_ast::{Block, Declaration, Element, Entry, File, Group, Value};
use tomet_span::Span;
use tomet_tree::{for_each_declaration, for_each_element};
use tomet_type::Type;

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

/// Finds all matches for `pattern` in `file`. `index` is the CST of the same `src`: a
/// capture is the text as written there, not a rendering of the AST.
pub fn find_matches(
    file: &File,
    pattern: &Pattern,
    src: &str,
    index: &ElementIndex,
) -> Vec<MatchResult> {
    let mut results = Vec::new();

    match pattern {
        Pattern::Element(el_pat) => match el_pat.sigil {
            SigilKind::At => {
                for_each_element(file, |el| {
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
            SigilKind::Document => {
                for_each_declaration(file, |decl| {
                    if let Some(bindings) = match_declaration(
                        el_pat,
                        decl,
                        src,
                        index.get(decl.span.start..decl.span.end),
                    ) {
                        results.push(MatchResult {
                            span: decl.span,
                            bindings,
                        });
                    }
                });
            }
        },
    }

    results
}

/// Checks if an individual AST element matches `pat` and extracts variable bindings.
pub fn match_element(
    pat: &ElementPattern,
    el: &Element,
    src: &str,
    parts: Option<&Parts>,
) -> Option<Bindings> {
    if pat.sigil != SigilKind::At {
        return None;
    }
    let tag = el.name.to_string();
    match_fields(
        pat,
        &tag,
        el.id.as_ref().map(|id| id.0.as_str()),
        &el.data,
        el.content.as_deref(),
        src,
        parts,
    )
}

/// Checks if an individual AST declaration matches `pat` and extracts variable bindings.
pub fn match_declaration(
    pat: &ElementPattern,
    decl: &Declaration,
    src: &str,
    parts: Option<&Parts>,
) -> Option<Bindings> {
    if pat.sigil != SigilKind::Document {
        return None;
    }
    let tag = decl.name.to_string();
    match_fields(
        pat,
        &tag,
        None,
        &decl.data,
        decl.content.as_deref(),
        src,
        parts,
    )
}

fn match_fields(
    pat: &ElementPattern,
    tag_name: &str,
    id: Option<&str>,
    data: &tomet_ast::Data,
    content: Option<&[Block]>,
    src: &str,
    parts: Option<&Parts>,
) -> Option<Bindings> {
    let mut bindings = Bindings::new();

    // 1. Tag name matching
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
        match (id_pat, id) {
            (SlotPattern::Exact(expected), Some(actual_id)) => {
                if actual_id != expected {
                    return None;
                }
            }
            (SlotPattern::Var(v), Some(actual_id)) => {
                bindings.insert(v.clone(), actual_id.to_string());
            }
            (SlotPattern::Any, Some(_)) => {}
            _ => return None,
        }
    }

    // 3. Data `{...}` matching
    if let Some(data_pat) = &pat.data {
        let entries = match &data.value {
            Some(Group(entries)) => entries,
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

        if let Some(rest_var) = &data_pat.rest_var {
            let fallback = || {
                entries
                    .iter()
                    .filter_map(|e| match e {
                        Entry::Pair(k, v) if !matched_keys.contains(&k.as_str()) => {
                            Some(format!("{k}: {}", value_to_source(v)))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            let rest = rest_of_group(written, entries.len(), &matched_keys, src, fallback);
            bindings.insert(rest_var.clone(), rest);
        }
    }

    // 4. Args `(...)` matching
    if let Some(args_pat) = &pat.args {
        let args = data.args.as_ref()?;

        let written = parts.map_or(&[][..], |p| p.args_entries.as_slice());
        let mut matched_keys = Vec::new();

        for entry_pat in &args_pat.entries {
            match entry_pat {
                ArgEntryPattern::Named(expected_key, val_pat) => match args {
                    Value::Map(entries) => {
                        let found = entries
                            .iter()
                            .find_map(|(k, v)| if k == expected_key { Some(v) } else { None });

                        if !match_value(
                            val_pat,
                            found?,
                            text_of(src, written, Some(expected_key)),
                            &mut bindings,
                        ) {
                            return None;
                        }
                        matched_keys.push(expected_key.as_str());
                    }
                    _ => return None,
                },
                ArgEntryPattern::Positional(val_pat) => match args {
                    Value::Map(entries) => {
                        let found = entries
                            .iter()
                            .find_map(|(k, v)| if k.is_empty() { Some(v) } else { None });
                        if !match_value(val_pat, found?, text_of(src, written, None), &mut bindings)
                        {
                            return None;
                        }
                    }
                    _ => {
                        if !match_value(val_pat, args, text_of(src, written, None), &mut bindings) {
                            return None;
                        }
                    }
                },
            }
        }

        if let Some(rest_var) = &args_pat.rest_var {
            let actual_len = match args {
                Value::Map(entries) => entries.len(),
                _ => 1,
            };
            let fallback = || match args {
                Value::Map(entries) => entries
                    .iter()
                    .filter_map(|(k, v)| {
                        if matched_keys.contains(&k.as_str()) {
                            None
                        } else if k.is_empty() {
                            Some(value_to_source(v))
                        } else {
                            Some(format!("{k}: {}", value_to_source(v)))
                        }
                    })
                    .collect::<Vec<_>>(),
                other => vec![value_to_source(other)],
            };
            let rest = rest_of_group(written, actual_len, &matched_keys, src, fallback);
            bindings.insert(rest_var.clone(), rest);
        }
    }

    // 5. Content `[...]` matching
    if let Some(content_pat) = &pat.content {
        let blocks = content?;
        let content_str = extract_content_text(src, blocks);

        match content_pat {
            SlotPattern::Exact(expected) => {
                if content_str != *expected {
                    return None;
                }
            }
            SlotPattern::Var(v) => {
                bindings.insert(v.clone(), content_str);
            }
            SlotPattern::Any => {}
        }
    }

    Some(bindings)
}

/// The entries of a group that were not matched, formatted as written: commas, line
/// breaks and indentation kept, comments about dropped entries dropped, and comments
/// about kept entries kept.
fn rest_of_group(
    written: &[EntrySource],
    actual_len: usize,
    matched_keys: &[&str],
    src: &str,
    fallback: impl FnOnce() -> Vec<String>,
) -> String {
    if written.is_empty() {
        return fallback().join(", ");
    }
    let kept: Vec<(usize, &EntrySource)> = written
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.key.as_deref().is_some_and(|k| matched_keys.contains(&k)))
        .collect();
    if kept.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut previous: Option<usize> = None;
    for (i, entry) in kept {
        if let Some(prev) = previous {
            let gap = &src[written[prev].whole.end..entry.whole.start];
            let gap = if prev + 1 == i {
                gap.to_string()
            } else {
                plain_gap(gap)
            };
            out.push_str(&gap);
        }
        out.push_str(&src[entry.whole.clone()]);
        previous = Some(i);
    }
    let _ = actual_len;
    out
}

/// A separator without comments: its comma if it had one, then its line break and indentation
/// (or `", "` between two entries on one line).
fn plain_gap(gap: &str) -> String {
    if gap.chars().all(|c| c.is_whitespace() || c == ',') {
        return gap.to_string();
    }
    let comma = if gap.trim_start().starts_with(',') {
        ","
    } else {
        ""
    };
    match gap.rfind('\n') {
        Some(nl) => format!("{comma}{}", &gap[nl..]),
        None => ", ".to_string(),
    }
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
        ValuePattern::Typed { ty, var } => {
            if !value_fits_type(ty, actual) {
                return false;
            }
            if let Some(v) = var {
                bindings.insert(
                    v.clone(),
                    text.map_or_else(|| value_to_source(actual), str::to_string),
                );
            }
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
    }
}

/// Whether an AST value conforms to the given tomet type.
fn value_fits_type(ty: &Type, actual: &Value) -> bool {
    let integer = || match actual {
        Value::Int(n) => Some(*n),
        Value::Number(s) => s.parse::<i64>().ok(),
        _ => None,
    };
    match ty {
        Type::Int => integer().is_some(),
        Type::Uint => integer().is_some_and(|n| n >= 0),
        Type::Float => matches!(actual, Value::Int(_)) || actual.as_number().is_some(),
        Type::Bool => matches!(actual, Value::Bool(_)),
        Type::List(inner) => match actual {
            Value::Seq(items) => {
                if let Some(inner_ty) = inner {
                    items.iter().all(|item| value_fits_type(inner_ty, item))
                } else {
                    true
                }
            }
            _ => false,
        },
        Type::String | Type::Nanoid => actual.scalar_text().is_some(),
        Type::Datetime | Type::Version | Type::Uuid => actual
            .scalar_text()
            .is_some_and(|text| ty.holds_text(&text) == Some(true)),
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

fn extract_content_text(src: &str, blocks: &[Block]) -> String {
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

    #[test]
    fn matches_meta_and_captures_fields() {
        let src = "@meta{ type: \"article\", title: \"My Post\", author: \"Alice\" }\n";
        let file = tomet_ast_parser::parse_file(src).unwrap();
        let pat = parse_pattern("@meta{ type: <?(kind)>, <?rest(rest)> }").unwrap();

        let matches = find_matches(
            &file,
            &pat,
            src,
            &ElementIndex::new(&tomet_cst_parser::parse_cst(src)),
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
        let file = tomet_ast_parser::parse_file(src).unwrap();
        let pat = parse_pattern("@link(url: <?(u)>)[ <?(text)> ]").unwrap();

        let matches = find_matches(
            &file,
            &pat,
            src,
            &ElementIndex::new(&tomet_cst_parser::parse_cst(src)),
        );
        assert_eq!(matches.len(), 1);
        let b = &matches[0].bindings;
        assert_eq!(b.get("u").unwrap(), "\"https://example.com\"");
        assert_eq!(b.get("text").unwrap(), "Visit Here");
    }

    #[test]
    fn typed_holes_match_int_and_uint() {
        let src = "@counter(val: 42) @neg(val: -5) @text(val: \"hello\")\n";
        let file = tomet_ast_parser::parse_file(src).unwrap();
        let index = ElementIndex::new(&tomet_cst_parser::parse_cst(src));

        let pat_int = parse_pattern("<?element(tag, val: <?int(n)>)>").unwrap();
        let matches = find_matches(&file, &pat_int, src, &index);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].bindings.get("tag").unwrap(), "counter");
        assert_eq!(matches[0].bindings.get("n").unwrap(), "42");
        assert_eq!(matches[1].bindings.get("tag").unwrap(), "neg");
        assert_eq!(matches[1].bindings.get("n").unwrap(), "-5");

        let pat_uint = parse_pattern("<?element(tag, val: <?uint(u)>)>").unwrap();
        let matches = find_matches(&file, &pat_uint, src, &index);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].bindings.get("tag").unwrap(), "counter");
        assert_eq!(matches[0].bindings.get("u").unwrap(), "42");
    }

    #[test]
    fn typed_holes_match_version() {
        let src = "@dep(ver: 1.10.0) @dep(ver: \"2.0.1\") @bad(ver: \"not-a-ver\")\n";
        let file = tomet_ast_parser::parse_file(src).unwrap();
        let index = ElementIndex::new(&tomet_cst_parser::parse_cst(src));

        let pat_ver = parse_pattern("@dep(ver: <?version(v)>)").unwrap();
        let matches = find_matches(&file, &pat_ver, src, &index);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].bindings.get("v").unwrap(), "1.10.0");
        assert_eq!(matches[1].bindings.get("v").unwrap(), "\"2.0.1\"");

        // Anonymous version hole
        let pat_anon = parse_pattern("@dep(ver: <?version>)").unwrap();
        let matches = find_matches(&file, &pat_anon, src, &index);
        assert_eq!(matches.len(), 2);
        assert!(matches[0].bindings.is_empty());
    }

    #[test]
    fn typed_holes_match_uuid_and_datetime() {
        let src = "@item(id: \"123e4567-e89b-12d3-a456-426614174000\") @item(id: \"invalid\") @event(at: 2026-10-25) @event(at: \"bad-date\")\n";
        let file = tomet_ast_parser::parse_file(src).unwrap();
        let index = ElementIndex::new(&tomet_cst_parser::parse_cst(src));

        let pat_uuid = parse_pattern("@item(id: <?uuid(id)>)").unwrap();
        let matches = find_matches(&file, &pat_uuid, src, &index);
        assert_eq!(matches.len(), 1);
        assert_eq!(
            matches[0].bindings.get("id").unwrap(),
            "\"123e4567-e89b-12d3-a456-426614174000\""
        );

        let pat_dt = parse_pattern("@event(at: <?datetime(d)>)").unwrap();
        let matches = find_matches(&file, &pat_dt, src, &index);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].bindings.get("d").unwrap(), "2026-10-25");
    }

    #[test]
    fn typed_holes_match_bool_and_float() {
        let src = "@flag(ok: true) @flag(ok: false) @flag(ok: \"yes\") @num(v: 3.14) @num(v: 100) @num(v: \"str\")\n";
        let file = tomet_ast_parser::parse_file(src).unwrap();
        let index = ElementIndex::new(&tomet_cst_parser::parse_cst(src));

        let pat_bool = parse_pattern("@flag(ok: <?bool(b)>)").unwrap();
        let matches = find_matches(&file, &pat_bool, src, &index);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].bindings.get("b").unwrap(), "true");
        assert_eq!(matches[1].bindings.get("b").unwrap(), "false");

        let pat_float = parse_pattern("@num(v: <?float(f)>)").unwrap();
        let matches = find_matches(&file, &pat_float, src, &index);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].bindings.get("f").unwrap(), "3.14");
        assert_eq!(matches[1].bindings.get("f").unwrap(), "100");
    }

    #[test]
    fn typed_holes_match_list() {
        let src = "@tags(items: list(\"a\", \"b\")) @tags(items: \"not-a-list\")\n";
        let file = tomet_ast_parser::parse_file(src).unwrap();
        let index = ElementIndex::new(&tomet_cst_parser::parse_cst(src));

        let pat_list = parse_pattern("@tags(items: <?list(l)>)").unwrap();
        let matches = find_matches(&file, &pat_list, src, &index);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].bindings.get("l").unwrap(), "list(\"a\", \"b\")");
    }
}
