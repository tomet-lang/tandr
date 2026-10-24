//! Parser and AST for tandr structural patterns ("tomet with holes").
//!
//! Examples:
//! - `@meta{ type: ?(kind), ?(...rest) }`
//! - `@link(url: ?(u))[ ?(text) ]`
//! - `@task(?(status))[ ?(body) ]`
//!
//! A hole is `?(name)`, `?()` (anything, bound to nothing) or `?(...name)`
//! (the rest of a group); see [`crate::hole`].

use std::collections::BTreeSet;
use std::fmt;

use crate::hole::{Hole, parse_hole};

/// A structural pattern to match against tomet documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// Matches an element `@sigil(args){data}[content]#(id)`.
    Element(ElementPattern),
}

/// Which sigil an element pattern is written with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SigilKind {
    /// `@name`
    #[default]
    At,
}

/// Pattern for matching an element.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ElementPattern {
    pub sigil: SigilKind,
    /// Tag name matcher (e.g. `meta`, `link`, or `?(tag)`).
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValuePattern {
    Any,
    Var(String),
    String(String),
    Number(i64),
    Bool(bool),
    Ident(String),
}

impl fmt::Display for ValuePattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValuePattern::Any => write!(f, "?()"),
            ValuePattern::Var(v) => write!(f, "?({v})"),
            ValuePattern::String(s) => write!(f, "\"{s}\""),
            ValuePattern::Number(n) => write!(f, "{n}"),
            ValuePattern::Bool(b) => write!(f, "{b}"),
            ValuePattern::Ident(i) => write!(f, "{i}"),
        }
    }
}

/// Parses a pattern string into a [`Pattern`].
pub fn parse_pattern(src: &str) -> Result<Pattern, String> {
    let mut parser = PatternParser::new(src);
    parser.parse()
}

impl Pattern {
    /// The names this pattern binds, for checking a template against it.
    pub fn bound_names(&self) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let Pattern::Element(el) = self;
        if let TagPattern::Var(v) = &el.tag {
            names.insert(v.clone());
        }
        if let Some(SlotPattern::Var(v)) = &el.id {
            names.insert(v.clone());
        }
        if let Some(SlotPattern::Var(v)) = &el.content {
            names.insert(v.clone());
        }
        if let Some(args) = &el.args {
            for entry in &args.entries {
                let (ArgEntryPattern::Positional(v) | ArgEntryPattern::Named(_, v)) = entry;
                if let ValuePattern::Var(n) = v {
                    names.insert(n.clone());
                }
            }
            names.extend(args.rest_var.clone());
        }
        if let Some(data) = &el.data {
            for (_, v) in &data.fields {
                if let ValuePattern::Var(n) = v {
                    names.insert(n.clone());
                }
            }
            names.extend(data.rest_var.clone());
        }
        names
    }
}

struct PatternParser<'a> {
    chars: Vec<char>,
    pos: usize,
    _src: &'a str,
}

