//! The hole syntax shared by patterns and templates.
//!
//! A hole is a sigil followed by a group, like every other tomet construct
//! (`#(id)`, `^(id)`, `@ftn(id)`):
//!
//! - `?(name)` binds (in a pattern) or inserts (in a template) the thing in
//!   that place;
//! - `?()` matches anything and binds nothing;
//! - `?(...name)` is the rest of a group: the entries a pattern did not name.
//!
//! The group delimits the name, so `?(k)` and `?(kind)` never get confused.

/// One hole, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hole {
    /// The name it binds, `None` for `?()`.
    pub name: Option<String>,
    /// Whether it is the rest of a group, `?(...name)`.
    pub rest: bool,
}

/// Whether `c` may be in a hole's name.
pub fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Parses the hole that starts at `chars[pos]` (which must be the `?`), and
/// returns it with the index just after its `)`.
pub fn parse_hole(chars: &[char], pos: usize) -> Result<(Hole, usize), String> {
    debug_assert_eq!(chars.get(pos), Some(&'?'));
    if chars.get(pos + 1) != Some(&'(') {
        return Err(format!(
            "a hole is written ?(name), ?() or ?(...name); found {:?} after '?' at position {pos}",
            chars.get(pos + 1)
        ));
    }
    let Some(close) = chars[pos + 2..].iter().position(|&c| c == ')') else {
        return Err(format!(
            "the hole at position {pos} is never closed with ')'"
        ));
    };
    let inner: String = chars[pos + 2..pos + 2 + close].iter().collect();
    let next = pos + 2 + close + 1;

    let inner = inner.trim();
    let (rest, name) = match inner.strip_prefix("...") {
        Some(name) => (true, name.trim()),
        None => (false, inner),
    };
    if !name.chars().all(is_name_char) {
        return Err(format!(
            "?({inner}) is not a hole: a name is letters, digits and '_' (filters are not implemented yet)"
        ));
    }
    Ok((
        Hole {
            name: (!name.is_empty()).then(|| name.to_string()),
            rest,
        },
        next,
    ))
}
