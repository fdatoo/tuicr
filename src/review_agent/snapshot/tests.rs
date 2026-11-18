use super::*;
use crate::forge::local_git::test_support::{git, init_repo_with_commit};

struct Fixture {
    _scratch: tempfile::TempDir,
    repo: PathBuf,
    cache: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        // macOS's temp dir may be returned via a system symlink; production
        // private cache directories intentionally refuse symlink components.
        let root = scratch.path().canonicalize().unwrap();
        let repo = root.join("repo");
        fs::create_dir(&repo).unwrap();
        Self {
            _scratch: scratch,
            repo,
            cache: root.join("cache"),
        }
    }

    fn local(&self, target: LocalTarget) -> Snapshot {
        SnapshotRequest::local(
            self.repo.clone(),
            VcsType::Git,
            target,
            None,
            false,
            Vec::new(),
        )
        .materialize_in(self.cache.clone())
        .unwrap()
    }

    fn file(&self, path: &str, content: &str) {
        let path = self.repo.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = cache::make_removable(&self.cache);
    }
}

fn file(snapshot: &Snapshot, name: &str) -> String {
    fs::read_to_string(snapshot.dir().join(name)).unwrap()
}

#[test]
fn review_snapshot_separates_commit_index_and_live_worktree() {
    let fixture = Fixture::new();
    let base = init_repo_with_commit(&fixture.repo, &["base"]);
    fixture.file("src/state.txt", "at pinned commit");
    fixture.file("src/unchanged.txt", "caller outside diff");
    fixture.file(".gitignore", "ignored.txt\n");
    git(&fixture.repo, &["add", "."]);
    git(&fixture.repo, &["commit", "-qm", "pin this version"]);
    let sha = git(&fixture.repo, &["rev-parse", "HEAD"]).trim().to_owned();
    fixture.file("src/state.txt", "staged content");
    git(&fixture.repo, &["add", "src/state.txt"]);
    fixture.file("src/state.txt", "dirty worktree content");
    fixture.file("new.txt", "untracked content");
    fixture.file("ignored.txt", "not reviewed");
    let index = fixture.local(LocalTarget::Index);
    let live = fixture.local(LocalTarget::Worktree);
    let commit = fixture.local(LocalTarget::Revision(sha.clone()));
    let parent = fixture.local(LocalTarget::ParentOf(sha));
    assert_eq!(file(&index, "src/state.txt"), "staged content");
    assert!(!index.dir().join("new.txt").exists());
    assert_eq!(file(&commit, "src/state.txt"), "at pinned commit");
    assert_eq!(file(&live, "src/state.txt"), "dirty worktree content");
    assert_eq!(file(&live, "new.txt"), "untracked content");
    assert!(!live.dir().join("ignored.txt").exists());
    assert_eq!(file(&index, "src/unchanged.txt"), "caller outside diff");
    assert_eq!(file(&commit, "src/unchanged.txt"), "caller outside diff");
    assert_eq!(file(&live, "src/unchanged.txt"), "caller outside diff");
    assert!(!parent.dir().join("src/state.txt").exists());
    assert_eq!(
        parent.dir(),
        fixture.local(LocalTarget::Revision(base)).dir()
    );
    assert_eq!(live.dir(), fixture.local(LocalTarget::Worktree).dir());
    fixture.file(
        "src/unchanged.txt",
        "a tool must now read different content",
    );
    let newer = fixture.local(LocalTarget::Worktree);
    assert_ne!(live.dir(), newer.dir());
    assert_eq!(file(&live, "src/unchanged.txt"), "caller outside diff");
}

