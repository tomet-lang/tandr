//! Where the parts of an element are in the source.
//!
//! The AST has one span per element and keeps no text, so it cannot say where
//! `@kind` ends or where the `(args)` are. The CST keeps every byte, and gives
//! each part of an element a node with a range: the sigil and name, and each
//! group. Matching is done on the AST; this is how a match finds its parts.

use std::collections::HashMap;
use std::ops::Range;

use tomet_cst::{SyntaxKind, SyntaxNode};

/// The byte ranges of one element's parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parts {
    /// The whole element, its connects included.
    pub whole: Range<usize>,
    /// The sigil and name: `@kind`, `^`, `^footnote`.
    pub head: Range<usize>,
    /// `(args)`, delimiters included.
    pub args: Option<Range<usize>>,
    /// `{data}`
    pub data: Option<Range<usize>>,
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
            // direct children only: a connect's own `(args)` is inside its CONNECT node
            for child in node.children() {
                match child.kind() {
                    SyntaxKind::SIGIL => head = Some(range(&child)),
                    SyntaxKind::ARGS => args = Some(range(&child)),
                    SyntaxKind::VALUE_DATA => data = Some(range(&child)),
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
                    data,
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