impl<'a> PatternParser<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            chars: src.chars().collect(),
            pos: 0,
            _src: src,
        }
    }

    fn parse(&mut self) -> Result<Pattern, String> {
        self.skip_ws();
        if matches!(self.peek(), Some('@')) {
            let el = self.parse_element()?;
            self.skip_ws();
            if self.pos < self.chars.len() {
                return Err(format!(
                    "Unexpected trailing content at position {}: {:?}",
                    self.pos,
                    &self.chars[self.pos..]
                ));
            }
            Ok(Pattern::Element(el))
        } else {
            Err(format!(
                "Pattern must start with '@' (got {:?})",
                self.peek()
            ))
        }
    }

    fn parse_element(&mut self) -> Result<ElementPattern, String> {
        self.expect('@')?;
        let sigil = SigilKind::At;
        let tag = self.parse_tag()?;

        let mut el = ElementPattern {
            sigil,
            tag,
            ..Default::default()
        };

        // Parse optional groups in any order: `(args)`, `{data}`, `[content]`, `#(id)`
        loop {
            self.skip_ws();
            match self.peek() {
                Some('(') => {
                    if el.args.is_some() {
                        return Err("Duplicate args group `(...)` in pattern".to_string());
                    }
                    el.args = Some(self.parse_args()?);
                }
                Some('{') => {
                    if el.data.is_some() {
                        return Err("Duplicate data group `{...}` in pattern".to_string());
                    }
                    el.data = Some(self.parse_data()?);
                }
                Some('[') => {
                    if el.content.is_some() {
                        return Err("Duplicate content group `[...]` in pattern".to_string());
                    }
                    el.content = Some(self.parse_content()?);
                }
                Some('#') => {
                    if el.id.is_some() {
                        return Err("Duplicate id group `#(id)` in pattern".to_string());
                    }
                    el.id = Some(self.parse_id()?);
                }
                _ => break,
            }
        }

        Ok(el)
    }

    /// A hole at the cursor. `?` must be the next character.
    fn hole(&mut self) -> Result<Hole, String> {
        let (hole, next) = parse_hole(&self.chars, self.pos)?;
        self.pos = next;
        Ok(hole)
    }

    /// A hole where a single thing goes: a rest hole is not one.
    fn single_hole(&mut self) -> Result<Option<String>, String> {
        let hole = self.hole()?;
        if hole.rest {
            return Err(
                "?(...name) is the rest of a group; it cannot stand for one thing".to_string(),
            );
        }
        Ok(hole.name)
    }

    /// An identifier, but not the old wildcard `_`.
    fn eat_name(&mut self) -> Result<String, String> {
        let ident = self.eat_ident()?;
        if ident == "_" {
            return Err("write ?() for a wildcard; a bare _ is no longer one".to_string());
        }
        Ok(ident)
    }

    fn parse_tag(&mut self) -> Result<TagPattern, String> {
        self.skip_ws();
        if self.peek() == Some('?') {
            Ok(match self.single_hole()? {
                Some(name) => TagPattern::Var(name),
                None => TagPattern::Any,
            })
        } else {
            Ok(TagPattern::Exact(self.eat_name()?))
        }
    }

    fn parse_id(&mut self) -> Result<SlotPattern, String> {
        self.expect('#')?;
        if self.peek() == Some('(') {
            self.bump();
            self.skip_ws();
            let slot = self.parse_slot_content(')')?;
            self.expect(')')?;
            Ok(slot)
        } else {
            // bare `#name`
            let ident = self.eat_ident()?;
            Ok(SlotPattern::Exact(ident))
        }
    }

    fn parse_args(&mut self) -> Result<ArgsPattern, String> {
        self.expect('(')?;
        self.skip_ws();

        let mut entries = Vec::new();
        let mut rest_var = None;

        while self.peek() != Some(')') && self.pos < self.chars.len() {
            self.skip_ws();
            if self.starts_with("...") {
                return Err(
                    "the rest of a group is written ?(...name); ...?name is gone".to_string(),
                );
            }
            if self.peek() == Some('?') {
                let (hole, next) = parse_hole(&self.chars, self.pos)?;
                if hole.rest {
                    let Some(name) = hole.name else {
                        return Err("name the rest, ?(...name): an anonymous rest binds nothing"
                            .to_string());
                    };
                    self.pos = next;
                    rest_var = Some(name);
                    self.skip_ws();
                    if self.peek() == Some(',') {
                        self.bump();
                    }
                    continue;
                }
            }

            // Check if key: value or positional value
            let checkpoint = self.pos;
            if let Ok(ident) = self.eat_ident() {
                self.skip_ws();
                if self.peek() == Some(':') {
                    self.bump();
                    self.skip_ws();
                    let val = self.parse_value()?;
                    entries.push(ArgEntryPattern::Named(ident, val));
                    self.skip_ws();
                    if self.peek() == Some(',') {
                        self.bump();
                    }
                    continue;
                }
            }
            // Rewind and parse as positional value
            self.pos = checkpoint;
            let val = self.parse_value()?;
            entries.push(ArgEntryPattern::Positional(val));
            self.skip_ws();
            if self.peek() == Some(',') {
                self.bump();
            }
        }

        self.expect(')')?;
        Ok(ArgsPattern { entries, rest_var })
    }

    fn parse_data(&mut self) -> Result<DataPattern, String> {
        self.expect('{')?;
        self.skip_ws();

        let mut fields = Vec::new();
        let mut rest_var = None;

        while self.peek() != Some('}') && self.pos < self.chars.len() {
            self.skip_ws();
            if self.starts_with("...") {
                return Err(
                    "the rest of a group is written ?(...name); ...?name is gone".to_string(),
                );
            }
            if self.peek() == Some('?') {
                let (hole, next) = parse_hole(&self.chars, self.pos)?;
                if hole.rest {
                    let Some(name) = hole.name else {
                        return Err("name the rest, ?(...name): an anonymous rest binds nothing"
                            .to_string());
                    };
                    self.pos = next;
                    rest_var = Some(name);
                    self.skip_ws();
                    if self.peek() == Some(',') {
                        self.bump();
                    }
                    continue;
                }
            }

            let key = self.eat_ident()?;
            self.skip_ws();
            self.expect(':')?;
            self.skip_ws();
            let val = self.parse_value()?;
            fields.push((key, val));

            self.skip_ws();
            if self.peek() == Some(',') {
                self.bump();
            }
        }

        self.expect('}')?;
        Ok(DataPattern { fields, rest_var })
    }

    fn parse_content(&mut self) -> Result<SlotPattern, String> {
        self.expect('[')?;
        self.skip_ws();
        let slot = self.parse_slot_content(']')?;
        self.expect(']')?;
        Ok(slot)
    }

    fn parse_slot_content(&mut self, closing: char) -> Result<SlotPattern, String> {
        self.skip_ws();
        if self.peek() == Some('?') {
            let slot = match self.single_hole()? {
                Some(name) => SlotPattern::Var(name),
                None => SlotPattern::Any,
            };
            self.skip_ws();
            Ok(slot)
        } else if self.peek() == Some('_') {
            Err("write ?() for a wildcard; a bare _ is no longer one".to_string())
        } else {
            // Read until closing bracket
            let start = self.pos;
            while let Some(c) = self.peek() {
                if c == closing {
                    break;
                }
                self.bump();
            }
            let s: String = self.chars[start..self.pos].iter().collect();
            Ok(SlotPattern::Exact(s.trim().to_string()))
        }
    }

    fn parse_value(&mut self) -> Result<ValuePattern, String> {
        self.skip_ws();
        match self.peek() {
            Some('?') => Ok(match self.single_hole()? {
                Some(name) => ValuePattern::Var(name),
                None => ValuePattern::Any,
            }),
            Some('"') | Some('\'') => {
                let quote = self.bump().unwrap();
                let mut s = String::new();
                while let Some(c) = self.bump() {
                    if c == quote {
                        break;
                    }
                    if c == '\\' {
                        if let Some(escaped) = self.bump() {
                            s.push(escaped);
                            continue;
                        }
                    }
                    s.push(c);
                }
                Ok(ValuePattern::String(s))
            }
            Some(c) if c.is_ascii_digit() || c == '-' => {
                let mut num_str = String::new();
                num_str.push(self.bump().unwrap());
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        num_str.push(self.bump().unwrap());
                    } else {
                        break;
                    }
                }
                let n: i64 = num_str
                    .parse()
                    .map_err(|e| format!("Invalid integer: {e}"))?;
                Ok(ValuePattern::Number(n))
            }
            _ => {
                let ident = self.eat_name()?;
                match ident.as_str() {
                    "true" => Ok(ValuePattern::Bool(true)),
                    "false" => Ok(ValuePattern::Bool(false)),
                    _ => Ok(ValuePattern::Ident(ident)),
                }
            }
        }
    }

    fn eat_ident(&mut self) -> Result<String, String> {
        self.skip_ws();
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
                self.bump();
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err(format!(
                "Expected identifier at position {}, found {:?}",
                self.pos,
                self.peek()
            ));
        }
        Ok(self.chars[start..self.pos].iter().collect())
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.bump();
            } else {
                break;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn expect(&mut self, expected: char) -> Result<(), String> {
        self.skip_ws();
        if self.peek() == Some(expected) {
            self.bump();
            Ok(())
        } else {
            Err(format!(
                "Expected '{}' at position {}, found {:?}",
                expected,
                self.pos,
                self.peek()
            ))
        }
    }

    fn starts_with(&self, prefix: &str) -> bool {
        let prefix_chars: Vec<char> = prefix.chars().collect();
        if self.pos + prefix_chars.len() > self.chars.len() {
            return false;
        }
        &self.chars[self.pos..self.pos + prefix_chars.len()] == prefix_chars.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meta_pattern_with_rest() {
        let p = parse_pattern("@meta{ type: ?(kind), ?(...rest) }").unwrap();
        match p {
            Pattern::Element(el) => {
                assert_eq!(el.tag, TagPattern::Exact("meta".to_string()));
                let data = el.data.unwrap();
                assert_eq!(data.fields.len(), 1);
                assert_eq!(data.fields[0].0, "type");
                assert_eq!(data.fields[0].1, ValuePattern::Var("kind".to_string()));
                assert_eq!(data.rest_var.as_deref(), Some("rest"));
            }
        }
    }

    #[test]
    fn parses_link_pattern_with_args_and_content() {
        let p = parse_pattern("@link(url: ?(u))[ ?(text) ]").unwrap();
        match p {
            Pattern::Element(el) => {
                assert_eq!(el.tag, TagPattern::Exact("link".to_string()));
                let args = el.args.unwrap();
                assert_eq!(args.entries.len(), 1);
                match &args.entries[0] {
                    ArgEntryPattern::Named(k, v) => {
                        assert_eq!(k, "url");
                        assert_eq!(v, &ValuePattern::Var("u".to_string()));
                    }
                    _ => panic!("expected named arg"),
                }
                assert_eq!(el.content, Some(SlotPattern::Var("text".to_string())));
            }
        }
    }

    #[test]
    fn parses_positional_args_and_id() {
        let p = parse_pattern("@task#(task-1)(?(status))[ ?(body) ]").unwrap();
        match p {
            Pattern::Element(el) => {
                assert_eq!(el.tag, TagPattern::Exact("task".to_string()));
                assert_eq!(el.id, Some(SlotPattern::Exact("task-1".to_string())));
                let args = el.args.unwrap();
                assert_eq!(args.entries.len(), 1);
                assert_eq!(
                    args.entries[0],
                    ArgEntryPattern::Positional(ValuePattern::Var("status".to_string()))
                );
                assert_eq!(el.content, Some(SlotPattern::Var("body".to_string())));
            }
        }
    }

    #[test]
    fn a_hole_has_a_group_and_the_group_delimits_the_name() {
        let p = parse_pattern("@x(a: ?(k), b: ?(kind))").unwrap();
        let Pattern::Element(el) = &p;
        assert_eq!(
            p.bound_names().into_iter().collect::<Vec<_>>(),
            ["k", "kind"]
        );
        assert_eq!(el.args.as_ref().unwrap().entries.len(), 2);
    }

    #[test]
    fn the_wildcard_binds_nothing_and_the_tag_may_be_a_hole() {
        let p = parse_pattern("@?(tag)(?(), a: ?())").unwrap();
        let Pattern::Element(el) = &p;
        assert_eq!(el.tag, TagPattern::Var("tag".to_string()));
        assert_eq!(p.bound_names().into_iter().collect::<Vec<_>>(), ["tag"]);
        let p = parse_pattern("@?()").unwrap();
        let Pattern::Element(el) = &p;
        assert_eq!(el.tag, TagPattern::Any);
    }

    #[test]
    fn the_rest_of_a_group_is_named_in_a_hole() {
        let p = parse_pattern("@x(a: 1, ?(...more)){ k: ?(v), ?(...rest) }").unwrap();
        let Pattern::Element(el) = &p;
        assert_eq!(el.args.as_ref().unwrap().rest_var.as_deref(), Some("more"));
        assert_eq!(el.data.as_ref().unwrap().rest_var.as_deref(), Some("rest"));
        assert!(p.bound_names().contains("more"));
    }

    #[test]
    fn the_old_spellings_are_refused_with_the_new_one() {
        for (src, hint) in [
            ("@meta{ type: ?kind }", "?(name)"),
            ("@meta{ ...?rest }", "?(...name)"),
            ("@meta{ type: _ }", "?()"),
            ("@_", "?()"),
            ("@link[ _ ]", "?()"),
            ("@x(?(...r))[ ?(...c) ]", "rest of a group"),
            ("@x(?(...))", "name the rest"),
        ] {
            let err = parse_pattern(src).unwrap_err();
            assert!(err.contains(hint), "{src}: {err}");
        }
    }

    #[test]
    fn a_pattern_without_groups_matches_by_name_alone() {
        let Pattern::Element(el) = parse_pattern("@kind").unwrap();
        assert_eq!(el.args, None);
        assert_eq!(el.data, None);
        assert_eq!(el.content, None);
    }
}
