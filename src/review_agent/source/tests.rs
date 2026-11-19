use super::*;
use crate::forge::local_git::test_support::{git, init_repo, init_repo_with_commit};
use std::fs;
use std::time::{Duration, SystemTime};

struct Fixture {
    _scratch: tempfile::TempDir,
    repo: PathBuf,
    cache: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let scratch = tempfile::tempdir().unwrap();
        // macOS's temp dir may be returned via a system symlink.
        let root = scratch.path().canonicalize().unwrap();
        let repo = root.join("repo");
        fs::create_dir(&repo).unwrap();
        Self {
            _scratch: scratch,
            repo,
            cache: root.join("cache"),
        }
    }

    fn local(&self, target: LocalTarget) -> Workspace {
        self.request(VcsType::Git, target)
            .workspace_in(&self.cache)
            .unwrap()
    }

    fn request(&self, vcs: VcsType, target: LocalTarget) -> SourceRequest {
        SourceRequest::local(self.repo.clone(), vcs, target, false)
    }

    fn file(&self, path: &str, content: &str) {
        let path = self.repo.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn commit(&self, message: &str) -> String {
        git(&self.repo, &["add", "."]);
        git(&self.repo, &["commit", "--quiet", "-m", message]);
        git(&self.repo, &["rev-parse", "HEAD"]).trim().to_owned()
    }
}

fn file(workspace: &Workspace, name: &str) -> String {
    fs::read_to_string(workspace.dir().join(name)).unwrap()
}

#[test]
fn worktree_runs_in_the_repository_itself() {
    let fixture = Fixture::new();
    init_repo_with_commit(&fixture.repo, &["base"]);
    fixture.file("file.txt", "uncommitted edit");
    let workspace = fixture.local(LocalTarget::Worktree);
    assert_eq!(workspace.dir(), fixture.repo);
    assert_eq!(file(&workspace, "file.txt"), "uncommitted edit");
    assert_eq!(workspace.note(), None);
    assert!(!fixture.cache.exists(), "no cache is needed in place");
}

#[test]
fn alternate_index_is_called_out_on_both_sides() {
    let fixture = Fixture::new();
    init_repo_with_commit(&fixture.repo, &["base"]);
    let alternate = |target| {
        SourceRequest::local(fixture.repo.clone(), VcsType::Git, target, true)
            .workspace_in(&fixture.cache)
            .unwrap()
    };
    let worktree = alternate(LocalTarget::Worktree);
    assert_eq!(worktree.dir(), fixture.repo);
    assert!(worktree.note().unwrap().contains("alternate Git index"));
    let index = alternate(LocalTarget::Index);
    assert_eq!(index.dir(), fixture.repo);
    assert!(index.note().unwrap().contains("cannot be read exactly"));
}

#[test]
fn staged_index_runs_in_place_and_points_at_git_show() {
    let fixture = Fixture::new();
    init_repo_with_commit(&fixture.repo, &["base"]);
    fixture.file("file.txt", "staged");
    git(&fixture.repo, &["add", "file.txt"]);
    fixture.file("file.txt", "unstaged");
    let workspace = fixture.local(LocalTarget::Index);
    assert_eq!(workspace.dir(), fixture.repo);
    let note = workspace.note().unwrap();
    assert!(note.contains("staged Git index") && note.contains("git show :<path>"));
    let request = fixture.request(VcsType::Git, LocalTarget::Index);
    assert_eq!(
        request.local_text()(Path::new("file.txt")).as_deref(),
        Some("staged")
    );
}

#[test]
fn revision_is_a_private_checkout_with_history_even_when_the_worktree_differs() {
    let fixture = Fixture::new();
    let first = init_repo_with_commit(&fixture.repo, &["first"]);
    fixture.file("src/state.txt", "at pinned commit");
    let pinned = fixture.commit("pin this version");
    fixture.file("src/state.txt", "committed later");
    fixture.commit("later");
    fixture.file("src/state.txt", "on the user's worktree");

    let workspace = fixture.local(LocalTarget::Revision(pinned.clone()));
    let dir = workspace.dir().to_owned();
    assert_ne!(dir, fixture.repo);
    assert!(
        dir.starts_with(
            fixture
                .cache
                .canonicalize()
                .unwrap()
                .join("review-checkouts")
        )
    );
    assert_eq!(dir.file_name().unwrap().to_str().unwrap(), pinned);
    assert_eq!(file(&workspace, "src/state.txt"), "at pinned commit");
    assert_eq!(workspace.note(), None);
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]).trim(), pinned);
    assert!(git(&dir, &["status", "--porcelain"]).is_empty());
    // Full history, including commits the revision descends from, and refs
    // from the user's repository.
    let log = git(&dir, &["log", "--format=%s"]);
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        ["pin this version", "first"]
    );
    assert!(git(&dir, &["cat-file", "-t", &first]).starts_with("commit"));
    assert!(!git(&dir, &["branch", "-a"]).is_empty());
    // The user's repository is untouched.
    assert_eq!(
        file_in(&fixture.repo, "src/state.txt"),
        "on the user's worktree"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let repo_dir = dir.parent().unwrap();
        assert_eq!(
            fs::metadata(repo_dir).unwrap().permissions().mode() & 0o077,
            0
        );
    }
}

