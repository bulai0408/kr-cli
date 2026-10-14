use std::time::Duration;

use kr_run::{input_hash, longest_first};

#[test]
fn work_is_ordered_longest_first_with_unknown_durations_first() {
    let order = longest_first(vec!["a", "b", "c", "d"], |name| match *name {
        "a" => Some(Duration::from_secs(10)),
        "b" => Some(Duration::from_secs(90)),
        "c" => None,
        _ => Some(Duration::from_secs(30)),
    });
    assert_eq!(order, ["c", "b", "d", "a"]);
}

#[test]
fn the_input_hash_changes_with_file_contents_and_the_command_but_not_mtime() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("A.swift"), "let a = 1").unwrap();
    std::fs::write(dir.path().join("project.yml"), "name: X").unwrap();
    let files = vec!["A.swift".to_owned(), "project.yml".to_owned()];

    let h1 = input_hash(dir.path(), &files, &["xcodebuild", "build-for-testing"]).unwrap();
    std::fs::write(dir.path().join("A.swift"), "let a = 1").unwrap();
    assert_eq!(
        input_hash(dir.path(), &files, &["xcodebuild", "build-for-testing"]).unwrap(),
        h1
    );

    std::fs::write(dir.path().join("A.swift"), "let a = 2").unwrap();
    let h2 = input_hash(dir.path(), &files, &["xcodebuild", "build-for-testing"]).unwrap();
    assert_ne!(h1, h2);
    assert_ne!(
        input_hash(dir.path(), &files, &["xcodebuild", "build"]).unwrap(),
        h2
    );
}

#[test]
fn a_listed_file_that_was_deleted_still_hashes() {
    let dir = tempfile::tempdir().unwrap();
    let files = vec!["gone.swift".to_owned()];
    assert!(input_hash(dir.path(), &files, &[]).is_ok());
}
