//! What tandr makes of a hole in a pattern.
//!
//! tomet reads the syntax: `<?matcher(name, args){options}>[content]` is a
//! [`tomet_ast::Hole`] in what `tomet_ast_parser::parse_file` returns
//! (spec: `tomet/tmtroot/spec/forms/hole.tmt`). What a matcher is and what a
//! hole matches is tandr's, and this is where it is decided:
//!
//! - `<?(name)>` binds whatever stands in that place; `<?>` matches it and binds nothing;
//! - `<?rest(name)>` is the rest of a group: the entries a pattern did not name;
//! - `<?element(tag, args){data}>[content]` is any element, in the place of the
//!   `@name` of an element pattern; `<?declaration(...)>` is the same for a `::name`
//!   declaration. Their groups are the element's own, written as an element writes
//!   them: the first bare word of `( )` is the *tag* it binds (not the whole
//!   element, which is what a bare word binds in every other hole), the keyed
//!   entries after it are the element's `(args)`, `{ }` is its data and `[ ]` its
//!   content.
//!
//! Any other matcher, and the `{options}` and `[content]` groups of any other hole,
//! are not implemented; a pattern that uses one is refused when it is read.

use tomet_ast::{Hole as AstHole, Value};
use tomet_type::Type;

/// A hole, as tandr reads it.
#[derive(Debug, Clone, PartialEq)]
pub enum Hole {
    /// `<?(name)>` or `<?>` / `<?()>`: matches one thing; the name it binds, if it has one.
    Bind(Option<String>),
    /// `<?int(name)>`, `<?version(name)>`, or `<?int>`: matches a typed value.
    Typed { ty: Type, var: Option<String> },
    /// `<?rest(name)>`: the rest of a `{ }` or `( )` group.
    Rest(String),
    /// `<?element(tag, args){data}>[content]` or `<?declaration(...)>`: any element, or any
    /// `::` declaration. `tag` is the name it binds the tag to, `args` what the element's
    /// own `( )` has to match (the entries after the tag). The `{ }` and `[ ]` are the
    /// element's, left on the pattern.
    Element {
        declaration: bool,
        tag: Option<String>,
        args: Option<Value>,
    },
}

/// The matcher of the rest of a group.
pub const REST: &str = "rest";

/// The matcher of any element.
pub const ELEMENT: &str = "element";

/// The matcher of any `::` declaration.
pub const DECLARATION: &str = "declaration";

