//! Integration tests for `akku::fs`.

use std::io::ErrorKind;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use akku::fs::{
    glob, walk, write_atomic, GlobError, GlobMatcher, PatternIssue, TempDir, TempFile, WalkBuilder,
    WalkEntry,
};

/// Create `root/rel` holding `bytes`, creating missing parents as needed.
fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent directories");
    }
    std::fs::write(path, bytes).expect("write test file");
}

/// The file name of an entry, as display text.
fn base(entry: &WalkEntry) -> String {
    entry
        .path
        .file_name()
        .expect("entry has a file name")
        .to_string_lossy()
        .into_owned()
}

/// (`depth`, `file-name`) pairs, sorted by name.
fn pairs<'a>(entries: impl IntoIterator<Item = &'a WalkEntry>) -> Vec<(usize, String)> {
    let mut got: Vec<(usize, String)> = entries
        .into_iter()
        .map(|entry| (entry.depth, base(entry)))
        .collect();
    got.sort();
    got
}

/// Children only (depth > 0), name-sorted.
fn child_pairs(entries: &[WalkEntry]) -> Vec<(usize, String)> {
    pairs(entries.iter().filter(|entry| entry.depth > 0))
}

/// ```text
/// root/ { a.txt, d1/d2/d3/f, sub/b.txt }
/// ```
fn walk_tree() -> TempDir {
    let root = TempDir::new("walk-tree").unwrap();
    put(root.as_path(), "a.txt", b"a");
    put(root.as_path(), "d1/d2/d3/f", b"f");
    put(root.as_path(), "sub/b.txt", b"b");
    root
}

#[test]
fn walk_yields_root_with_children_and_depths() {
    let root = walk_tree();
    let entries: Vec<WalkEntry> = walk(root.as_path()).map(|entry| entry.unwrap()).collect();
    assert_eq!(entries.len(), 8);
    assert_eq!(
        child_pairs(&entries),
        vec![
            (1, "a.txt".to_owned()),
            (1, "d1".to_owned()),
            (1, "sub".to_owned()),
            (2, "b.txt".to_owned()),
            (2, "d2".to_owned()),
            (3, "d3".to_owned()),
            (4, "f".to_owned()),
        ]
    );
    let roots: Vec<&WalkEntry> = entries.iter().filter(|entry| entry.depth == 0).collect();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].path, root.as_path());
    assert!(roots[0].is_dir());
    assert!(!roots[0].is_file());
}

#[test]
fn walk_max_depth_limits_descend() {
    let root = walk_tree();
    let shallow: Vec<WalkEntry> = WalkBuilder::new(root.as_path())
        .max_depth(1)
        .build()
        .filter_map(Result::ok)
        .collect();
    // root, a.txt, d1, sub.
    assert_eq!(shallow.len(), 4);
    assert!(child_pairs(&shallow).contains(&(1, "d1".to_owned())));
    assert!(!shallow.iter().any(|entry| base(entry) == "d2"));

    let deepest: Vec<WalkEntry> = WalkBuilder::new(root.as_path())
        .max_depth(0)
        .build()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(deepest.len(), 1);
    assert_eq!(deepest[0].path, root.as_path());
}

#[test]
fn walk_hidden_names_are_filtered() {
    let root = TempDir::new("walk-hidden").unwrap();
    put(root.as_path(), "visible.txt", b"v");
    put(root.as_path(), ".hidden.txt", b"h");
    put(root.as_path(), ".hiddendir/inside.txt", b"i");

    let without: Vec<WalkEntry> = walk(root.as_path()).filter_map(Result::ok).collect();
    // root + visible.txt only.
    assert_eq!(without.len(), 2);
    assert!(!without.iter().any(|entry| base(entry).starts_with('.')));

    let with: Vec<WalkEntry> = WalkBuilder::new(root.as_path())
        .include_hidden(true)
        .build()
        .filter_map(Result::ok)
        .collect();
    // root, visible.txt, .hidden.txt, .hiddendir, inside.txt.
    assert_eq!(with.len(), 5);
    assert!(child_pairs(&with).contains(&(2, "inside.txt".to_owned())));
}

#[test]
fn walk_sort_by_name_is_deterministic() {
    let root = TempDir::new("walk-sort").unwrap();
    put(root.as_path(), "c.txt", b"c");
    put(root.as_path(), "a.txt", b"a");
    put(root.as_path(), "folder/z.txt", b"z");
    put(root.as_path(), "b.txt", b"b");
    put(root.as_path(), "folder/y.txt", b"y");

    let sorted: Vec<WalkEntry> = WalkBuilder::new(root.as_path())
        .sort_by_name(true)
        .build()
        .map(|entry| entry.unwrap())
        .collect();
    let names: Vec<String> = sorted.iter().map(base).collect();
    // The root comes first, then each level in sorted order.
    let expected: Vec<String> = ["a.txt", "b.txt", "c.txt", "folder", "y.txt", "z.txt"]
        .iter()
        .map(|name| name.to_string())
        .collect();
    assert_eq!(&names[1..], &expected[..]);
}

