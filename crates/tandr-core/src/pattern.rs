//! Parser and AST for tandr structural patterns ("tomet with holes").
//!
//! Examples:
//! - `@meta{ type: ?kind, ...?rest }`
//! - `@link(url: ?u)[ ?text ]`
//! - `@task(?status)[ ?body ]`

use std::fmt;

/// A structural pattern to match against tomet documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    /// Matches an element `@sigil(args){data}[content]#(id)`.
    Element(ElementPattern),
}

/// Pattern for matching an element.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ElementPattern {
    /// Tag name matcher (e.g. `meta`, `link`, or `?tag`).
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
            ValuePattern::Any => write!(f, "_"),
            ValuePattern::Var(v) => write!(f, "?{v}"),
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
        if self.peek() == Some('@') {
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
                "Pattern must currently start with '@' (got {:?})",
                self.peek()
            ))
        }
    }

    fn parse_element(&mut self) -> Result<ElementPattern, String> {
        self.expect('@')?;
        let tag = self.parse_tag()?;

        let mut el = ElementPattern {
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

    fn parse_tag(&mut self) -> Result<TagPattern, String> {
        self.skip_ws();
        if self.peek() == Some('?') {
            self.bump();
            let name = self.eat_ident()?;
            Ok(TagPattern::Var(name))
        } else if self.peek() == Some('_') {
            self.bump();
            Ok(TagPattern::Any)
        } else {
            let name = self.eat_ident()?;
            Ok(TagPattern::Exact(name))
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
                self.pos += 3;
                self.skip_ws();
                if self.peek() == Some('?') {
                    self.bump();
                }
                rest_var = Some(self.eat_ident()?);
                self.skip_ws();
                if self.peek() == Some(',') {
                    self.bump();
                }
                continue;
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
                self.pos += 3;
                self.skip_ws();
                if self.peek() == Some('?') {
                    self.bump();
                }
                rest_var = Some(self.eat_ident()?);
                self.skip_ws();
                if self.peek() == Some(',') {
                    self.bump();
                }
                continue;
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
            self.bump();
            let name = self.eat_ident()?;
            self.skip_ws();
            Ok(SlotPattern::Var(name))
        } else if self.peek() == Some('_') {
            self.bump();
            self.skip_ws();
            Ok(SlotPattern::Any)
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
            Some('?') => {
                self.bump();
                let name = self.eat_ident()?;
                Ok(ValuePattern::Var(name))
            }
            Some('_') => {
                self.bump();
                Ok(ValuePattern::Any)
            }
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
                let n: i64 = num_str.parse().map_err(|e| format!("Invalid integer: {e}"))?;
                Ok(ValuePattern::Number(n))
            }
            _ => {
                let ident = self.eat_ident()?;
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
        let p = parse_pattern("@meta{ type: ?kind, ...?rest }").unwrap();
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
        let p = parse_pattern("@link(url: ?u)[ ?text ]").unwrap();
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
        let p = parse_pattern("@task#(task-1)(?status)[ ?body ]").unwrap();
        match p {
            Pattern::Element(el) => {
                assert_eq!(el.tag, TagPattern::Exact("task".to_string()));
                assert_eq!(el.id, Some(SlotPattern::Exact("task-1".to_string())));
                let args = el.args.unwrap();
                assert_eq!(args.entries.len(), 1);
                assert_eq!(args.entries[0], ArgEntryPattern::Positional(ValuePattern::Var("status".to_string())));
                assert_eq!(el.content, Some(SlotPattern::Var("body".to_string())));
            }
        }
    }
}