/// Whether `s` may be a hole's name.
pub fn is_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Reads the AST hole `hole` into a tandr [`Hole`].
pub fn read(hole: &AstHole) -> Result<Hole, String> {
    let matcher = hole.matcher.as_ref().map(|m| m.name.as_str());
    if matcher == Some(ELEMENT) || matcher == Some(DECLARATION) {
        let (tag, args) = split_element_args(hole.data.args.as_ref())?;
        return Ok(Hole::Element {
            declaration: matcher == Some(DECLARATION),
            tag,
            args,
        });
    }
    if hole.content.is_some() || hole.data.value.is_some() {
        return Err(
            "a hole with `{ }` options or `[ ]` content is not implemented yet; \
             write <?(name)> or <?>"
                .to_string(),
        );
    }
    let name = name_of(hole.data.args.as_ref())?;
    match matcher {
        None => Ok(Hole::Bind(name)),
        Some(REST) => match name {
            Some(name) => Ok(Hole::Rest(name)),
            None => {
                Err("name the rest, <?rest(name)>: an anonymous rest binds nothing".to_string())
            }
        },
        Some(m) if Type::from_name(m).is_some() => {
            let ty = Type::from_name(m).unwrap();
            Ok(Hole::Typed { ty, var: name })
        }
        Some(other) => Err(format!(
            "the matcher `{other}` is not implemented; tandr knows `{REST}`, `{ELEMENT}`, `{DECLARATION}` and types ({})",
            Type::ALL
                .iter()
                .map(|t| t.base_name())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The binding name a hole's `( )` holds: nothing, or one bare word.
fn name_of(args: Option<&Value>) -> Result<Option<String>, String> {
    match args {
        None => Ok(None),
        Some(Value::Map(entries)) if entries.is_empty() => Ok(None),
        Some(Value::String(name)) => check_name(name).map(Some),
        Some(_) => Err("a hole takes one name, <?(name)>".to_string()),
    }
}

/// `name` as a binding name.
fn check_name(name: &str) -> Result<String, String> {
    if is_name(name) {
        Ok(name.to_string())
    } else if name.starts_with("...") {
        Err(format!(
            "<?({name})> is the old rest of a group; write <?{REST}({})>",
            name.trim_start_matches('.')
        ))
    } else {
        Err(format!(
            "`{name}` is not a name: a name is letters, digits and '_'"
        ))
    }
}

/// The `( )` of `<?element(...)>`: the tag it binds (a first entry without a key, a bare
/// word) and what is left, which is the element's own `(args)`.
fn split_element_args(args: Option<&Value>) -> Result<(Option<String>, Option<Value>), String> {
    match args {
        None => Ok((None, None)),
        Some(Value::Map(entries)) => {
            let mut rest = entries.clone();
            let tag = match rest.first() {
                Some((key, Value::String(name))) if key.is_empty() => {
                    let tag = check_name(name)?;
                    rest.remove(0);
                    Some(tag)
                }
                _ => None,
            };
            Ok((tag, (!rest.is_empty()).then_some(Value::Map(rest))))
        }
        Some(Value::String(name)) => Ok((Some(check_name(name)?), None)),
        // one value that is not a name: the element's only positional argument
        Some(other) => Ok((None, Some(other.clone()))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tomet_ast::Block;

    /// The hole `src` is, read as a one-hole pattern.
    fn hole(src: &str) -> Result<Hole, String> {
        let file = tomet_ast_parser::parse_file(src).map_err(|e| e.to_string())?;
        let Some(Block::Hole(h)) = file.body.first() else {
            panic!(
                "`{src}` does not start with a hole; body was {:?}",
                file.body
            );
        };
        read(h)
    }

    #[test]
    fn a_hole_binds_or_binds_nothing() {
        assert_eq!(hole("<?(kind)>"), Ok(Hole::Bind(Some("kind".to_string()))));
        assert_eq!(hole("<?>"), Ok(Hole::Bind(None)));
        assert_eq!(hole("<?()>"), Ok(Hole::Bind(None)));
    }

    #[test]
    fn the_rest_is_a_matcher_and_needs_a_name() {
        assert_eq!(hole("<?rest(r)>"), Ok(Hole::Rest("r".to_string())));
        assert!(hole("<?rest()>").unwrap_err().contains("anonymous"));
    }

    #[test]
    fn an_element_hole_binds_the_tag_and_keeps_the_rest_of_its_parens_as_the_args() {
        assert_eq!(
            hole("<?element(tag)>"),
            Ok(Hole::Element {
                declaration: false,
                tag: Some("tag".to_string()),
                args: None,
            })
        );
        assert_eq!(
            hole("<?element()>"),
            Ok(Hole::Element {
                declaration: false,
                tag: None,
                args: None,
            })
        );
        let Ok(Hole::Element { tag, args, .. }) = hole("<?element(tag, url: <?>)>") else {
            panic!("not an element hole");
        };
        assert_eq!(tag.as_deref(), Some("tag"));
        let Some(Value::Map(args)) = args else {
            panic!("no args");
        };
        assert_eq!(args.len(), 1);
        assert_eq!(args[0].0, "url");
        // no tag, only args
        let Ok(Hole::Element { tag, args, .. }) = hole("<?element(url: <?>)>") else {
            panic!("not an element hole");
        };
        assert_eq!(tag, None);
        assert!(args.is_some());
    }

    #[test]
    fn typed_holes_match_type_and_bind_or_not() {
        assert_eq!(
            hole("<?int(n)>"),
            Ok(Hole::Typed {
                ty: Type::Int,
                var: Some("n".to_string())
            })
        );
        assert_eq!(
            hole("<?int>"),
            Ok(Hole::Typed {
                ty: Type::Int,
                var: None
            })
        );
        assert_eq!(
            hole("<?int()>"),
            Ok(Hole::Typed {
                ty: Type::Int,
                var: None
            })
        );
        assert_eq!(
            hole("<?uint(u)>"),
            Ok(Hole::Typed {
                ty: Type::Uint,
                var: Some("u".to_string())
            })
        );
        assert_eq!(
            hole("<?float(f)>"),
            Ok(Hole::Typed {
                ty: Type::Float,
                var: Some("f".to_string())
            })
        );
        assert_eq!(
            hole("<?string(s)>"),
            Ok(Hole::Typed {
                ty: Type::String,
                var: Some("s".to_string())
            })
        );
        assert_eq!(
            hole("<?bool(b)>"),
            Ok(Hole::Typed {
                ty: Type::Bool,
                var: Some("b".to_string())
            })
        );
        assert_eq!(
            hole("<?datetime(d)>"),
            Ok(Hole::Typed {
                ty: Type::Datetime,
                var: Some("d".to_string())
            })
        );
        assert_eq!(
            hole("<?version(v)>"),
            Ok(Hole::Typed {
                ty: Type::Version,
                var: Some("v".to_string())
            })
        );
        assert_eq!(
            hole("<?uuid(u)>"),
            Ok(Hole::Typed {
                ty: Type::Uuid,
                var: Some("u".to_string())
            })
        );
        assert_eq!(
            hole("<?nanoid(n)>"),
            Ok(Hole::Typed {
                ty: Type::Nanoid,
                var: Some("n".to_string())
            })
        );
        assert_eq!(
            hole("<?list(items)>"),
            Ok(Hole::Typed {
                ty: Type::List(None),
                var: Some("items".to_string())
            })
        );
    }

    #[test]
    fn what_is_not_implemented_is_refused_by_name() {
        assert!(hole("<?foo(n)>").unwrap_err().contains("`foo`"));
        assert!(hole("<?(n)>[abc]").unwrap_err().contains("not implemented"));
        assert!(
            hole("<?(n){min: 1}>")
                .unwrap_err()
                .contains("not implemented")
        );
        assert!(hole("<?(a, b)>").unwrap_err().contains("one name"));
        assert!(hole("<?(\"a b\")>").unwrap_err().contains("not a name"));
    }
}
