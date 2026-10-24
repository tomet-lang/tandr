//! Where the parts of an element are in the source.
//!
//! The AST has one span per element and keeps no text, so it cannot say where
//! `@kind` ends or where the `(args)` are. The CST keeps every byte, and gives
//! each part of an element a node with a range: the sigil and name, and each
//! group. Matching is done on the AST; this is how a match finds its parts.

use std::collections::HashMap;
use std::ops::Range;

use tomet_cst::{NodeOrToken, SyntaxElement, SyntaxKind, SyntaxNode};

/// One entry of an `(args)` or `{data}` group, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntrySource {
    /// The key of `key: value`; none for a positional value.
    pub key: Option<String>,
    /// The whole entry, `key: value`.
    pub whole: Range<usize>,
    /// The value alone, as written: quotes, escapes and digits untouched.
    pub value: Range<usize>,
}

/// The byte ranges of one element's parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parts {
    /// The whole element, its connects included.
    pub whole: Range<usize>,
    /// The sigil and name: `@kind`, `^`, `^footnote`.
    pub head: Range<usize>,
    /// `(args)`, delimiters included.
    pub args: Option<Range<usize>>,
    /// The entries of `(args)`, in order.
    pub args_entries: Vec<EntrySource>,
    /// `{data}`
    pub data: Option<Range<usize>>,
    /// The entries of `{data}`, in order.
    pub data_entries: Vec<EntrySource>,
    /// `[content]`
    pub content: Option<Range<usize>>,
    /// `#(id)`
    pub id: Option<Range<usize>>,
}

/// The parts of every element in a document, found by the element's range.
#[derive(Debug, Default)]
pub struct ElementIndex {
    by_range: HashMap<(usize, usize), Parts>,
}

fn range(node: &SyntaxNode) -> Range<usize> {
    let r = node.text_range();
    usize::from(r.start())..usize::from(r.end())
}

/// Whether a token is only spacing: not part of what a value says.
fn is_trivia(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE)
}

/// The entries of a group node: `key: value` pairs and positional values.
fn entries(group: &SyntaxNode) -> Vec<EntrySource> {
    let mut out = Vec::new();
    for node in group.children() {
        match node.kind() {
            SyntaxKind::MAP_ENTRY => {
                let mut key = None;
                let mut start = None;
                let mut end = None;
                let mut after_colon = false;
                for el in node.children_with_tokens() {
                    let r = range_of(&el);
                    if !after_colon {
                        match el.kind() {
                            SyntaxKind::COLON => after_colon = true,
                            k if is_trivia(k) => {}
                            _ => {
                                // `"w": 1` and `w: 1` are the same key
                                key.get_or_insert_with(|| {
                                    let text = text_of(&el);
                                    match text.strip_prefix('"').and_then(|t| t.strip_suffix('"')) {
                                        Some(inner) => inner.to_string(),
                                        None => text,
                                    }
                                });
                            }
                        }
                    } else if !is_trivia(el.kind()) {
                        start.get_or_insert(r.start);
                        end = Some(r.end);
                    }
                }
                // the node may carry the spacing after its value; the entry does not
                let node_range = range(&node);
                let whole = node_range.start..end.unwrap_or(node_range.end);
                let value = match (start, end) {
                    (Some(s), Some(e)) => s..e,
                    _ => whole.end..whole.end,
                };
                out.push(EntrySource { key, whole, value });
            }
            SyntaxKind::SEQ_ITEM => {
                let whole = range(&node);
                out.push(EntrySource {
                    key: None,
                    value: whole.clone(),
                    whole,
                });
            }
            _ => {}
        }
    }
    out
}

fn range_of(el: &SyntaxElement) -> Range<usize> {
    let r = el.text_range();
    usize::from(r.start())..usize::from(r.end())
}

fn text_of(el: &SyntaxElement) -> String {
    match el {
        NodeOrToken::Node(n) => n.text().to_string(),
        NodeOrToken::Token(t) => t.text().to_string(),
    }
}

impl ElementIndex {
    pub fn new(root: &SyntaxNode) -> Self {
        let mut by_range = HashMap::new();
        for node in root.descendants() {
            if !matches!(
                node.kind(),
                SyntaxKind::BLOCK_ELEMENT | SyntaxKind::INLINE_ELEMENT
            ) {
                continue;
            }
            let mut head = None;
            let (mut args, mut data, mut content, mut id) = (None, None, None, None);
            let (mut args_entries, mut data_entries) = (Vec::new(), Vec::new());
            // direct children only: a connect's own `(args)` is inside its CONNECT node
            for child in node.children() {
                match child.kind() {
                    SyntaxKind::SIGIL => head = Some(range(&child)),
                    SyntaxKind::ARGS => {
                        args = Some(range(&child));
                        args_entries = entries(&child);
                    }
                    SyntaxKind::VALUE_DATA => {
                        data = Some(range(&child));
                        data_entries = entries(&child);
                    }
                    SyntaxKind::CONTENT => content = Some(range(&child)),
                    SyntaxKind::ID_GROUP => id = Some(range(&child)),
                    _ => {}
                }
            }
            let Some(head) = head else { continue };
            let whole = range(&node);
            by_range.insert(
                (whole.start, whole.end),
                Parts {
                    whole,
                    head,
                    args,
                    args_entries,
                    data,
                    data_entries,
                    content,
                    id,
                },
            );
        }
        Self { by_range }
    }

    /// The parts of the element that spans exactly `span`.
    pub fn get(&self, span: Range<usize>) -> Option<&Parts> {
        self.by_range.get(&(span.start, span.end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(src: &str) -> ElementIndex {
        ElementIndex::new(&tomet_parser::parse_cst(src))
    }

    #[test]
    fn an_element_is_split_into_its_parts() {
        let src = "@kind(config){ a: 1 }[ c ]#(x):rule(allow:list(a))\n\ntext\n";
        let ix = index(src);
        let p = ix.get(0..50).unwrap();
        assert_eq!(&src[p.head.clone()], "@kind");
        assert_eq!(&src[p.args.clone().unwrap()], "(config)");
        assert_eq!(&src[p.data.clone().unwrap()], "{ a: 1 }");
        assert_eq!(&src[p.content.clone().unwrap()], "[ c ]");
        assert_eq!(&src[p.id.clone().unwrap()], "#(x)");
        // the connect's own (args) are not the element's
        assert_eq!(p.whole, 0..50);
    }

    #[test]
    fn a_range_that_is_not_an_element_has_no_parts() {
        let ix = index("@a\n");
        assert!(ix.get(0..1).is_none());
    }
}