#[test]
fn review_snapshot_honors_exact_names_and_keeps_unexpanded_git_objects() {
    let fixture = Fixture::new();
    init_repo_with_commit(&fixture.repo, &["base"]);
    fixture.file("[name].txt", "$Format:%H$");
    fixture.file("name.txt", "not selected");
    fixture.file(
        "omitted.txt",
        "an export-ignore file is still reviewed source",
    );
    fixture.file(
        ".gitattributes",
        "*.txt export-subst\nomitted.txt export-ignore\n",
    );
    git(&fixture.repo, &["add", "."]);
    git(&fixture.repo, &["commit", "-qm", "archive attributes"]);
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]).trim().to_owned();
    let full = fixture.local(LocalTarget::Revision(head.clone()));
    assert_eq!(file(&full, "[name].txt"), "$Format:%H$");
    assert!(file(&full, "omitted.txt").contains("still reviewed source"));
    let exact = SnapshotRequest::local(
        fixture.repo.clone(),
        VcsType::Git,
        LocalTarget::Revision(head),
        Some(PathBuf::from("[name].txt")),
        false,
        Vec::new(),
    )
    .materialize_in(fixture.cache.clone())
    .unwrap();
    assert_eq!(file(&exact, "[name].txt"), "$Format:%H$");
    assert!(!exact.dir().join("name.txt").exists());
}

#[test]
fn review_snapshot_never_reads_default_index_for_alternate_index() {
    let fixture = Fixture::new();
    init_repo_with_commit(&fixture.repo, &["base"]);
    fixture.file("anchored.txt", "shown in review");
    fixture.file("other.txt", "not an anchor");
    git(&fixture.repo, &["add", "."]);
    let side = |target| {
        SnapshotRequest::local(
            fixture.repo.clone(),
            VcsType::Git,
            target,
            None,
            true,
            vec![PathBuf::from("anchored.txt")],
        )
        .materialize_in(fixture.cache.clone())
        .unwrap()
    };
    let index = side(LocalTarget::Index);
    assert!(index.limits().unwrap().contains("alternate index"));
    assert_eq!(fs::read_dir(index.dir()).unwrap().count(), 0);
    let live = side(LocalTarget::Worktree);
    assert_eq!(file(&live, "anchored.txt"), "shown in review");
    assert!(!live.dir().join("other.txt").exists());
    assert!(live.limits().unwrap().contains("exact changed paths"));
}

#[test]
fn review_snapshot_uses_matching_pr_objects_not_the_checkout() {
    let fixture = Fixture::new();
    let base = init_repo_with_commit(&fixture.repo, &["base"]);
    let original = fs::read_to_string(fixture.repo.join("file.txt")).unwrap();
    fixture.file("file.txt", "at pr head");
    git(&fixture.repo, &["commit", "-qam", "reviewed head"]);
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]).trim().to_owned();
    fixture.file("file.txt", "on the user's worktree");
    git(
        &fixture.repo,
        &[
            "remote",
            "add",
            "origin",
            "git@gitlab.snapshot.invalid:review/tree.git",
        ],
    );
    let repository = ForgeRepository {
        kind: ForgeKind::GitLab,
        host: "gitlab.snapshot.invalid".into(),
        owner: "review".into(),
        name: "tree".into(),
    };
    let side = |sha| {
        SnapshotRequest::pull_request(Some(fixture.repo.clone()), repository.clone(), sha)
            .materialize_in(fixture.cache.clone())
            .unwrap()
    };
    assert_eq!(file(&side(Some(head.clone())), "file.txt"), "at pr head");
    assert_eq!(file(&side(Some(base)), "file.txt"), original);
    let unconfirmed = side(None);
    assert_eq!(fs::read_dir(unconfirmed.dir()).unwrap().count(), 0);
    assert!(
        unconfirmed
            .limits()
            .unwrap()
            .contains("No pinned source revision")
    );
    git(
        &fixture.repo,
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.snapshot.invalid:other/tree.git",
        ],
    );
    let wrong_checkout = side(Some(head));
    assert_eq!(fs::read_dir(wrong_checkout.dir()).unwrap().count(), 0);
    assert!(
        wrong_checkout
            .limits()
            .unwrap()
            .contains("No matching local Git objects")
    );
}