#[test]
fn walk_symlink_cycle_terminates() {
    let root = TempDir::new("walk-cycle").unwrap();
    put(root.as_path(), "sub/in.txt", b"i");
    let link = root.as_path().join("sub/loop");
    std::os::unix::fs::symlink(root.as_path(), &link).unwrap();

    // Following symlinks must hit the cycle guard instead of looping.
    let followed: Vec<WalkEntry> = WalkBuilder::new(root.as_path())
        .follow_symlinks(true)
        .build()
        .map(|entry| entry.unwrap())
        .collect();
    assert_eq!(
        child_pairs(&followed),
        vec![
            (1, "sub".to_owned()),
            (2, "in.txt".to_owned()),
            (2, "loop".to_owned()),
        ]
    );
    let looped = followed.iter().find(|entry| base(entry) == "loop").unwrap();
    assert!(looped.is_dir());

    // Without following, the symlink stays a symlink.
    let plain: Vec<WalkEntry> = walk(root.as_path()).filter_map(Result::ok).collect();
    assert!(plain.iter().any(WalkEntry::is_symlink));
    assert_eq!(
        plain.iter().filter(|entry| base(entry) == "loop").count(),
        1
    );
}

#[test]
fn walk_missing_root_reports_then_skips() {
    let missing = std::env::temp_dir().join("akku-walk-missing-root");
    let wrong = walk(&missing).next().unwrap().unwrap_err();
    assert_eq!(wrong.path, missing);
    assert_eq!(wrong.io_error.kind(), ErrorKind::NotFound);

    let empty: Vec<WalkEntry> = WalkBuilder::new(&missing)
        .skip_errors(true)
        .build()
        .filter_map(Result::ok)
        .collect();
    assert!(empty.is_empty());
}

#[test]
fn walk_file_root_yields_single_entry() {
    let file = TempFile::new("walk-file-root").unwrap();
    file.write(b"content").unwrap();
    let entries: Vec<WalkEntry> = walk(file.as_path()).map(|entry| entry.unwrap()).collect();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].is_file());
    assert_eq!(entries[0].depth, 0);
    assert_eq!(entries[0].metadata().unwrap().len(), 7);
}

#[test]
fn glob_double_star_expands_to_any_segments() {
    let m = |pattern: &str, path: &str| matcher(pattern).is_match(path);
    assert!(m("**/foo/*.rs", "foo/a.rs"));
    assert!(m("**/foo/*.rs", "x/foo/b.rs"));
    assert!(m("**/foo/*.rs", "x/y/foo/zz.rs"));
    assert!(!m("**/foo/*.rs", "foo/sub/x.rs"));
    assert!(!m("**/foo/*.rs", "foo/a.txt"));

    assert!(m("foo/**", "foo"));
    assert!(m("foo/**", "foo/a.rs"));
    assert!(m("foo/**", "foo/x/y/z.txt"));
    assert!(!m("foo/**", "fooo"));

    assert!(m("a/**/b", "a/b"));
    assert!(m("a/**/b", "a/x/b"));
    assert!(m("a/**/b", "a/x/y/b"));
    assert!(!m("a/**/b", "b"));
    assert!(!m("a/**/b", "a"));
}

#[test]
fn glob_star_stays_within_one_segment() {
    let m = |pattern: &str, path: &str| matcher(pattern).is_match(path);
    assert!(m("*.rs", "main.rs"));
    assert!(!m("*.rs", "src/main.rs"));
    assert!(m("?at", "hat"));
    assert!(!m("?at", "that"));
    assert!(!m("?at", "at"));
}

#[test]
fn glob_char_classes_match_members() {
    let m = |pattern: &str, path: &str| matcher(pattern).is_match(path);
    assert!(m("[a-c]at", "bat"));
    assert!(m("[a-c]at", "cat"));
    assert!(!m("[a-c]at", "rat"));
    assert!(m("[!a-c]at", "rat"));
    assert!(!m("[!a-c]at", "bat"));
    assert!(m("x[0-9]y", "x7y"));
    assert!(!m("x[0-9]y", "xey"));
}

#[test]
fn glob_braces_alternate() {
    let m = |pattern: &str, path: &str| matcher(pattern).is_match(path);
    assert!(m("*.{rs,toml}", "a.rs"));
    assert!(m("*.{rs,toml}", "b.toml"));
    assert!(!m("*.{rs,toml}", "c.txt"));
    assert!(m("a{,b}c", "ac"));
    assert!(m("a{,b}c", "abc"));
    assert!(!m("a{,b}c", "abbc"));
}

