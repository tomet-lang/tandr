//! Replacement templates: text with holes.
//!
//! `?(name)` and `?(...name)` insert what the pattern bound; `??` is a literal
//! `?`; any other `?` is itself. A template is parsed once, so a name that the
//! pattern never binds is found when the rules are loaded, not when a file is
//! half rewritten.

use crate::hole::parse_hole;
use crate::matcher::Bindings;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    Hole(String),
}

/// A parsed template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pieces: Vec<Piece>,
}

impl Template {
    pub fn parse(src: &str) -> Result<Self, String> {
        let chars: Vec<char> = src.chars().collect();
        let mut pieces = Vec::new();
        let mut text = String::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] != '?' {
                text.push(chars[i]);
                i += 1;
            } else if chars.get(i + 1) == Some(&'?') {
                text.push('?');
                i += 2;
            } else if chars.get(i + 1) == Some(&'(') {
                let (hole, next) = parse_hole(&chars, i)?;
                let Some(name) = hole.name else {
                    return Err(format!(
                        "?() in a template at position {i} has nothing to insert; name it, ?(name)"
                    ));
                };
                if !text.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                }
                pieces.push(Piece::Hole(name));
                i = next;
            } else {
                text.push('?');
                i += 1;
            }
        }
        if !text.is_empty() {
            pieces.push(Piece::Text(text));
        }
        Ok(Self { pieces })
    }

    /// The names this template inserts.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.pieces.iter().filter_map(|p| match p {
            Piece::Hole(name) => Some(name.as_str()),
            Piece::Text(_) => None,
        })
    }

    pub fn expand(&self, bindings: &Bindings) -> Result<String, String> {
        let mut out = String::new();
        for piece in &self.pieces {
            match piece {
                Piece::Text(t) => out.push_str(t),
                Piece::Hole(name) => match bindings.get(name) {
                    Some(value) => out.push_str(value),
                    None => return Err(format!("?({name}) is not bound by the pattern")),
                },
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bind(pairs: &[(&str, &str)]) -> Bindings {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn inserts_what_is_bound() {
        let t = Template::parse("@kind(?(kind))\n@meta{ ?(...rest) }").unwrap();
        let b = bind(&[("kind", "\"article\""), ("rest", "title: x")]);
        assert_eq!(
            t.expand(&b).unwrap(),
            "@kind(\"article\")\n@meta{ title: x }"
        );
    }

    #[test]
    fn a_name_that_is_a_prefix_of_another_is_not_confused() {
        let t = Template::parse("?(k)-?(kind)-?(k)").unwrap();
        let b = bind(&[("k", "1"), ("kind", "2")]);
        assert_eq!(t.expand(&b).unwrap(), "1-2-1");
    }

    #[test]
    fn a_bound_value_is_not_expanded_again() {
        let t = Template::parse("?(a) ?(b)").unwrap();
        let b = bind(&[("a", "?(b)"), ("b", "x")]);
        assert_eq!(t.expand(&b).unwrap(), "?(b) x");
    }

    #[test]
    fn a_question_mark_is_itself_unless_it_opens_a_hole() {
        let t = Template::parse("really? ??(not a hole) ?x").unwrap();
        assert_eq!(
            t.expand(&Bindings::new()).unwrap(),
            "really? ?(not a hole) ?x"
        );
    }

    #[test]
    fn names_lists_what_a_template_needs() {
        let t = Template::parse("?(a) and ?(...b)").unwrap();
        assert_eq!(t.names().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn an_unbound_name_is_an_error() {
        let t = Template::parse("?(missing)").unwrap();
        assert!(t.expand(&Bindings::new()).unwrap_err().contains("missing"));
    }

    #[test]
    fn a_malformed_hole_is_an_error() {
        for src in ["?(", "?(a b)", "?(a | trim)", "?()"] {
            assert!(Template::parse(src).is_err(), "{src}");
        }
    }
}
