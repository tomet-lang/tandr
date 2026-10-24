//! `tandr run` over directories, driven through the real binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RULES: &str = r#"
doc_kind: { match: "@kind", replace: { head: "::kind" } }
"#;

/// A vault with a frozen directory, as tomet's own repository has, and a rule file.
fn vault(name: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("tandr_cli_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("frozen")).unwrap();
    fs::create_dir_all(dir.join("notes")).unwrap();
    fs::write(
        dir.join("default.config.tmt"),
        "::kind(config)\n::config{ workspace: { unswept: list(\"frozen\") } }\n",
    )
    .unwrap();
    for f in ["a.tmt", "notes/b.tmt", "frozen/c.tmt"] {
        fs::write(dir.join(f), "@kind(note)\n\nbody\n").unwrap();
    }
    fs::write(dir.join("notes/plain.tmt"), "no kind here\n").unwrap();
    let rules = dir.join("migrate.tandr");
    fs::write(&rules, RULES).unwrap();
    (dir, rules)
}

fn tandr(rules: &Path, args: &[&str], targets: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tandr"))
        .arg("run")
        .arg("-r")
        .arg(rules)
        .args(args)
        .args(targets)
        .output()
        .unwrap()
}

fn text(o: &[u8]) -> String {
    String::from_utf8_lossy(o).into_owned()
}

#[test]
fn a_directory_is_rewritten_in_place_except_what_the_config_leaves_alone() {
    let (dir, rules) = vault("inplace");
    let out = tandr(&rules, &["-i"], &[&dir]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        fs::read_to_string(dir.join("a.tmt")).unwrap(),
        "::kind(note)\n\nbody\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("notes/b.tmt")).unwrap(),
        "::kind(note)\n\nbody\n"
    );
    // the frozen directory and the file with no match are as they were
    assert_eq!(
        fs::read_to_string(dir.join("frozen/c.tmt")).unwrap(),
        "@kind(note)\n\nbody\n"
    );
    assert_eq!(
        fs::read_to_string(dir.join("notes/plain.tmt")).unwrap(),
        "no kind here\n"
    );
    // the config file is a document too; it already says `::kind(config)` (tomet reads no
    // other spelling), so the rules leave it as it is
    assert!(
        fs::read_to_string(dir.join("default.config.tmt"))
            .unwrap()
            .starts_with("::kind(config)\n")
    );
    assert!(
        text(&out.stdout).contains("2 of 4 file(s) changed"),
        "{}",
        text(&out.stdout)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn force_rewrites_the_frozen_directory_too() {
    let (dir, rules) = vault("force");
    let out = tandr(&rules, &["-i", "--force"], &[&dir]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        fs::read_to_string(dir.join("frozen/c.tmt")).unwrap(),
        "::kind(note)\n\nbody\n"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn check_exits_nonzero_for_a_change_and_writes_nothing() {
    let (dir, rules) = vault("check");
    let out = tandr(&rules, &["--check"], &[&dir]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    assert_eq!(
        fs::read_to_string(dir.join("a.tmt")).unwrap(),
        "@kind(note)\n\nbody\n"
    );

    // once migrated there is nothing left to do
    assert!(tandr(&rules, &["-i"], &[&dir]).status.success());
    let again = tandr(&rules, &["--check"], &[&dir]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_directory_without_in_place_diff_or_check_is_an_error() {
    let (dir, rules) = vault("bare");
    let out = tandr(&rules, &[], &[&dir]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("--in-place"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.join("a.tmt")).unwrap(),
        "@kind(note)\n\nbody\n"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_named_frozen_file_is_skipped_with_a_warning_until_forced() {
    let (dir, rules) = vault("named");
    let frozen = dir.join("frozen/c.tmt");
    let out = tandr(&rules, &["-i"], &[&frozen]);
    assert!(out.status.success());
    assert!(
        text(&out.stderr).contains("skipped"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(&frozen).unwrap(),
        "@kind(note)\n\nbody\n"
    );

    assert!(
        tandr(&rules, &["-i", "--force"], &[&frozen])
            .status
            .success()
    );
    assert_eq!(
        fs::read_to_string(&frozen).unwrap(),
        "::kind(note)\n\nbody\n"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn one_file_that_fails_does_not_stop_the_rest() {
    let (dir, rules) = vault("failure");
    // not valid utf-8, so it cannot be read as text
    fs::write(dir.join("a-bad.tmt"), [0xff, 0xfe, 0x00]).unwrap();
    let out = tandr(&rules, &["-i"], &[&dir]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("a-bad.tmt"),
        "{}",
        text(&out.stderr)
    );
    assert!(
        text(&out.stderr).contains("1 file(s) failed"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.join("notes/b.tmt")).unwrap(),
        "::kind(note)\n\nbody\n"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_rule_is_reported_once_and_nothing_is_touched() {
    let (dir, _) = vault("badrule");
    let rules = dir.join("bad.tandr");
    fs::write(&rules, "r: { match: \"@kind\", replace: \"?(missing)\" }\n").unwrap();
    let out = tandr(&rules, &["-i"], &[&dir]);
    assert!(!out.status.success());
    let err = text(&out.stderr);
    assert_eq!(err.matches("missing").count(), 1, "{err}");
    assert_eq!(
        fs::read_to_string(dir.join("a.tmt")).unwrap(),
        "@kind(note)\n\nbody\n"
    );
    let _ = fs::remove_dir_all(&dir);
}