#[test]
fn glob_escapes_are_literal() {
    let m = |pattern: &str, path: &str| matcher(pattern).is_match(path);
    assert!(m(r"a\*b", "a*b"));
    assert!(!m(r"a\*b", "aab"));
    assert!(m(r"a\?b", "a?b"));
    assert!(!m(r"a\?b", "axb"));
}

/// Compile a pattern or fail the test.
fn matcher(pattern: &str) -> GlobMatcher {
    GlobMatcher::new(pattern)
        .unwrap_or_else(|err| panic!("pattern {pattern:?} did not compile: {err}"))
}

#[test]
fn glob_syntax_errors_report_position_and_issue() {
    fn syntax(pattern: &str) -> (usize, PatternIssue) {
        match GlobMatcher::new(pattern).unwrap_err() {
            GlobError::Syntax { position, issue } => (position, issue),
            other => panic!("expected a syntax error, got {other:?}"),
        }
    }

    assert_eq!(syntax("a\\"), (1, PatternIssue::UnexpectedEscape));
    assert_eq!(syntax("a[bd"), (1, PatternIssue::UnclosedClass));
    assert_eq!(syntax("a{b"), (1, PatternIssue::UnclosedBrace));
    assert_eq!(syntax("a{b{c,d},e}"), (3, PatternIssue::NestedBrace));
    assert_eq!(syntax("a{}"), (1, PatternIssue::EmptyBrace));
    assert_eq!(syntax("a**b"), (1, PatternIssue::DoubleStarNotWholeSegment));
    assert_eq!(syntax("**a"), (0, PatternIssue::DoubleStarNotWholeSegment));
    assert_eq!(syntax("a**"), (1, PatternIssue::DoubleStarNotWholeSegment));
    assert_eq!(syntax("[z-a]"), (1, PatternIssue::InvalidClassRange));

    // The expansion path surfaces the same error via `glob`.
    assert!(matches!(
        glob("a**b"),
        Err(GlobError::Syntax {
            issue: PatternIssue::DoubleStarNotWholeSegment,
            ..
        })
    ));
}

#[test]
fn glob_matcher_from_str_and_pattern_access() {
    let matcher: GlobMatcher = "docs/**/note-*.md".parse().unwrap();
    assert!(matcher.is_match("docs/fs/note-a-1.md"));
    assert!(!matcher.is_match("src/fs/note-a-1.md"));
    assert_eq!(matcher.pattern(), "docs/**/note-*.md");
}

#[test]
fn glob_expands_the_file_system() {
    let root = TempDir::new("glob-fs").unwrap();
    put(root.as_path(), "a.rs", b"");
    put(root.as_path(), "readme.txt", b"");
    put(root.as_path(), "sub/c.rs", b"");
    put(root.as_path(), "sub/deep/d.rs", b"");
    let root = root.as_path();

    let pattern = format!("{}/**/*.rs", root.display());
    let found = glob(&pattern).unwrap();
    assert_eq!(
        found,
        vec![
            root.join("a.rs"),
            root.join("sub/c.rs"),
            root.join("sub/deep/d.rs"),
        ]
    );

    let shallow = glob(&format!("{}/*.rs", root.display())).unwrap();
    assert_eq!(shallow, vec![root.join("a.rs")]);

    let missing = glob(&format!("{}/none/*.rs", root.display())).unwrap();
    assert!(missing.is_empty());

    let braces = glob(&format!("{}/{{a,b}}.rs", root.display())).unwrap();
    assert_eq!(braces, vec![root.join("a.rs")]);
}

#[test]
fn write_atomic_creates_and_overwrites() {
    let target = std::env::temp_dir().join("akku-fs-atomic-target.txt");
    let _ = std::fs::remove_file(&target);
    write_atomic(&target, b"one").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "one");
    write_atomic(&target, b"twotwotwo").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "twotwotwo");
    std::fs::remove_file(&target).unwrap();
}