#[test]
#[cfg(unix)]
fn review_snapshot_refuses_symlinks_private_paths_and_oversize_data() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let fixture = Fixture::new();
    init_repo_with_commit(&fixture.repo, &["base"]);
    let outside = fixture.repo.parent().unwrap().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("source.txt"), "private").unwrap();
    fixture.file("linked_dir/source.txt", "previously tracked");
    git(&fixture.repo, &["add", "linked_dir/source.txt"]);
    fs::remove_dir_all(fixture.repo.join("linked_dir")).unwrap();
    symlink(&outside, fixture.repo.join("linked_dir")).unwrap();
    symlink(outside.join("source.txt"), fixture.repo.join("linked_file")).unwrap();
    fixture.file(".env.local", "secret");
    fixture.file(".claude/settings.json", "provider configuration");
    File::create(fixture.repo.join("too-large"))
        .unwrap()
        .set_len(MAX_FILE_BYTES + 1)
        .unwrap();
    let snapshot = fixture.local(LocalTarget::Worktree);
    for item in [
        ".git",
        ".env.local",
        ".claude",
        "linked_dir",
        "linked_file",
        "too-large",
    ] {
        assert!(
            !snapshot.dir().join(item).exists(),
            "unsafe path {item} was copied"
        );
    }
    let limits = snapshot.limits().unwrap();
    assert!(limits.contains("larger than 8 MiB"));
    assert!(limits.contains("linked or non-regular"));
    assert_eq!(
        fs::metadata(&fixture.cache).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(snapshot.dir()).unwrap().permissions().mode() & 0o777,
        0o500
    );
    assert_eq!(
        fs::metadata(snapshot.dir().join("file.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o400
    );
}

#[test]
#[cfg(unix)]
fn review_snapshot_accepts_a_symlinked_cache_parent() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    init_repo_with_commit(&fixture.repo, &["base"]);
    let root = fixture.repo.parent().unwrap();
    let actual = root.join("actual-cache-parent");
    let alias = root.join("cache-parent-alias");
    fs::create_dir(&actual).unwrap();
    symlink(&actual, &alias).unwrap();

    let request = SnapshotRequest::local(
        fixture.repo.clone(),
        VcsType::Git,
        LocalTarget::Worktree,
        None,
        false,
        Vec::new(),
    );
    let snapshot = request.materialize_in(alias.join("snapshots")).unwrap();
    assert!(snapshot.dir().starts_with(actual.join("snapshots")));
    assert_eq!(file(&snapshot, "file.txt"), "one\ntwo\n");
    drop(snapshot);
    cache::make_removable(&actual).unwrap();
}

#[test]
#[cfg(unix)]
fn review_snapshot_refuses_linked_cache_and_prunes_only_inactive_snapshots() {
    use std::os::unix::fs::symlink;
    use std::time::{Duration, SystemTime};
    let fixture = Fixture::new();
    init_repo_with_commit(&fixture.repo, &["base"]);
    symlink(&fixture.repo, &fixture.cache).unwrap();
    let request = SnapshotRequest::local(
        fixture.repo.clone(),
        VcsType::Git,
        LocalTarget::Worktree,
        None,
        false,
        Vec::new(),
    );
    assert!(request.materialize_in(fixture.cache.clone()).is_err());
    fs::remove_file(&fixture.cache).unwrap();
    let snapshot = request.materialize_in(fixture.cache.clone()).unwrap();
    let dir = snapshot.dir().parent().unwrap().to_owned();
    File::open(&dir)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60))
        .unwrap();
    fixture.local(LocalTarget::Index);
    assert!(dir.exists(), "active snapshot was pruned");
    drop(snapshot);
    fixture.local(LocalTarget::Index);
    assert!(!dir.exists(), "expired unused snapshot was not pruned");
}

#[test]
fn review_snapshot_serialization_preserves_source_side() {
    let request = SnapshotRequest::local(
        PathBuf::from("repo"),
        VcsType::Git,
        LocalTarget::Index,
        None,
        false,
        Vec::new(),
    );
    let bytes = serde_json::to_vec(&request).unwrap();
    assert_eq!(
        serde_json::from_slice::<SnapshotRequest>(&bytes).unwrap(),
        request
    );
}

