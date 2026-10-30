//! Replacement templates: text with `${hole.name}` in it.
//!
//! `${hole.name}` inserts what the pattern bound to `name` (`?(name)` or `?rest(name)`);
//! everything else, `?` and a `$` that does not open `${` included, is itself. tomet reads
//! the `${...}`; `hole` is not a value tomet evaluates, so tandr resolves it here.
//!
//! A template is parsed once, so a name that the pattern never binds is found when the
//! rules are loaded, not when a file is half rewritten.

use tomet_ast::{ElementValue, InterpExprKind, Sigil};

use crate::hole::is_name;
use crate::matcher::Bindings;

/// The namespace a template reads the pattern's bindings from.
const NAMESPACE: &str = "hole";

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
        let mut pieces = Vec::new();
        let mut rest = src;
        let mut offset = 0;
        while let Some(at) = rest.find("${") {
            if at > 0 {
                pieces.push(Piece::Text(rest[..at].to_string()));
            }
            let (name, len) =
                parse_hole(&rest[at..]).map_err(|e| format!("{e} (at byte {})", offset + at))?;
            pieces.push(Piece::Hole(name));
            rest = &rest[at + len..];
            offset += at + len;
        }
        if !rest.is_empty() {
            pieces.push(Piece::Text(rest.to_string()));
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
                    None => {
                        return Err(format!(
                            "${{{NAMESPACE}.{name}}} is not bound by the pattern"
                        ));
                    }
                },
            }
        }
        Ok(out)
    }
}

/// The `${hole.name}` that `src` starts with: the name, and how many bytes it takes.
fn parse_hole(src: &str) -> Result<(String, usize), String> {
    let el = tomet_parser::parse_dollar_element_str(src).map_err(|e| e.to_string())?;
    let Some(ElementValue::Interp(expr)) = &el.value else {
        return Err(format!(
            "only ${{{NAMESPACE}.name}} can be inserted in a template"
        ));
    };
    let name = match (&el.sigil, &expr.kind) {
        (Sigil::Dollar, InterpExprKind::Member { object, member }) if matches!(&object.kind, InterpExprKind::Identifier(ns) if ns == NAMESPACE) => {
            member
        }
        _ => {
            return Err(format!(
                "only ${{{NAMESPACE}.name}} can be inserted in a template; filters and other \
                 expressions are not implemented"
            ));
        }
    };
    if !is_name(name) {
        return Err(format!(
            "`{name}` is not a name: a name is letters, digits and '_'"
        ));
    }
    Ok((name.clone(), el.span.end))
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
        let t = Template::parse("@kind(${hole.kind})\n@meta{ ${hole.rest} }").unwrap();
        let b = bind(&[("kind", "\"article\""), ("rest", "title: x")]);
        assert_eq!(
            t.expand(&b).unwrap(),
            "@kind(\"article\")\n@meta{ title: x }"
        );
    }

    #[test]
    fn a_name_that_is_a_prefix_of_another_is_not_confused() {
        let t = Template::parse("${hole.k}-${hole.kind}-${hole.k}").unwrap();
        let b = bind(&[("k", "1"), ("kind", "2")]);
        assert_eq!(t.expand(&b).unwrap(), "1-2-1");
    }

    #[test]
    fn a_bound_value_is_not_expanded_again() {
        let t = Template::parse("${hole.a} ${hole.b}").unwrap();
        let b = bind(&[("a", "${hole.b}"), ("b", "x")]);
        assert_eq!(t.expand(&b).unwrap(), "${hole.b} x");
    }

    #[test]
    fn only_a_dollar_brace_opens_a_hole() {
        // `?`, `?(...)` and a `$` of its own are text now
        let t = Template::parse("really? ?(not a hole) costs $5 ${hole.x}").unwrap();
        assert_eq!(
            t.expand(&bind(&[("x", "!")])).unwrap(),
            "really? ?(not a hole) costs $5 !"
        );
    }

    #[test]
    fn names_lists_what_a_template_needs() {
        let t = Template::parse("${hole.a} and ${hole.b}").unwrap();
        assert_eq!(t.names().collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn an_unbound_name_is_an_error() {
        let t = Template::parse("${hole.missing}").unwrap();
        assert!(t.expand(&Bindings::new()).unwrap_err().contains("missing"));
    }

    #[test]
    fn a_malformed_or_unknown_insert_is_an_error() {
        for src in [
            "${",
            "${hole.a b}",
            "${hole}",
            "${hole.a | trim}",
            "${vars.a}",
            "${hole.a.b}",
            "${title}",
        ] {
            assert!(Template::parse(src).is_err(), "{src}");
        }
    }
}
