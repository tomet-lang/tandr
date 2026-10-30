//! `tandr run` over directories, driven through the real binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tomet_load_config::VAULT_CONFIG_FILE;

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
        dir.join(VAULT_CONFIG_FILE),
        "::kind(config)\n::workspace{ unswept: list(\"frozen\") }\n",
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
        fs::read_to_string(dir.join(VAULT_CONFIG_FILE))
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
    fs::write(
        &rules,
        "r: { match: \"@kind\", replace: \"${hole.missing}\" }\n",
    )
    .unwrap();
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

fn code(o: &Output) -> Option<i32> {
    o.status.code()
}

#[test]
fn exit_codes_tell_changed_from_failed() {
    let (dir, rules) = vault("codes");

    // 0: a file the rules do not touch
    let plain = dir.join("notes/plain.tmt");
    assert_eq!(code(&tandr(&rules, &["--check"], &[&plain])), Some(0));
    // 1: --check finds a change; without --check a change is not a failure
    let a = dir.join("a.tmt");
    assert_eq!(code(&tandr(&rules, &["--check"], &[&a])), Some(1));
    assert_eq!(code(&tandr(&rules, &["--diff"], &[&a])), Some(0));

    // 2: a bad rule file, a missing target, a missing rule file
    let bad = dir.join("bad.tandr");
    fs::write(&bad, "r: { match: \"@x(?nope(n))\" }\n").unwrap();
    let out = tandr(&bad, &["--check"], &[&a]);
    assert_eq!(code(&out), Some(2), "{}", text(&out.stderr));
    let gone = dir.join("gone.tmt");
    assert_eq!(code(&tandr(&rules, &["--check"], &[&gone])), Some(2));
    assert_eq!(
        code(&tandr(&dir.join("gone.tandr"), &["--check"], &[&a])),
        Some(2)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_file_is_two_even_when_another_would_change_and_the_rest_still_runs() {
    let (dir, rules) = vault("failed");
    // not UTF-8: it cannot be read as a document
    fs::write(dir.join("notes/binary.tmt"), [0xff, 0xfe, 0x00]).unwrap();

    let checked = tandr(&rules, &["--check"], &[&dir]);
    assert_eq!(code(&checked), Some(2), "{}", text(&checked.stderr));

    let written = tandr(&rules, &["-i"], &[&dir]);
    assert_eq!(code(&written), Some(2));
    assert_eq!(
        fs::read_to_string(dir.join("a.tmt")).unwrap(),
        "::kind(note)\n\nbody\n",
        "one bad file does not stop the others"
    );
    let _ = fs::remove_dir_all(&dir);
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(&out.stdout)))
}

#[test]
fn format_json_reports_each_file_and_keeps_the_exit_code() {
    let (dir, rules) = vault("json");
    let out = tandr(&rules, &["--check", "--format", "json"], &[&dir]);
    assert_eq!(code(&out), Some(1), "{}", text(&out.stderr));
    let report = json(&out);
    assert_eq!(report["changed"], 2);
    assert_eq!(report["failed"], 0);
    let files = report["files"].as_array().unwrap();
    assert_eq!(files.len(), 4);
    let a = files
        .iter()
        .find(|f| f["path"].as_str().unwrap().ends_with("a.tmt"))
        .unwrap();
    assert_eq!(a["changed"], true);
    assert_eq!(a["changes"], 1);
    assert!(a.get("error").is_none());
    let plain = files
        .iter()
        .find(|f| f["path"].as_str().unwrap().ends_with("plain.tmt"))
        .unwrap();
    assert_eq!(plain["changed"], false);
    // --check wrote nothing
    assert_eq!(
        fs::read_to_string(dir.join("a.tmt")).unwrap(),
        "@kind(note)\n\nbody\n"
    );

    // after -i, the same report says nothing is left and exits 0
    let written = tandr(&rules, &["-i", "--format", "json"], &[&dir]);
    assert_eq!(code(&written), Some(0));
    assert_eq!(json(&written)["changed"], 2);
    let again = tandr(&rules, &["--check", "--format", "json"], &[&dir]);
    assert_eq!(code(&again), Some(0));
    assert_eq!(json(&again)["changed"], 0);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn format_json_names_the_file_that_failed() {
    let (dir, rules) = vault("jsonfail");
    fs::write(dir.join("notes/binary.tmt"), [0xff, 0xfe, 0x00]).unwrap();
    let out = tandr(&rules, &["--check", "--format", "json"], &[&dir]);
    assert_eq!(code(&out), Some(2));
    let report = json(&out);
    assert_eq!(report["failed"], 1);
    let bad = report["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f.get("error").is_some())
        .unwrap();
    assert!(bad["path"].as_str().unwrap().ends_with("binary.tmt"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn format_json_needs_check_or_in_place() {
    let (dir, rules) = vault("jsonbare");
    let a = dir.join("a.tmt");
    let out = tandr(&rules, &["--format", "json"], &[&a]);
    assert_eq!(code(&out), Some(2));
    assert!(
        text(&out.stderr).contains("--check"),
        "{}",
        text(&out.stderr)
    );
    assert!(out.stdout.is_empty());
    let _ = fs::remove_dir_all(&dir);
}