#[test]
fn review_snapshot_hg_archive_and_worktree_when_hg_available() {
    use std::process::{Command, Stdio};
    if !Command::new("hg")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }
    let fixture = Fixture::new();
    let hg = |args: &[&str]| {
        let output = Command::new("hg")
            .current_dir(&fixture.repo)
            .env("HGRCPATH", "")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "hg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    hg(&["init"]);
    fixture.file("source.txt", "pinned hg content");
    hg(&["add", "source.txt"]);
    hg(&["commit", "-u", "tester", "-m", "base"]);
    let sha = hg(&["log", "-r", ".", "-T", "{node}"]);
    fixture.file("source.txt", "hg working content");
    let side = |target| {
        SnapshotRequest::local(
            fixture.repo.clone(),
            VcsType::Mercurial,
            target,
            None,
            false,
            Vec::new(),
        )
        .materialize_in(fixture.cache.clone())
        .unwrap()
    };
    assert_eq!(
        file(&side(LocalTarget::Revision(sha)), "source.txt"),
        "pinned hg content"
    );
    assert_eq!(
        file(&side(LocalTarget::Worktree), "source.txt"),
        "hg working content"
    );
}

#[test]
fn review_snapshot_jj_requires_a_colocated_git_checkout() {
    use std::process::{Command, Stdio};
    if !Command::new("jj")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
    {
        return;
    }
    let fixture = Fixture::new();
    let sha = init_repo_with_commit(&fixture.repo, &["base"]);
    let result = Command::new("jj")
        .current_dir(&fixture.repo)
        .env("JJ_CONFIG", "")
        .args(["git", "init", "--colocate"])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "jj init failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let snapshot = SnapshotRequest::local(
        fixture.repo.clone(),
        VcsType::Jujutsu,
        LocalTarget::Revision(sha),
        None,
        false,
        Vec::new(),
    )
    .materialize_in(fixture.cache.clone())
    .unwrap();
    assert_eq!(file(&snapshot, "file.txt"), "one\ntwo\n");
    assert!(!snapshot.dir().join(".jj").exists());
    fs::rename(
        fixture.repo.join(".git"),
        fixture.repo.join("not-a-checkout"),
    )
    .unwrap();
    let unavailable = SnapshotRequest::local(
        fixture.repo.clone(),
        VcsType::Jujutsu,
        LocalTarget::Worktree,
        None,
        false,
        Vec::new(),
    )
    .materialize_in(fixture.cache.clone())
    .unwrap();
    assert!(
        unavailable
            .limits()
            .unwrap()
            .contains("No independently readable tree")
    );
}

#[test]
fn review_snapshot_github_archive_strips_prefix_and_ignores_archive_links() {
    let fixture = Fixture::new();
    let cache = Cache::open(fixture.cache.clone()).unwrap();
    let stage = cache.stage().unwrap();
    let mut build = Build::new(&stage.tree, false);
    let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gzip);
    let payload = b"unchanged source available without a matching checkout\n";
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(payload.len() as u64);
    tar.append_data(
        &mut header,
        "repository-commit/src/caller.txt",
        &payload[..],
    )
    .unwrap();
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    tar.append_link(
        &mut header,
        "repository-commit/linked",
        "/private/unreviewed.txt",
    )
    .unwrap();
    let compressed = tar.into_inner().unwrap().finish().unwrap();
    vcs::extract_github_archive(compressed.as_slice(), &mut build).unwrap();
    let (_, limits) = build.finish();
    let snapshot = cache
        .publish(&identity(format_args!("fake-archive")), stage, &limits)
        .unwrap();
    assert_eq!(file(&snapshot, "src/caller.txt").as_bytes(), payload);
    assert!(!snapshot.dir().join("repository-commit").exists());
    assert!(fs::symlink_metadata(snapshot.dir().join("linked")).is_err());
    assert!(snapshot.limits().unwrap().contains("linked or non-regular"));
    assert!(snapshot.limits().unwrap().contains("export-ignore"));
}

#[test]
fn review_snapshot_bounds_total_files_and_bytes() {
    let fixture = Fixture::new();
    let cache = Cache::open(fixture.cache.clone()).unwrap();
    let stage = cache.stage().unwrap();
    let mut build = Build::new(&stage.tree, false);
    build.bytes = MAX_TREE_BYTES - 4;
    build
        .write(Path::new("too-large"), 5, b"hello".as_slice())
        .unwrap();
    build.files = MAX_FILES;
    build
        .write(Path::new("too-many"), 1, b"a".as_slice())
        .unwrap();
    assert_eq!(fs::read_dir(&stage.tree).unwrap().count(), 0);
    let (_, limits) = build.finish();
    assert!(limits.contains("20,000 files"));
    assert!(limits.contains("64 MiB snapshot byte budget"));
}