#[test]
fn write_atomic_syncs_parent_dir_for_nested_and_relative_paths() {
    // Nested parent: the post-rename directory fsync must succeed and keep the data.
    let dir = TempDir::new("atomic-dirsync").unwrap();
    std::fs::create_dir(dir.as_path().join("sub")).unwrap();
    let nested = dir.as_path().join("sub/file.bin");
    write_atomic(&nested, b"durable").unwrap();
    write_atomic(&nested, b"durable-2").unwrap();
    assert_eq!(std::fs::read(&nested).unwrap(), b"durable-2");
    let leftovers: Vec<_> = std::fs::read_dir(dir.as_path().join("sub"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers.len(),
        1,
        "no temp files may remain: {leftovers:?}"
    );

    // A bare file name has no parent component: the current directory is synced.
    let name = format!("akku-atomic-rel-{}.tmp", std::process::id());
    write_atomic(&name, b"rel").unwrap();
    assert_eq!(std::fs::read(&name).unwrap(), b"rel");
    std::fs::remove_file(&name).unwrap();
}

#[test]
fn write_atomic_fails_for_missing_parent() {
    let dir = TempDir::new("atomic-missing").unwrap();
    let target = dir.as_path().join("nope/missing/file.txt");
    let err = write_atomic(&target, b"").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[test]
fn temp_file_writes_persists_and_roundtrips() {
    let file = TempFile::new("tf-roundtrip").unwrap();
    assert!(file.as_path().exists());
    file.write(b"payload").unwrap();
    assert_eq!(std::fs::read_to_string(file.as_path()).unwrap(), "payload");

    let kept = file.persist().unwrap();
    assert!(kept.exists());
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "payload");
    std::fs::remove_file(kept).unwrap();
}

#[test]
fn temp_file_cleans_up_on_drop() {
    let path;
    {
        let file = TempFile::new("tf-raii").unwrap();
        path = file.as_path().to_owned();
        assert!(path.exists());
    }
    assert!(!path.exists());
}

#[test]
fn temp_dir_cleans_up_on_drop_and_persists() {
    let gone;
    {
        let dir = TempDir::new("td-raii").unwrap();
        put(dir.as_path(), "inner/data.txt", b"x");
        gone = dir.as_path().join("inner/data.txt");
        assert!(gone.exists());
    }
    assert!(!gone.exists());

    let dir = TempDir::new("td-persist").unwrap();
    put(dir.as_path(), "keep.txt", b"k");
    let kept = dir.persist().unwrap();
    assert!(kept.join("keep.txt").exists());
    std::fs::remove_dir_all(kept).unwrap();
}

#[test]
fn temp_prefixes_are_validated() {
    assert_eq!(
        TempFile::new("bad slash/name").unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        TempDir::new("weird*name!").unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    assert_eq!(
        TempDir::new("x".repeat(80).as_str()).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    // An empty prefix falls back to a default name.
    let file = TempFile::new("").unwrap();
    assert!(file.as_path().exists());
}

#[test]
fn fs_reexports_are_usable() {
    let dir = TempDir::new("reexports").unwrap();
    let canonical = akku::fs::canonicalize(dir.as_path()).unwrap();
    assert_eq!(canonical, akku::fs::canonicalize(&canonical).unwrap());
    assert!(akku::fs::try_exists(dir.as_path()).unwrap());

    let mut file = akku::fs::File::create(dir.as_path().join("f.txt")).unwrap();
    file.write_all(b"hi").unwrap();
    assert_eq!(
        akku::fs::read_to_string(dir.as_path().join("f.txt")).unwrap(),
        "hi"
    );
    let listed: Vec<PathBuf> = akku::fs::read_dir(dir.as_path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert!(listed.contains(&dir.as_path().join("f.txt")));
}

#[cfg(unix)]
#[test]
fn glob_symlink_cycle_terminates_without_following() {
    let root = TempDir::new("glob-symlink-cycle").unwrap();
    put(root.as_path(), "a.txt", b"a");
    put(root.as_path(), "sub/b.txt", b"b");
    let link = root.as_path().join("loop");
    std::os::unix::fs::symlink(root.as_path(), &link).unwrap();

    let all = glob(&format!("{}/**", root.as_path().display())).unwrap();
    assert!(all.contains(&root.as_path().join("a.txt")));
    assert!(all.contains(&root.as_path().join("sub/b.txt")));
    assert!(all.contains(&link));
    assert!(!all
        .iter()
        .any(|path| path.starts_with(&link) && path != &link));

    let txts = glob(&format!("{}/**/*.txt", root.as_path().display())).unwrap();
    assert_eq!(
        txts,
        vec![
            root.as_path().join("a.txt"),
            root.as_path().join("sub/b.txt"),
        ]
    );
}

#[cfg(unix)]
#[test]
fn walk_broken_symlink_yields_error_and_siblings() {
    let root = TempDir::new("walk-broken-sibling").unwrap();
    put(root.as_path(), "a_valid.txt", b"ok");
    put(root.as_path(), "z_valid.txt", b"ok2");
    let broken = root.as_path().join("m_broken");
    std::os::unix::fs::symlink(root.as_path().join("does-not-exist"), &broken).unwrap();

    let items: Vec<_> = WalkBuilder::new(root.as_path())
        .follow_symlinks(true)
        .sort_by_name(true)
        .build()
        .collect();

    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for item in items {
        match item {
            Ok(entry) => entries.push(entry.path),
            Err(err) => errors.push(err.path),
        }
    }

    assert_eq!(errors, vec![broken]);
    assert!(entries.contains(&root.as_path().join("a_valid.txt")));
    assert!(entries.contains(&root.as_path().join("z_valid.txt")));
}
