//! Structural patterns ("tomet with holes"), and how tandr reads them.
//!
//! Examples:
//! - `@meta{ type: <?(kind)>, <?rest(r)> }`
//! - `@link(url: <?(u)>)[ <?(text)> ]`
//! - `@task(<?(status)>)[ <?(body)> ]`
//!
//! tomet reads the text: [`parse_pattern`] hands it to `tomet_ast_parser::parse_file`,
//! which builds the pattern as an AST with holes in it, and lowers that tree
//! to the [`ElementPattern`] the matcher works on. A hole is `<?(name)>`, `<?>` / `<?()>`
//! (anything, bound to nothing) or `<?rest(name)>` (the rest of a group); see
//! [`crate::hole`].

use std::collections::BTreeSet;
use std::fmt;

use tomet_ast::{Block, Declaration, Element, Entry, Hole as AstHole, Inline, Value};
use tomet_type::Type;

use crate::hole::{self, Hole};

/// A structural pattern to match against tomet documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// Matches an element `@name(args){data}[content]#(id)` or `::name(args){data}[content]#(id)`.
    Element(ElementPattern),
}

/// Which sigil an element pattern is written with. A pattern matches only the
/// elements written with the same sigil: an `@name` pattern never matches a
/// `::name` declaration, and the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SigilKind {
    /// `@name`: content
    #[default]
    At,
    /// `::name`: a declaration about the document
    Document,
}