fn file_in(dir: &Path, name: &str) -> String {
    fs::read_to_string(dir.join(name)).unwrap()
}

#[test]
fn parent_of_and_recorded_head_resolve_to_their_own_checkouts() {
    let fixture = Fixture::new();
    let base = init_repo_with_commit(&fixture.repo, &["base"]);
    fixture.file("file.txt", "child");
    let child = fixture.commit("child");
    let parent = fixture.local(LocalTarget::ParentOf(child.clone()));
    assert_eq!(file(&parent, "file.txt"), "one\ntwo\n");
    assert_eq!(parent.dir().file_name().unwrap().to_str().unwrap(), base);
    let head = fixture.local(LocalTarget::RecordedHead(child[..12].to_owned()));
    assert_eq!(file(&head, "file.txt"), "child");
}

#[test]
fn reusing_a_revision_returns_the_same_checkout_and_rebuilds_incomplete_ones() {
    let fixture = Fixture::new();
    let sha = init_repo_with_commit(&fixture.repo, &["base"]);
    let first = fixture.local(LocalTarget::Revision(sha.clone()));
    let dir = first.dir().to_owned();
    fs::write(dir.join("marker"), "created by the first run").unwrap();
    let second = fixture.local(LocalTarget::Revision(sha.clone()));
    assert_eq!(second.dir(), dir);
    assert!(
        dir.join("marker").exists(),
        "a complete checkout is reused as is"
    );
    drop((first, second));

    // Without its lease a checkout never finished; it must not be trusted.
    let lease = dir.with_file_name(format!("{sha}.lease"));
    fs::remove_file(&lease).unwrap();
    let third = fixture.local(LocalTarget::Revision(sha));
    assert_eq!(third.dir(), dir);
    assert!(!dir.join("marker").exists());
    assert!(lease.exists());
    assert_eq!(file(&third, "file.txt"), "one\ntwo\n");
}

