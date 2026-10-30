//! Rewriting engine for tandr.
//!
//! A rule either replaces the whole match with a template, or edits some parts
//! of the matched element (its sigil and name, its `(args)`, `{data}`,
//! `[content]`, `#(id)`) and leaves every other byte as it was. Both come down
//! to a list of byte-range edits, applied from the back so the ranges stay
//! valid.

use std::ops::Range;

use anyhow::Result;
use tomet_parser::{parse_cst, parse_document};

use crate::cst::ElementIndex;
use crate::matcher::{MatchResult, find_matches};
use crate::rule::{Action, PartTemplates, RuleSet};

/// The result of applying rewrite rules to a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewriteResult {
    /// The resulting transformed document source text.
    pub output: String,
    /// Total number of transformations applied.
    pub changes_count: usize,
    /// Whether any modification was made to the original input.
    pub modified: bool,
    /// Matches that were left alone, and why.
    pub warnings: Vec<String>,
}

/// One replacement of a byte range. An empty range is an insertion.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Edit {
    range: Range<usize>,
    text: String,
}

/// Whether two edits cannot both be applied.
///
/// Two insertions at one point do not conflict (they are applied in the order
/// they were planned); an insertion at the end or the start of a replaced range
/// does not either. An insertion strictly inside a replaced range, or two
/// overlapping replacements, do.
fn conflicts(a: &Edit, b: &Edit) -> bool {
    let (a, b) = (&a.range, &b.range);
    match (a.is_empty(), b.is_empty()) {
        (true, true) => false,
        (true, false) => b.start < a.start && a.start < b.end,
        (false, true) => a.start < b.start && b.start < a.end,
        (false, false) => a.start < b.end && b.start < a.end,
    }
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

/// What one match becomes, as edits.
fn plan(
    action: &Action,
    m: &MatchResult,
    index: &ElementIndex,
) -> std::result::Result<Vec<Edit>, String> {
    match action {
        Action::Whole(template) => Ok(vec![Edit {
            range: m.span.start..m.span.end,
            text: template.expand(&m.bindings)?,
        }]),
        Action::Parts(parts) => plan_parts(parts, m, index),
    }
}

fn plan_parts(
    parts: &PartTemplates,
    m: &MatchResult,
    index: &ElementIndex,
) -> std::result::Result<Vec<Edit>, String> {
    let found = index
        .get(m.span.start..m.span.end)
        .ok_or("the parts of the element could not be found in the syntax tree")?;
    let mut edits = Vec::new();

    if let Some(t) = &parts.before {
        edits.push(Edit {
            range: found.whole.start..found.whole.start,
            text: t.expand(&m.bindings)?,
        });
    }
    for (template, range, name) in [
        (&parts.head, Some(found.head.clone()), "head"),
        (&parts.args, found.args.clone(), "args"),
        (&parts.data, found.data.clone(), "data"),
        (&parts.content, found.content.clone(), "content"),
        (&parts.id, found.id.clone(), "id"),
    ] {
        let Some(template) = template else { continue };
        let Some(range) = range else {
            return Err(format!("the element has no {name} to replace"));
        };
        edits.push(Edit {
            range,
            text: template.expand(&m.bindings)?,
        });
    }
    if let Some(t) = &parts.after {
        edits.push(Edit {
            range: found.whole.end..found.whole.end,
            text: t.expand(&m.bindings)?,
        });
    }
    Ok(edits)
}

/// Applies `edits` to `text`. The edits must not conflict.
fn apply_edits(text: &mut String, edits: Vec<Edit>) {
    let mut ordered: Vec<(usize, Edit)> = edits.into_iter().enumerate().collect();
    // From the back. At one start, a replacement goes before an insertion (so
    // the insertion lands in front of the new text), and of two insertions the
    // later planned goes first (so the earlier ends up in front).
    ordered.sort_by(|(i, a), (j, b)| {
        b.range
            .start
            .cmp(&a.range.start)
            .then(a.range.is_empty().cmp(&b.range.is_empty()))
            .then(j.cmp(i))
    });
    for (_, edit) in ordered {
        text.replace_range(edit.range, &edit.text);
    }
}

/// Applies all rules in `rule_set` to `src`.
///
/// The rules are checked first, all of them, and none is applied if any is
/// wrong. Each rule then sees the text the rules before it left, and the text it
/// leaves must still parse: if it does not, this is an error and nothing is returned.
pub fn apply_rule_set(src: &str, rule_set: &RuleSet) -> Result<RewriteResult> {
    let rules = rule_set
        .compile()
        .map_err(|errors| anyhow::anyhow!(errors.join("\n")))?;

    let mut current_text = src.to_string();
    let mut total_changes = 0;
    let mut warnings = Vec::new();

    for rule in &rules {
        let Some(action) = &rule.action else {
            continue;
        };

        let doc = parse_document(&current_text).map_err(|e| {
            anyhow::anyhow!("Failed to parse document during rule '{}': {e}", rule.name)
        })?;
        let index = ElementIndex::new(&parse_cst(&current_text));
        let mut matches = find_matches(&doc, &rule.pattern, &current_text, &index);
        if matches.is_empty() {
            continue;
        }
        // outer matches before the ones inside them
        matches.sort_by_key(|m| (m.span.start, std::cmp::Reverse(m.span.end)));

        let mut accepted: Vec<Edit> = Vec::new();
        for m in &matches {
            let line = line_of(&current_text, m.span.start);
            let edits = match plan(action, m, &index) {
                Ok(edits) => edits,
                Err(why) => {
                    warnings.push(format!(
                        "line {line}: rule '{}': {why}; left alone",
                        rule.name
                    ));
                    continue;
                }
            };
            if edits
                .iter()
                .any(|e| accepted.iter().any(|a| conflicts(e, a)))
            {
                warnings.push(format!(
                    "line {line}: rule '{}': overlaps another match of this rule; left alone",
                    rule.name
                ));
                continue;
            }
            accepted.extend(edits);
            total_changes += 1;
        }
        if accepted.is_empty() {
            continue;
        }
        apply_edits(&mut current_text, accepted);
        // A template can break what it is put into (a rest that ends in a `//` comment
        // swallows the `}` after it). Say which rule did it, and write nothing.
        if let Err(e) = parse_document(&current_text) {
            anyhow::bail!(
                "rule '{}' made a document that does not parse ({e}); nothing is written",
                rule.name
            );
        }
    }

    let modified = current_text != src;
    Ok(RewriteResult {
        output: current_text,
        changes_count: total_changes,
        modified,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(src: &str) -> RuleSet {
        RuleSet::from_cotomet_str(src).unwrap()
    }

    fn run(src: &str, rules_src: &str) -> RewriteResult {
        apply_rule_set(src, &rules(rules_src)).unwrap()
    }

    #[test]
    fn rewrites_meta_type_to_kind_with_a_template() {
        let src = "@meta{ type: \"article\", title: \"My Note\" }\n\nContent here.\n";
        let res = run(
            src,
            r#"
            promote_meta: {
                match: "@meta{ type: ?(k), ?rest(rest) }"
                replace: "@kind(${hole.k})\n@meta{ ${hole.rest} }"
            }
        "#,
        );
        assert!(res.modified);
        assert_eq!(res.changes_count, 1);
        assert_eq!(
            res.output,
            "@kind(\"article\")\n@meta{ title: \"My Note\" }\n\nContent here.\n"
        );
    }

    #[test]
    fn the_same_rewrite_by_parts_inserts_before_and_replaces_the_data() {
        let src = "@meta{ type: \"article\", title: \"My Note\" }\n\nContent here.\n";
        let res = run(
            src,
            r#"
            promote_meta: {
                match: "@meta{ type: ?(k), ?rest(rest) }"
                replace: { before: "@kind(${hole.k})\n", data: "{ ${hole.rest} }" }
            }
        "#,
        );
        assert_eq!(
            res.output,
            "@kind(\"article\")\n@meta{ title: \"My Note\" }\n\nContent here.\n"
        );
    }

    #[test]
    fn rewrites_links_in_prose() {
        let src = "Read @link(url: \"https://example.com\")[ Click Here ] now!\n";
        let res = run(
            src,
            r#"
            add_rel: {
                match: "@link(url: ?(u))[ ?(t) ]"
                replace: "@link(url: ${hole.u}, rel: \"external\")[ ${hole.t} ]"
            }
        "#,
        );
        assert_eq!(
            res.output,
            "Read @link(url: \"https://example.com\", rel: \"external\")[ Click Here ] now!\n"
        );
    }

    #[test]
    fn an_element_hole_matches_any_tag_and_inserts_it() {
        let src = "@note(a: 1)[ x ] and @warn(a: 2)[ y ] and @link[ z ]\n";
        let res = run(
            src,
            r#"
            tag_all: {
                match: "?element(t, a: ?(v))"
                replace: { head: "@${hole.t}-v${hole.v}" }
            }
        "#,
        );
        // `@link` has no `a`, so it is left alone
        assert_eq!(
            res.output,
            "@note-v1(a: 1)[ x ] and @warn-v2(a: 2)[ y ] and @link[ z ]\n"
        );
    }

    #[test]
    fn a_declaration_hole_matches_declarations_only() {
        let src = "::kind(config)\n::meta{ id: x }\n\n@meta{ id: y }\n";
        let res = run(
            src,
            r#"
            note: {
                match: "?declaration(t){ id: ?(i) }"
                replace: { before: "// ${hole.t} ${hole.i}\n" }
            }
        "#,
        );
        assert_eq!(
            res.output,
            "::kind(config)\n// meta x\n::meta{ id: x }\n\n@meta{ id: y }\n"
        );
    }

    const DOC_RULES: &str = r#"
        doc_kind:   { match: "@kind",   replace: { head: "::kind" } }
        doc_meta:   { match: "@meta",   replace: { head: "::meta" } }
        doc_config: { match: "@config", replace: { head: "::config" } }
    "#;

    #[test]
    fn a_head_is_renamed_and_every_other_byte_is_kept() {
        let src = "@kind(config)\n@meta{   id:  x ,\n   title:   \"T\"   }\n@config{ a:   1,   b: 2 }[ prose ]#(i):rule(allow:list(a))\n\n=[ T ]\n\ntext @kind(inline)\n";
        let res = run(src, DOC_RULES);
        assert_eq!(
            res.output,
            "::kind(config)\n::meta{   id:  x ,\n   title:   \"T\"   }\n::config{ a:   1,   b: 2 }[ prose ]#(i):rule(allow:list(a))\n\n=[ T ]\n\ntext ::kind(inline)\n"
        );
        assert_eq!(res.changes_count, 4);
        // and the reverse gives back the original, byte for byte
        assert_eq!(res.output.replace("::", "@"), src);
    }

    #[test]
    fn running_it_again_changes_nothing() {
        let src = "@kind(config)\n@meta{ id: x }\n";
        let once = run(src, DOC_RULES).output;
        let twice = run(&once, DOC_RULES);
        assert!(!twice.modified);
        assert_eq!(twice.changes_count, 0);
    }

    #[test]
    fn code_and_inline_code_are_not_elements() {
        let src = "Write `@kind(x)` first.\n\n```tmt\n@kind(x)\n```\n\n@kind(real)\n";
        let res = run(src, DOC_RULES);
        assert_eq!(
            res.output,
            "Write `@kind(x)` first.\n\n```tmt\n@kind(x)\n```\n\n::kind(real)\n"
        );
    }

    #[test]
    fn a_part_is_replaced_whole_or_removed() {
        let res = run(
            "@x(a)[ b ]{ c: 1 }\n",
            r#"
            r: { match: "@x", replace: { args: "(z)", data: "" } }
        "#,
        );
        assert_eq!(res.output, "@x(z)[ b ]\n");
    }

    #[test]
    fn after_and_before_wrap_the_element_connects_included() {
        let res = run(
            "@x(a):rule(b)\n",
            r#"
            r: { match: "@x", replace: { before: "<<", after: ">>" } }
        "#,
        );
        assert_eq!(res.output, "<<@x(a):rule(b)>>\n");
    }

    #[test]
    fn a_part_the_element_lacks_is_a_warning_and_the_element_is_left_alone() {
        let src = "@note[ hi ]\n\n@note(x)[ ho ]\n";
        let res = run(
            src,
            r#"
            r: { match: "@note", replace: { head: "@memo", args: "(y)" } }
        "#,
        );
        assert_eq!(res.output, "@note[ hi ]\n\n@memo(y)[ ho ]\n");
        assert_eq!(res.changes_count, 1);
        assert_eq!(res.warnings.len(), 1);
        assert!(res.warnings[0].contains("line 1"), "{:?}", res.warnings);
        assert!(res.warnings[0].contains("no args"), "{:?}", res.warnings);
    }

    #[test]
    fn nested_matches_are_edited_by_part_without_trouble() {
        let res = run(
            "@box[ @box[ x ] ]\n",
            r#"
            r: { match: "@box", replace: { head: "@crate" } }
        "#,
        );
        assert_eq!(res.output, "@crate[ @crate[ x ] ]\n");
        assert_eq!(res.changes_count, 2);
        assert!(res.warnings.is_empty());
    }

    #[test]
    fn a_whole_match_wins_over_the_matches_inside_it() {
        let res = run(
            "@box[ @box[ x ] ]\n",
            r#"
            r: { match: "@box[ ?(c) ]", replace: "<${hole.c}>" }
        "#,
        );
        assert_eq!(res.output, "<@box[ x ]>\n");
        assert_eq!(res.changes_count, 1);
        assert!(res.warnings[0].contains("overlaps"), "{:?}", res.warnings);
    }

    #[test]
    fn a_rewrite_that_breaks_the_document_is_an_error_naming_the_rule() {
        // the rest ends in a line comment, and the template puts `}` on the same line
        let src = "@m{\n  type: x\n  a: 1 // about a\n}\n";
        let rules = RuleSet::from_cotomet_str(
            r#"
            swallow: { match: "@m{ type: ?(t), ?rest(r) }", replace: "@n{ ${hole.r} }" }
        "#,
        )
        .unwrap();
        let err = apply_rule_set(src, &rules).unwrap_err().to_string();
        assert!(err.contains("rule 'swallow'"), "{err}");
        assert!(err.contains("nothing is written"), "{err}");
    }

    #[test]
    fn a_literal_is_compared_as_tomet_reads_it() {
        // a bare word and a quoted string are the same string to tomet, so to a pattern
        let src = "@x(id: v)\n@x(id: \"v\")\n@x(ok: true)\n@x(ok: \"true\")\n";
        let res = run(
            src,
            r#"
            a: { match: "@x(id: \"v\")", replace: { head: "@same" } }
            b: { match: "@x(ok: true)", replace: { head: "@bool" } }
        "#,
        );
        assert_eq!(
            res.output,
            "@same(id: v)\n@same(id: \"v\")\n@bool(ok: true)\n@x(ok: \"true\")\n"
        );
    }

    #[test]
    fn a_capture_is_the_text_as_written() {
        // bare word, escaped quote, trailing zero, a call: none may be rendered again
        let src = "@t(a: 1.50, b: x){ s: \"q\\\"z\", l: list(1, 2) }\n";
        let res = run(
            src,
            r#"
            r: { match: "@t(a: ?(a), b: ?(b)){ s: ?(s), l: ?(l) }", replace: "@u(${hole.a}, ${hole.b}, ${hole.s}, ${hole.l})" }
        "#,
        );
        assert_eq!(res.output, "@u(1.50, x, \"q\\\"z\", list(1, 2))\n");
    }

    #[test]
    fn the_rest_is_kept_as_written_too() {
        let src = "@m{ type: a, k: 0x10, \"w\": \"v\" }\n";
        let res = run(
            src,
            r#"
            r: { match: "@m{ type: ?(t), ?rest(rest) }", replace: "@k(${hole.t}) @m{ ${hole.rest} }" }
        "#,
        );
        assert_eq!(res.output, "@k(a) @m{ k: 0x10, \"w\": \"v\" }\n");
    }

    #[test]
    fn a_rest_written_one_entry_per_line_comes_back_one_per_line() {
        let src = "@meta{\n  type: a\n  title: \"t\"\n  author: \"x\"\n}\n";
        let res = run(
            src,
            r#"
            r: { match: "@meta{ type: ?(t), ?rest(rest) }", replace: "@kind(${hole.t})\n@meta{\n  ${hole.rest}\n}" }
        "#,
        );
        assert_eq!(
            res.output,
            "@kind(a)\n@meta{\n  title: \"t\"\n  author: \"x\"\n}\n"
        );
    }

    #[test]
    fn a_rest_with_an_entry_taken_out_of_the_middle_keeps_the_separators_style() {
        let src = "@m{\n  a: 1,\n  type: x,\n  b: 2,\n  c: 3\n}\n";
        let res = run(
            src,
            r#"
            r: { match: "@m{ type: ?(t), ?rest(rest) }", replace: "@n{\n  ${hole.rest}\n}" }
        "#,
        );
        assert_eq!(res.output, "@n{\n  a: 1,\n  b: 2,\n  c: 3\n}\n");
    }

    #[test]
    fn a_comment_between_two_kept_entries_stays_and_one_about_a_taken_entry_goes() {
        let src = "@m{\n  a: 1 // about a\n  type: x\n  // about b\n  b: 2\n}\n";
        let res = run(
            src,
            r#"
            r: { match: "@m{ type: ?(t), ?rest(rest) }", replace: "@n{\n  ${hole.rest}\n}" }
        "#,
        );
        // `// about a` is a's own (it ends the entry); `// about b` sat in the gap after the
        // taken `type`, so the rest keeps that gap's line break and indentation, not the comment
        assert_eq!(res.output, "@n{\n  a: 1 // about a\n  b: 2\n}\n");

        // two kept entries side by side keep what is between them
        let src = "@m{ type: x\n  a: 1 // about a\n  b: 2 }\n";
        let res = run(
            src,
            r#"
            r: { match: "@m{ type: ?(t), ?rest(rest) }", replace: "@n{ ${hole.rest} }" }
        "#,
        );
        assert_eq!(res.output, "@n{ a: 1 // about a\n  b: 2 }\n");
    }

    #[test]
    fn the_rest_of_the_args_binds_and_is_kept_as_written() {
        let src = "@x(a: 1, b: \"q\",\n   c: 0x10)\n";
        let res = run(
            src,
            r#"
            r: { match: "@x(a: ?(a), ?rest(r))", replace: "@y(${hole.a}; ${hole.r})" }
        "#,
        );
        assert_eq!(res.output, "@y(1; b: \"q\",\n   c: 0x10)\n");
    }

    #[test]
    fn a_declaration_pattern_matches_declarations_and_not_content() {
        let src = "::kind(config)\n::meta{ id: x }\n\n@meta{ id: y }\n";
        let res = run(
            src,
            r#"
            r: { match: "::meta{ id: ?(i) }", replace: "::meta{ id: ${hole.i}, v: 2 }" }
        "#,
        );
        assert_eq!(
            res.output,
            "::kind(config)\n::meta{ id: x, v: 2 }\n\n@meta{ id: y }\n"
        );
        assert_eq!(res.changes_count, 1);
    }

    #[test]
    fn a_content_pattern_does_not_match_a_declaration() {
        let src = "::kind(config)\n";
        let res = run(src, r#"r: { match: "@kind", replace: { head: "::gone" } }"#);
        assert_eq!(res.output, src);
        assert_eq!(res.changes_count, 0);
    }

    #[test]
    fn a_declaration_can_be_turned_into_content_and_back() {
        let res = run(
            "::meta{ id: x }\n",
            r#"r: { match: "::meta", replace: { head: "@meta" } }"#,
        );
        assert_eq!(res.output, "@meta{ id: x }\n");
        let res = run(
            "@meta{ id: x }\n",
            r#"r: { match: "@meta", replace: { head: "::meta" } }"#,
        );
        assert_eq!(res.output, "::meta{ id: x }\n");
    }

    #[test]
    fn a_name_that_is_a_prefix_of_another_is_not_confused() {
        let res = run(
            "@t(a: 1, b: 2)\n",
            r#"
            r: { match: "@t(a: ?(k), b: ?(kind))", replace: "@t(${hole.kind}, ${hole.k})" }
        "#,
        );
        assert_eq!(res.output, "@t(2, 1)\n");
    }

    #[test]
    fn a_rule_that_cannot_be_applied_as_written_is_refused_before_anything_changes() {
        let src = "@kind(x)\n";
        for rules_src in [
            r#"r: { match: "@kind", where: "?(a) == 1", replace: { head: "::kind" } }"#,
            r#"r: { match: "@kind", replace: "@k(${hole.missing})" }"#,
            r#"r: { match: "@kind", replace: { head: "@k(${hole.missing})" } }"#,
            r#"r: { match: "@kind", replace: {} }"#,
            r#"r: { match: "kind" }"#,
        ] {
            let err = apply_rule_set(src, &rules(rules_src))
                .unwrap_err()
                .to_string();
            assert!(err.contains("rule 'r'"), "{rules_src}: {err}");
        }
    }
}
