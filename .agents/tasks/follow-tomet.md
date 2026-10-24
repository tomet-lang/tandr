# tandr: stabilizing it as the bridge between old and new tomet

Pushed earlier (`26e6e7a`): `motem` -> `cotomet`, caret patterns removed, `[patch]` on `../tomet` removed.

## Done 2026-10-24 (uncommitted: the user commits)

- [x] `tomet check .` passes (9 files): `default.config.tmt` leaves `examples/` unswept (the old-form input); `examples.tmt` heading uses code
  spans; `readme.tmt` links with `@link(path)` as sisho does. `just run-example-diff` takes `--force` now.
- [x] clippy clean (`--all-targets`).
- [x] `SigilKind` has a second meaning: `::name(...)` is a pattern. `@` patterns match only content, `::` patterns only declarations;
  `head: "::x"` / `head: "@x"` moves an element from one to the other.
- [x] A capture is the text as written (CST `EntrySource` in `cst.rs`), not the AST value rendered again. Before, `id: x` came back as
  `id: "x"`, a float lost its digits, a string with `"` was not escaped, and an unknown `Value` vanished (`_ => String::new()`).
  `value_to_source` stays only as the fallback when the CST has no parts for the element.
- [x] A rule with a key it does not have (`replce`, `py_transform`) is refused; it used to become a search-only rule.
- [x] `tmtroot/spec/syntax.tmt` has an 実装状況 section; the caret leftovers are gone from it.

## Left

- [ ] The contract sisho calls (sisho `tool.rs:7` and its design memo still say tandr has no command line):
  - exit codes: changed under `--check` (1), bad rule (?), some files failed (?). Write them down in the spec and test them.
  - what `sisho check` / `sisho fix` run: `run --check` and `run -i` over the project, with which rule files? (a `rules` entry in the
    project config, or `migrations/*.tandr`?) Decide with sisho.
  - machine-readable output for `--check` (which files, how many changes), if sisho's counter needs it.
  - then sisho-desk can drop `tomet-transform` (StructuralGrep) for a subprocess call (`sisho-desk/.agents/tasks/tidy-the-tabs.md`).
- [ ] `where` (guards) is refused today, so `examples/catalogue.tandr` and the commented guards in `examples/rules.tandr` do not run.
  `add_external_rel_to_links` cannot be written without a regex test. Decide the minimal guard language, then build it.
- [ ] Not built, in the spec: heading / list / task / code-block patterns, `>` `>>` `+` `~`, filters (`?(x | trim)`), `sh` / `py`.
  The spec marks them; each needs its turn in the order the user's migrations ask for.
- [ ] A value that is not written the way the pattern spells it still compares on the AST (`id: x` and `id: "x"` are the same string
  to `match_value`). Say in the spec whether that is wanted.
- [ ] `{ ...?(rest) }` is joined with `", "`; if the data was written one entry per line, a template re-emits it on one line.
  Keep the separators as written?
- [ ] No `flake.nix`, so no dev shell of its own; this session used fs-catasto's (`nix develop ../fs-catasto`). Give it one, or join
  the toolchain that sisho brings.
- [ ] `tmtroot/idea.tmt` has uncommitted edits of the user (`sort`); leave them.
- [ ] `git push` on 2026-10-24 published the user's local commits `c151906` and the two before it (told, nothing to do).