#[test]
fn concurrent_requests_for_one_revision_share_a_checkout() {
    let fixture = Fixture::new();
    let sha = init_repo_with_commit(&fixture.repo, &["base"]);
    let dirs: Vec<_> = std::thread::scope(|scope| {
        (0..4)
            .map(|_| {
                scope.spawn(|| {
                    let workspace = fixture.local(LocalTarget::Revision(sha.clone()));
                    assert_eq!(file(&workspace, "file.txt"), "one\ntwo\n");
                    workspace.dir().to_owned()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    assert!(dirs.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn old_checkouts_are_pruned_only_when_no_workspace_holds_them() {
    let fixture = Fixture::new();
    let old = init_repo_with_commit(&fixture.repo, &["old"]);
    fixture.file("file.txt", "newer");
    let newer = fixture.commit("newer");
    let held = fixture.local(LocalTarget::Revision(old.clone()));
    let old_dir = held.dir().to_owned();
    let backdate = |dir: &Path| {
        let lease = dir.with_file_name(format!(
            "{}.lease",
            dir.file_name().unwrap().to_str().unwrap()
        ));
        let file = fs::OpenOptions::new().write(true).open(lease).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60))
            .unwrap();
    };
    backdate(&old_dir);

    drop(fixture.local(LocalTarget::Revision(newer.clone())));
    assert!(old_dir.exists(), "a held checkout is never pruned");

    drop(held);
    // A flock can briefly outlive its handle when another thread forks before
    // exec'ing a child (tests run in parallel), so a prune may need a retry.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let recent = loop {
        let recent = fixture.local(LocalTarget::Revision(newer.clone()));
        if !old_dir.exists() || std::time::Instant::now() > deadline {
            break recent;
        }
        drop(recent);
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(!old_dir.exists(), "an unused week-old checkout is pruned");
    assert!(recent.dir().exists());
}

#[test]
fn unreadable_revisions_fall_back_to_the_working_copy_with_a_note() {
    let fixture = Fixture::new();
    init_repo_with_commit(&fixture.repo, &["base"]);
    let missing = fixture.local(LocalTarget::Revision(
        "0123456789abcdef0123456789abcdef01234567".into(),
    ));
    assert_eq!(missing.dir(), fixture.repo);
    assert!(
        missing
            .note()
            .unwrap()
            .contains("exact reviewed revision is unavailable")
    );
    let unavailable = fixture.local(LocalTarget::Unavailable);
    assert_eq!(unavailable.dir(), fixture.repo);
    assert!(unavailable.note().is_some());

    let gone = SourceRequest::local(
        fixture.repo.join("missing"),
        VcsType::Git,
        LocalTarget::Worktree,
        false,
    )
    .workspace_in(&fixture.cache)
    .unwrap();
    assert_eq!(fs::read_dir(gone.dir()).unwrap().count(), 0);
    assert!(gone.note().unwrap().contains("No readable checkout"));
}

fn pr(fixture: &Fixture, repository: &ForgeRepository, sha: Option<String>) -> SourceRequest {
    SourceRequest::pull_request(Some(fixture.repo.clone()), repository.clone(), sha)
}

fn gitlab() -> ForgeRepository {
    ForgeRepository {
        kind: ForgeKind::GitLab,
        host: "gitlab.snapshot.invalid".into(),
        owner: "review".into(),
        name: "tree".into(),
    }
}

#[test]
fn pull_request_uses_matching_clone_objects_not_the_users_checkout() {
    let fixture = Fixture::new();
    let base = init_repo_with_commit(&fixture.repo, &["base"]);
    fixture.file("file.txt", "at pr head");
    let head = fixture.commit("reviewed head");
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
    let repository = gitlab();
    let side = |sha| {
        pr(&fixture, &repository, sha)
            .workspace_in(&fixture.cache)
            .unwrap()
    };

    let new = side(Some(head.clone()));
    assert_ne!(new.dir(), fixture.repo);
    assert_eq!(file(&new, "file.txt"), "at pr head");
    assert_eq!(git(new.dir(), &["log", "--format=%s"]).lines().count(), 2);
    assert_eq!(file(&side(Some(base)), "file.txt"), "one\ntwo\n");

    let unconfirmed = side(None);
    assert_eq!(fs::read_dir(unconfirmed.dir()).unwrap().count(), 0);
    assert_eq!(
        unconfirmed.note(),
        Some("No pinned source revision is available on this side; use the supplied diff.")
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
    let wrong = side(Some(head));
    assert_eq!(fs::read_dir(wrong.dir()).unwrap().count(), 0);
    assert!(
        wrong
            .note()
            .unwrap()
            .contains("No matching local Git checkout")
    );
}

#[test]
fn pull_request_fetches_a_missing_commit_from_the_matching_remote() {
    let fixture = Fixture::new();
    let remote = fixture.repo.with_file_name("remote");
    fs::create_dir(&remote).unwrap();
    init_repo_with_commit(&remote, &["base"]);
    git(
        fixture.repo.parent().unwrap(),
        &["clone", "--quiet", remote.to_str().unwrap(), "repo-clone"],
    );
    let local = fixture.repo.with_file_name("repo-clone");
    git(&local, &["config", "user.email", "test@example.com"]);
    // The PR's head lands on the remote after the clone was made.
    fs::write(remote.join("file.txt"), "pushed after the clone").unwrap();
    git(&remote, &["commit", "--quiet", "-am", "head"]);
    let head = git(&remote, &["rev-parse", "HEAD"]).trim().to_owned();
    git(
        &local,
        &[
            "remote",
            "set-url",
            "origin",
            "git@gitlab.snapshot.invalid:review/tree.git",
        ],
    );
    git(
        &local,
        &[
            "config",
            &format!("url.{}.insteadOf", remote.display()),
            "git@gitlab.snapshot.invalid:review/tree.git",
        ],
    );
    assert!(!vcs::has_commit(&local, &head));

    let request = SourceRequest::pull_request(Some(local.clone()), gitlab(), Some(head.clone()));
    let workspace = request.workspace_in(&fixture.cache).unwrap();
    assert_eq!(file(&workspace, "file.txt"), "pushed after the clone");
    assert_eq!(git(workspace.dir(), &["rev-parse", "HEAD"]).trim(), head);
    assert!(vcs::has_commit(&local, &head));
}

#[test]
fn unfetchable_pull_request_commit_on_another_forge_is_diff_only() {
    let fixture = Fixture::new();
    init_repo_with_commit(&fixture.repo, &["base"]);
    git(
        &fixture.repo,
        &[
            "remote",
            "add",
            "origin",
            "git@gitlab.snapshot.invalid:review/tree.git",
        ],
    );
    let absent = "0123456789abcdef0123456789abcdef01234567".to_owned();
    // Fetching fails (the host is unreachable); GitLab has no mirror fallback.
    let workspace = pr(&fixture, &gitlab(), Some(absent))
        .workspace_in(&fixture.cache)
        .unwrap();
    assert_eq!(fs::read_dir(workspace.dir()).unwrap().count(), 0);
    assert!(workspace.note().unwrap().contains("use the supplied diff"));
}

#[test]
fn source_request_serialization_preserves_source_side() {
    let request = SourceRequest::local(
        PathBuf::from("repo"),
        VcsType::Git,
        LocalTarget::Index,
        false,
    );
    let bytes = serde_json::to_vec(&request).unwrap();
    assert_eq!(
        serde_json::from_slice::<SourceRequest>(&bytes).unwrap(),
        request
    );
}

fn tool_available(name: &str) -> bool {
    use std::process::{Command, Stdio};
    Command::new(name)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn mercurial_revision_runs_in_place_and_names_the_revision() {
    use std::process::Command;
    if !tool_available("hg") {
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

    let request = |target| fixture.request(VcsType::Mercurial, target);
    let revision = request(LocalTarget::Revision(sha.clone()))
        .workspace_in(&fixture.cache)
        .unwrap();
    assert_eq!(revision.dir(), fixture.repo);
    let note = revision.note().unwrap();
    assert!(note.contains(&sha) && note.contains(&format!("hg cat -r {sha}")));
    assert_eq!(
        request(LocalTarget::Revision(sha)).local_text()(Path::new("source.txt")).as_deref(),
        Some("pinned hg content")
    );
    let worktree = request(LocalTarget::Worktree)
        .workspace_in(&fixture.cache)
        .unwrap();
    assert_eq!(file(&worktree, "source.txt"), "hg working content");
    assert_eq!(worktree.note(), None);
}

#[test]
fn jj_revisions_require_a_colocated_git_checkout() {
    use std::process::Command;
    if !tool_available("jj") {
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
    let revision = fixture
        .request(VcsType::Jujutsu, LocalTarget::Revision(sha.clone()))
        .workspace_in(&fixture.cache)
        .unwrap();
    assert_ne!(revision.dir(), fixture.repo);
    assert_eq!(file(&revision, "file.txt"), "one\ntwo\n");
    assert!(!revision.dir().join(".jj").exists());

    fs::rename(
        fixture.repo.join(".git"),
        fixture.repo.join("not-a-checkout"),
    )
    .unwrap();
    let unavailable = fixture
        .request(VcsType::Jujutsu, LocalTarget::Revision(sha))
        .workspace_in(&fixture.cache)
        .unwrap();
    assert_eq!(unavailable.dir(), fixture.repo);
    assert!(unavailable.note().unwrap().contains("unavailable"));
}

#[test]
fn local_text_reads_the_worktree_and_pinned_revisions() {
    let fixture = Fixture::new();
    let sha = init_repo_with_commit(&fixture.repo, &["base"]);
    fixture.file("file.txt", "edited");
    let read =
        |target, name: &str| fixture.request(VcsType::Git, target).local_text()(Path::new(name));
    assert_eq!(
        read(LocalTarget::Worktree, "file.txt").as_deref(),
        Some("edited")
    );
    assert_eq!(
        read(LocalTarget::Revision(sha), "file.txt").as_deref(),
        Some("one\ntwo\n")
    );
    assert_eq!(read(LocalTarget::Worktree, ".git/config"), None);
    let pr = pr(&fixture, &gitlab(), None);
    assert_eq!(pr.local_text()(Path::new("file.txt")), None);
}

#[test]
fn a_repository_without_commits_still_resolves_the_worktree() {
    let fixture = Fixture::new();
    init_repo(&fixture.repo);
    assert_eq!(fixture.local(LocalTarget::Worktree).dir(), fixture.repo);
}