/// Pattern for matching an element.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ElementPattern {
    pub sigil: SigilKind,
    /// Tag name matcher (e.g. `meta`, `link`).
    pub tag: TagPattern,
    /// ID slot matcher `#(id)` or `#id`.
    pub id: Option<SlotPattern>,
    /// Args matcher `(args)`.
    pub args: Option<ArgsPattern>,
    /// Data matcher `{key: val}`.
    pub data: Option<DataPattern>,
    /// Content matcher `[content]`.
    pub content: Option<SlotPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TagPattern {
    #[default]
    Any,
    Exact(String),
    Var(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotPattern {
    Any,
    Exact(String),
    Var(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ArgsPattern {
    pub entries: Vec<ArgEntryPattern>,
    pub rest_var: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgEntryPattern {
    Positional(ValuePattern),
    Named(String, ValuePattern),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DataPattern {
    pub fields: Vec<(String, ValuePattern)>,
    pub rest_var: Option<String>,
}

/// A value in a pattern. tomet does not keep whether a string was quoted, so a
/// bare word and a quoted string are both [`ValuePattern::String`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValuePattern {
    Any,
    Var(String),
    Typed { ty: Type, var: Option<String> },
    String(String),
    Number(i64),
    Bool(bool),
}

impl ValuePattern {
    /// The variable name bound by this value pattern, if any.
    pub fn bound_name(&self) -> Option<&str> {
        match self {
            ValuePattern::Var(n) => Some(n),
            ValuePattern::Typed { var: Some(n), .. } => Some(n),
            _ => None,
        }
    }
}

impl fmt::Display for ValuePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValuePattern::Any => write!(f, "<?>"),
            ValuePattern::Var(v) => write!(f, "<?({v})>"),
            ValuePattern::Typed { ty, var: Some(v) } => write!(f, "<?{ty}({v})>"),
            ValuePattern::Typed { ty, var: None } => write!(f, "<?{ty}>"),
            ValuePattern::String(s) => write!(f, "\"{s}\""),
            ValuePattern::Number(n) => write!(f, "{n}"),
            ValuePattern::Bool(b) => write!(f, "{b}"),
        }
    }
}

/// Parses a pattern string into a [`Pattern`].
pub fn parse_pattern(src: &str) -> Result<Pattern, String> {
    let file = tomet_ast_parser::parse_file(src).map_err(|e| e.to_string())?;

    // Check header declarations vs body blocks.
    if !file.header.is_empty() {
        if file.header.len() == 1 && file.body.is_empty() {
            return lower_declaration(&file.header[0], src).map(Pattern::Element);
        }
        return Err("a pattern is one element".to_string());
    }

    match file.body.as_slice() {
        [Block::Element(el)] => {
            if !src[el.span.start..].starts_with('@') {
                return Err("Pattern must start with '@' or '::'; a code block pattern is not implemented yet".to_string());
            }
            lower_element(el, src).map(Pattern::Element)
        }
        [Block::Hole(h)] => lower_hole_block(h, src).map(Pattern::Element),
        [Block::Paragraph(p)] => {
            let hint = match p.content.as_slice() {
                [Inline::Text(t), Inline::Hole(_), ..] if t.value.ends_with('@') => {
                    "; a hole cannot stand for the tag (`@<?(tag)>`: tomet reads the `@` there as \
                     text), write <?element(tag)> for any element"
                }
                _ => "",
            };
            Err(format!("Pattern must start with '@' or '::'{hint}"))
        }
        [] => Err("Pattern must start with '@' or '::' (it is empty)".to_string()),
        _ => Err("a pattern is one element".to_string()),
    }
}

impl Pattern {
    /// Every variable name this pattern binds. A template cannot insert a name
    /// that is not here.
    pub fn bound_names(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let Pattern::Element(el) = self;
        if let TagPattern::Var(n) = &el.tag {
            names.insert(n.clone());
        }
        if let Some(SlotPattern::Var(n)) = &el.id {
            names.insert(n.clone());
        }
        if let Some(SlotPattern::Var(n)) = &el.content {
            names.insert(n.clone());
        }
        if let Some(args) = &el.args {
            for entry in &args.entries {
                let v = match entry {
                    ArgEntryPattern::Positional(v) | ArgEntryPattern::Named(_, v) => v,
                };
                if let Some(n) = v.bound_name() {
                    names.insert(n.to_string());
                }
            }
            names.extend(args.rest_var.clone());
        }
        if let Some(data) = &el.data {
            for (_, val) in &data.fields {
                if let Some(n) = val.bound_name() {
                    names.insert(n.to_string());
                }
            }
            names.extend(data.rest_var.clone());
        }
        names
    }
}

/// The pattern declaration `decl` (`::name`) as the matcher wants it.
fn lower_declaration(decl: &Declaration, src: &str) -> Result<ElementPattern, String> {
    let args = decl.data.args.as_ref().map(lower_args).transpose()?;
    let data = decl
        .data
        .value
        .as_ref()
        .map(|g| lower_data(&g.0))
        .transpose()?;
    let content = decl
        .content
        .as_ref()
        .map(|blocks| lower_content(blocks, src))
        .transpose()?;
    Ok(ElementPattern {
        sigil: SigilKind::Document,
        tag: TagPattern::Exact(decl.name.to_string()),
        id: None,
        args,
        data,
        content,
    })
}

/// The pattern element `el` (`@name`) as the matcher wants it.
fn lower_element(el: &Element, src: &str) -> Result<ElementPattern, String> {
    if !el.connects.is_empty() {
        return Err("a connect (`:name(...)`) in a pattern is not implemented yet".to_string());
    }
    let id = el
        .id
        .as_ref()
        .map(|id| {
            if id.0.contains('?') {
                Err(format!(
                    "`#({})`: an id cannot be a hole; write the id itself",
                    id.0
                ))
            } else {
                Ok(SlotPattern::Exact(id.0.clone()))
            }
        })
        .transpose()?;
    let args = el.data.args.as_ref().map(lower_args).transpose()?;
    let data = el
        .data
        .value
        .as_ref()
        .map(|g| lower_data(&g.0))
        .transpose()?;
    let content = el
        .content
        .as_ref()
        .map(|blocks| lower_content(blocks, src))
        .transpose()?;
    Ok(ElementPattern {
        sigil: SigilKind::At,
        tag: TagPattern::Exact(el.name.to_string()),
        id,
        args,
        data,
        content,
    })
}

/// A top-level hole standing for any element or declaration (`<?element>` / `<?declaration>`).
fn lower_hole_block(h: &AstHole, src: &str) -> Result<ElementPattern, String> {
    match hole::read(h)? {
        Hole::Element {
            declaration,
            tag,
            args,
        } => {
            let sigil = if declaration {
                SigilKind::Document
            } else {
                SigilKind::At
            };
            let tag = tag.map_or(TagPattern::Any, TagPattern::Var);
            let args = args.as_ref().map(lower_args).transpose()?;
            let data = h
                .data
                .value
                .as_ref()
                .map(|g| lower_data(&g.0))
                .transpose()?;
            let content = h
                .content
                .as_ref()
                .map(|blocks| lower_content(blocks, src))
                .transpose()?;
            Ok(ElementPattern {
                sigil,
                tag,
                id: None,
                args,
                data,
                content,
            })
        }
        _ => Err("Pattern must start with '@' or '::'".to_string()),
    }
}

/// `( )`: a map reads as its entries, a single value as one positional entry.
fn lower_args(args: &Value) -> Result<ArgsPattern, String> {
    let mut out = ArgsPattern::default();
    let mut put = |key: Option<&str>, value: &Value| -> Result<(), String> {
        // The rest stands for entries, so it has no key: `a: <?rest(r)>` is refused below.
        if key.is_none()
            && let Some(name) = rest_of(value)?
        {
            if out.rest_var.replace(name).is_some() {
                return Err("a group has one rest, <?rest(name)>".to_string());
            }
            return Ok(());
        }
        let pattern = lower_value(value)?;
        out.entries.push(match key {
            Some(key) => ArgEntryPattern::Named(key.to_string(), pattern),
            None => ArgEntryPattern::Positional(pattern),
        });
        Ok(())
    };
    match args {
        Value::Map(entries) => {
            for (key, value) in entries {
                // tomet keys an entry written without a key by the empty string
                // (`POSITIONAL_ENTRY_KEY` in cotomet-syntax).
                put((!key.is_empty()).then_some(key.as_str()), value)?;
            }
        }
        single => put(None, single)?,
    }
    Ok(out)
}

/// `{ }`: `key: value` entries, and the rest of the group.
fn lower_data(entries: &[Entry]) -> Result<DataPattern, String> {
    let mut out = DataPattern::default();
    for entry in entries {
        match entry {
            Entry::Pair(key, value) => out.fields.push((key.clone(), lower_value(value)?)),
            Entry::Hole(h) => match hole::read(h)? {
                Hole::Rest(name) => {
                    if out.rest_var.replace(name).is_some() {
                        return Err("a group has one rest, <?rest(name)>".to_string());
                    }
                }
                Hole::Bind(_) | Hole::Typed { .. } => {
                    return Err(
                        "a hole alone in `{ }` has no key; write `key: <?...>`, or <?rest(name)> \
                         for the rest"
                            .to_string(),
                    );
                }
                Hole::Element { .. } => return Err(WHOLE_ELEMENT.to_string()),
            },
            Entry::Bare(_) | Entry::Element(_) => {
                return Err(
                    "an element in the `{ }` of a pattern is not implemented yet".to_string(),
                );
            }
        }
    }
    Ok(out)
}

/// The name of the rest if `value` is `<?rest(name)>`.
fn rest_of(value: &Value) -> Result<Option<String>, String> {
    match value {
        Value::Hole(h) => match hole::read(h)? {
            Hole::Rest(name) => Ok(Some(name)),
            Hole::Bind(_) | Hole::Typed { .. } => Ok(None),
            Hole::Element { .. } => Err(WHOLE_ELEMENT.to_string()),
        },
        _ => Ok(None),
    }
}

/// Why `<?element(...)>` and `<?declaration(...)>` are refused anywhere but at the head of a pattern.
const WHOLE_ELEMENT: &str = "<?element(...)> and <?declaration(...)> stand for a whole element; \
     they can only be the pattern itself, where `@name` or `::name` would be";

fn lower_value(value: &Value) -> Result<ValuePattern, String> {
    match value {
        Value::Hole(h) => match hole::read(h)? {
            Hole::Bind(Some(name)) => Ok(ValuePattern::Var(name)),
            Hole::Bind(None) => Ok(ValuePattern::Any),
            Hole::Typed { ty, var } => Ok(ValuePattern::Typed { ty, var }),
            Hole::Rest(_) => Err(
                "<?rest(name)> is the rest of a group; it cannot stand for one value".to_string(),
            ),
            Hole::Element { .. } => Err(WHOLE_ELEMENT.to_string()),
        },
        Value::String(s) => Ok(ValuePattern::String(s.clone())),
        Value::Int(n) => Ok(ValuePattern::Number(*n)),
        Value::Bool(b) => Ok(ValuePattern::Bool(*b)),
        other => Err(format!(
            "a value in a pattern is a hole, a string, an integer or a boolean; found {other:?}"
        )),
    }
}

/// `[ ]`: one hole, or text.
fn lower_content(blocks: &[Block], src: &str) -> Result<SlotPattern, String> {
    let mut paragraphs = Vec::new();
    for block in blocks {
        match block {
            Block::Paragraph(p) => paragraphs.push(p),
            _ => {
                return Err(
                    "block content in a pattern is not implemented yet; only text or one hole"
                        .to_string(),
                );
            }
        }
    }
    let holes = paragraphs
        .iter()
        .flat_map(|p| &p.content)
        .filter(|inline| matches!(inline, Inline::Hole(_)))
        .count();
    if holes == 0 {
        // The text as written: the matcher compares it to the content as written.
        let text = match (blocks.first(), blocks.last()) {
            (Some(first), Some(last)) => src
                .get(first.span().start..last.span().end)
                .unwrap_or_default()
                .trim(),
            _ => "",
        };
        return Ok(SlotPattern::Exact(text.to_string()));
    }
    match paragraphs.as_slice() {
        [p] => match p.content.as_slice() {
            [Inline::Hole(h)] => match hole::read(h)? {
                Hole::Bind(Some(name)) => Ok(SlotPattern::Var(name)),
                Hole::Bind(None) => Ok(SlotPattern::Any),
                Hole::Typed { .. } => Err(
                    "a typed hole cannot stand for the content; write <?(name)> or <?>".to_string(),
                ),
                Hole::Rest(_) => Err(
                    "<?rest(name)> is the rest of a group; it cannot stand for the content"
                        .to_string(),
                ),
                Hole::Element { .. } => Err(WHOLE_ELEMENT.to_string()),
            },
            _ => Err("a hole inside text in a pattern is not implemented yet".to_string()),
        },
        _ => Err("a hole inside text in a pattern is not implemented yet".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meta_pattern_with_holes() {
        let p = parse_pattern("@meta{ type: <?(kind)>, title: <?(title)> }").unwrap();
        let Pattern::Element(el) = &p;
        assert_eq!(el.sigil, SigilKind::At);
        assert_eq!(el.tag, TagPattern::Exact("meta".to_string()));
        let data = el.data.as_ref().unwrap();
        assert_eq!(
            data.fields,
            [
                ("type".to_string(), ValuePattern::Var("kind".to_string())),
                ("title".to_string(), ValuePattern::Var("title".to_string())),
            ]
        );
        assert_eq!(
            p.bound_names().into_iter().collect::<Vec<_>>(),
            ["kind", "title"]
        );
    }

    #[test]
    fn anonymous_hole_matches_anything_and_binds_nothing() {
        let p = parse_pattern("@x(<?>, a: <?>)").unwrap();
        let Pattern::Element(el) = &p;
        let args = el.args.as_ref().unwrap();
        assert_eq!(
            args.entries,
            [
                ArgEntryPattern::Positional(ValuePattern::Any),
                ArgEntryPattern::Named("a".to_string(), ValuePattern::Any),
            ]
        );
        assert!(p.bound_names().is_empty());
    }

    #[test]
    fn the_rest_of_a_group_is_a_matcher() {
        let p = parse_pattern("@x(a: 1, <?rest(more)>){ k: <?(v)>, <?rest(rest)> }").unwrap();
        let Pattern::Element(el) = &p;
        assert_eq!(el.args.as_ref().unwrap().rest_var.as_deref(), Some("more"));
        assert_eq!(el.data.as_ref().unwrap().rest_var.as_deref(), Some("rest"));
        assert!(p.bound_names().contains("more"));
        // a lone rest is the whole of the group
        let Pattern::Element(el) = parse_pattern("@x(<?rest(all)>)").unwrap();
        let args = el.args.unwrap();
        assert!(args.entries.is_empty());
        assert_eq!(args.rest_var.as_deref(), Some("all"));
    }

    #[test]
    fn what_tandr_does_not_implement_is_refused_with_why() {
        for (src, hint) in [
            ("@meta{ type: <?foo(n)> }", "`foo`"),
            ("@meta{ <?(...rest)> }", "<?rest(rest)>"),
            ("@meta{ <?(x)> }", "no key"),
            ("@x(<?rest()>)", "anonymous"),
            ("@x(a: <?rest(r)>)", "rest of a group"),
            ("@x[ <?rest(r)> ]", "rest of a group"),
            ("@x[ a <?(b)> ]", "inside text"),
            ("@x[ <?(a)>[b] ]", "not implemented"),
            ("@x#(<?(i)>)", "cannot be a hole"),
            ("@<?(tag)>[ <?(body)> ]", "write <?element(tag)>"),
            ("@x(a: <?element(t)>)", "whole element"),
            ("@x{ <?element(t)> }", "whole element"),
            ("@x{ k: <?declaration(t)> }", "whole element"),
            ("@x[ <?element(t)> ]", "whole element"),
            ("<?(x)>", "Pattern must start"),
            ("@x(a: 1.5)", "found"),
            ("@a @b", "Pattern must start"),
            ("```\n<?(code)>\n```", "code block"),
            ("- [ <?(x)> ]", "one element"),
        ] {
            let err = parse_pattern(src).unwrap_err();
            assert!(err.contains(hint), "{src}: {err}");
        }
    }

    #[test]
    fn an_element_hole_is_any_element_and_binds_its_tag() {
        let p = parse_pattern(
            "<?element(tag, url: <?(u)>, <?rest(more)>){ type: <?(k)> }>[ <?(text)> ]",
        )
        .unwrap();
        let Pattern::Element(el) = &p;
        assert_eq!(el.sigil, SigilKind::At);
        assert_eq!(el.tag, TagPattern::Var("tag".to_string()));
        let args = el.args.as_ref().unwrap();
        assert_eq!(
            args.entries,
            [ArgEntryPattern::Named(
                "url".to_string(),
                ValuePattern::Var("u".to_string())
            )]
        );
        assert_eq!(args.rest_var.as_deref(), Some("more"));
        assert_eq!(el.data.as_ref().unwrap().fields.len(), 1);
        assert_eq!(el.content, Some(SlotPattern::Var("text".to_string())));
        assert_eq!(
            p.bound_names().into_iter().collect::<Vec<_>>(),
            ["k", "more", "tag", "text", "u"]
        );

        // a declaration is the same, for `::`
        let Pattern::Element(el) = parse_pattern("<?declaration(t){ id: <?(i)> }>").unwrap();
        assert_eq!(el.sigil, SigilKind::Document);
        assert_eq!(el.tag, TagPattern::Var("t".to_string()));

        // no name: any tag, nothing bound for it; no `( )`: no constraint on the args
        let Pattern::Element(el) = parse_pattern("<?element()>").unwrap();
        assert_eq!(el.tag, TagPattern::Any);
        assert_eq!(el.args, None);
    }

    #[test]
    fn the_sigil_is_part_of_the_pattern() {
        let Pattern::Element(el) = parse_pattern("::meta{ id: <?(i)> }").unwrap();
        assert_eq!(el.sigil, SigilKind::Document);
        assert_eq!(el.tag, TagPattern::Exact("meta".to_string()));
        let Pattern::Element(el) = parse_pattern("@meta").unwrap();
        assert_eq!(el.sigil, SigilKind::At);
        let err = parse_pattern(":meta").unwrap_err();
        assert!(err.contains("'::'"), "{err}");
    }

    #[test]
    fn a_pattern_without_groups_matches_by_name_alone() {
        let Pattern::Element(el) = parse_pattern("@kind").unwrap();
        assert_eq!(el.args, None);
        assert_eq!(el.data, None);
        assert_eq!(el.content, None);
    }

    #[test]
    fn text_in_content_is_compared_as_written() {
        let Pattern::Element(el) = parse_pattern("@note[ some  text ]").unwrap();
        assert_eq!(
            el.content,
            Some(SlotPattern::Exact("some  text".to_string()))
        );
    }

    #[test]
    fn typed_holes_in_pattern_are_parsed_and_bind_names() {
        let p =
            parse_pattern("@item(count: <?int(n)>, <?uint>){ ver: <?version(v)>, tag: <?string> }")
                .unwrap();
        let Pattern::Element(el) = &p;
        let args = el.args.as_ref().unwrap();
        assert_eq!(
            args.entries,
            [
                ArgEntryPattern::Named(
                    "count".to_string(),
                    ValuePattern::Typed {
                        ty: Type::Int,
                        var: Some("n".to_string())
                    }
                ),
                ArgEntryPattern::Positional(ValuePattern::Typed {
                    ty: Type::Uint,
                    var: None
                }),
            ]
        );
        let data = el.data.as_ref().unwrap();
        assert_eq!(
            data.fields,
            [
                (
                    "ver".to_string(),
                    ValuePattern::Typed {
                        ty: Type::Version,
                        var: Some("v".to_string())
                    }
                ),
                (
                    "tag".to_string(),
                    ValuePattern::Typed {
                        ty: Type::String,
                        var: None
                    }
                ),
            ]
        );
        assert_eq!(p.bound_names().into_iter().collect::<Vec<_>>(), ["n", "v"]);
    }
}
