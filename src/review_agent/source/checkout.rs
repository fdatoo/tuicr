//! Private, per-user cache of real repository checkouts for the review agent.
//!
//! Layout under the tuicr cache directory:
//!
//! ```text
//! review-checkouts/<repo key>/.lock        per-repo lock (create, reuse, prune)
//! review-checkouts/<repo key>/<sha>        the checkout the provider runs in
//! review-checkouts/<repo key>/<sha>.lease  completion marker and use lease
//! review-checkouts/<repo key>/.stage-*     in-progress clone, renamed into place
//! review-mirrors/<host>/<owner>/<name>     blobless GitHub clone backing worktrees
//! review-scratch/empty-*                   empty directories for diff-only runs
//! ```
//!
//! A checkout is only ever complete when its lease file exists. Every
//! `Workspace` keeps a shared lock on its lease, so pruning (exclusive lock)
//! never removes a checkout a provider is still reading.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use sha2::{Digest, Sha256};

use crate::forge::traits::ForgeRepository;

use super::{is_full_object_id, vcs};

const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const STALE_STAGE: Duration = Duration::from_secs(60 * 60);
const LOCAL_TIMEOUT: Duration = Duration::from_secs(120);
const NETWORK_TIMEOUT: Duration = Duration::from_secs(300);
const STDERR_KEPT: usize = 8 * 1024;
const LEASE_SUFFIX: &str = ".lease";
const MIRROR_MARKER: &str = ".mirror";

pub(super) struct Checkout {
    pub(super) dir: PathBuf,
    pub(super) lease: File,
}

pub(super) struct Cache {
    checkouts: PathBuf,
    mirrors: PathBuf,
    scratch: PathBuf,
}

impl Cache {
    /// `base` is the tuicr cache directory. Aliases in its path (such as
    /// macOS's `/var`) are resolved once; the directories below it are
    /// created private and must stay owned by the current user.
    pub(super) fn open(base: &Path) -> io::Result<Self> {
        private_dir_builder().recursive(true).create(base)?;
        let base = base.canonicalize()?;
        let cache = Self {
            checkouts: base.join("review-checkouts"),
            mirrors: base.join("review-mirrors"),
            scratch: base.join("review-scratch"),
        };
        for dir in [&cache.checkouts, &cache.mirrors, &cache.scratch] {
            create_private(dir)?;
        }
        Ok(cache)
    }

    /// An empty directory that lives as long as the returned guard.
    pub(super) fn empty_dir(&self) -> io::Result<tempfile::TempDir> {
        tempfile::Builder::new()
            .prefix("empty-")
            .tempdir_in(&self.scratch)
    }

    /// A checkout of `sha` made from a shared clone of the user's own
    /// repository: full history and refs, objects borrowed, no working tree
    /// copy of the user's files.
    pub(super) fn local(&self, root: &Path, sha: &str) -> Result<Checkout, String> {
        let common = vcs::git_common_dir(root)
            .ok_or_else(|| "Could not locate the repository's Git directory".to_owned())?;
        let key = key(&format!("git:{}", common.display()));
        self.checkout(&key, sha, None, |repo_dir, dest| {
            let stage = stage_dir(repo_dir)?;
            let mut clone = vcs::git_network(repo_dir);
            clone
                .args(self.hooks_config())
                .args(["clone", "--shared", "--no-checkout", "--quiet", "--"])
                .arg(root)
                .arg(stage.path());
            run(clone, LOCAL_TIMEOUT, "git clone")?;
            let mut checkout = vcs::git_network(stage.path());
            checkout
                .env("GIT_LFS_SKIP_SMUDGE", "1")
                .args(self.hooks_config())
                .args(["checkout", "--detach", "--quiet", sha]);
            run(checkout, LOCAL_TIMEOUT, "git checkout")?;
            fs::rename(stage.path(), dest).map_err(|error| format!("Could not publish: {error}"))
        })
    }

    /// A checkout of `sha` from a private blobless mirror of a GitHub
    /// repository, for pull requests without a matching local clone.
    pub(super) fn github(
        &self,
        repository: &ForgeRepository,
        sha: &str,
    ) -> Result<Checkout, String> {
        let component = |value: &str| {
            !matches!(value, "" | "." | "..")
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        };
        if !component(&repository.host)
            || !component(&repository.owner)
            || !component(&repository.name)
        {
            return Err(format!(
                "Unsupported repository name {}/{}/{}",
                repository.host, repository.owner, repository.name
            ));
        }
        let owner_dir = self.mirrors.join(&repository.host).join(&repository.owner);
        let mirror = owner_dir.join(&repository.name);
        let key = key(&format!(
            "mirror:{}/{}/{}",
            repository.host, repository.owner, repository.name
        ));
        self.checkout(&key, sha, Some(&mirror), |_, dest| {
            if !mirror.join(".git").exists() {
                self.clone_mirror(repository, &owner_dir, &mirror)?;
            }
            if !vcs::has_commit(&mirror, sha) {
                fetch(&mirror, "origin", sha)?;
            }
            let mut prune = vcs::git(&mirror);
            prune.args(["worktree", "prune"]);
            run(prune, LOCAL_TIMEOUT, "git worktree prune")?;
            // Blobs are fetched lazily here, so lazy fetching must stay on.
            let mut add = vcs::git_lazy(&mirror);
            add.env("GIT_LFS_SKIP_SMUDGE", "1")
                .args(self.hooks_config())
                .args(["worktree", "add", "--detach", "--quiet"])
                .arg(dest)
                .arg(sha);
            run(add, NETWORK_TIMEOUT, "git worktree add")
        })
    }

    fn clone_mirror(
        &self,
        repository: &ForgeRepository,
        owner_dir: &Path,
        mirror: &Path,
    ) -> Result<(), String> {
        private_dir_builder()
            .recursive(true)
            .create(owner_dir)
            .map_err(|error| format!("Could not create the mirror directory: {error}"))?;
        let stage = stage_dir(owner_dir)?;
        let mut clone = Command::new("gh");
        clone
            .current_dir(owner_dir)
            .env("GH_PROMPT_DISABLED", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(["repo", "clone"])
            .arg(format!("{}/{}", repository.owner, repository.name))
            .arg(stage.path())
            .args(["--", "--filter=blob:none", "--no-checkout", "--quiet"]);
        if repository.host != "github.com" {
            clone.env("GH_HOST", &repository.host);
        }
        run(clone, NETWORK_TIMEOUT, "gh repo clone")?;
        fs::rename(stage.path(), mirror)
            .map_err(|error| format!("Could not publish the mirror: {error}"))
    }

    fn hooks_config(&self) -> [String; 2] {
        // Never run hooks from the user's templates or the reviewed repository.
        [
            "-c".into(),
            format!(
                "core.hooksPath={}",
                self.checkouts.join(".no-hooks").display()
            ),
        ]
    }

    /// Create `<repo>/<sha>` with `create`, or reuse the complete one that is
    /// already there, and return it with a shared lease held.
    fn checkout(
        &self,
        key: &str,
        sha: &str,
        mirror: Option<&Path>,
        create: impl FnOnce(&Path, &Path) -> Result<(), String>,
    ) -> Result<Checkout, String> {
        let io = |what: &str| {
            let what = what.to_owned();
            move |error: io::Error| format!("{what}: {error}")
        };
        let repo_dir = self.checkouts.join(key);
        create_private(&repo_dir).map_err(io("Could not create the checkout cache"))?;
        let guard = lock_file(&repo_dir.join(".lock"), true)
            .map_err(io("Could not lock the checkout cache"))?;
        if let Some(mirror) = mirror {
            fs::write(
                repo_dir.join(MIRROR_MARKER),
                mirror.as_os_str().as_encoded_bytes(),
            )
            .map_err(io("Could not record the mirror"))?;
        }
        let dest = repo_dir.join(sha);
        let lease_path = repo_dir.join(format!("{sha}{LEASE_SUFFIX}"));
        let complete = lease_path.exists() && vcs::head_commit(&dest).as_deref() == Some(sha);
        if !complete {
            // Either never finished or damaged. Only remove it when no
            // provider still holds a lease.
            if lease_path.exists() {
                let lease = open_lease(&lease_path, false).map_err(io("Could not open lease"))?;
                lease
                    .try_lock()
                    .map_err(|_| "The cached checkout is damaged and still in use".to_owned())?;
            }
            remove_checkout(&repo_dir, sha, mirror)
                .map_err(io("Could not remove the incomplete checkout"))?;
            create(&repo_dir, &dest)?;
            drop(open_lease(&lease_path, true).map_err(io("Could not create the checkout lease"))?);
        }
        let lease = open_lease(&lease_path, false).map_err(io("Could not open the lease"))?;
        lease
            .lock_shared()
            .map_err(io("Could not lock the checkout"))?;
        lease
            .set_modified(SystemTime::now())
            .map_err(io("Could not refresh the checkout"))?;
        drop(guard);
        self.prune();
        Ok(Checkout { dir: dest, lease })
    }

    /// Remove checkouts unused for a week, skipping any repository or
    /// checkout somebody else holds. Best effort.
    fn prune(&self) {
        let Ok(repos) = fs::read_dir(&self.checkouts) else {
            return;
        };
        let now = SystemTime::now();
        for repo in repos.flatten() {
            let repo_dir = repo.path();
            if !repo.file_type().is_ok_and(|ty| ty.is_dir()) {
                continue;
            }
            let Ok(guard) = lock_file(&repo_dir.join(".lock"), false) else {
                continue;
            };
            let mirror = fs::read(repo_dir.join(MIRROR_MARKER))
                .ok()
                .map(|bytes| PathBuf::from(os_string(bytes)));
            let age = |path: &Path| {
                fs::metadata(path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|mtime| now.duration_since(mtime).ok())
            };
            let Ok(entries) = fs::read_dir(&repo_dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(".stage-") && age(&entry.path()) > Some(STALE_STAGE) {
                    let _ = fs::remove_dir_all(entry.path());
                } else if let Some(sha) = name.strip_suffix(LEASE_SUFFIX)
                    && is_full_object_id(sha)
                    && age(&entry.path()) > Some(RETENTION)
                    && let Ok(lease) = open_lease(&entry.path(), false)
                    && lease.try_lock().is_ok()
                {
                    let _ = remove_checkout(&repo_dir, sha, mirror.as_deref());
                    let _ = fs::remove_file(entry.path());
                }
            }
            drop(guard);
        }
    }
}

fn os_string(bytes: Vec<u8>) -> std::ffi::OsString {
    #[cfg(unix)]
    {
        std::os::unix::ffi::OsStringExt::from_vec(bytes)
    }
    #[cfg(not(unix))]
    {
        String::from_utf8_lossy(&bytes).into_owned().into()
    }
}

/// Fetch one commit into the user's own repository from the remote whose URL
/// identified the pull request's repository.
pub(super) fn fetch(root: &Path, remote: &str, sha: &str) -> Result<(), String> {
    let mut fetch = vcs::git_network(root);
    fetch.args(["fetch", "--no-tags", "--quiet", "--", remote, sha]);
    run(fetch, NETWORK_TIMEOUT, "git fetch")
}

fn remove_checkout(repo_dir: &Path, sha: &str, mirror: Option<&Path>) -> io::Result<()> {
    let dest = repo_dir.join(sha);
    match fs::remove_dir_all(&dest) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let _ = fs::remove_file(repo_dir.join(format!("{sha}{LEASE_SUFFIX}")));
    if let Some(mirror) = mirror {
        let mut prune = vcs::git(mirror);
        prune.args(["worktree", "prune"]);
        let _ = run(prune, LOCAL_TIMEOUT, "git worktree prune");
    }
    Ok(())
}

fn key(identity: &str) -> String {
    let digest = Sha256::digest(identity.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn stage_dir(parent: &Path) -> Result<tempfile::TempDir, String> {
    tempfile::Builder::new()
        .prefix(".stage-")
        .tempdir_in(parent)
        .map_err(|error| format!("Could not create a staging directory: {error}"))
}

/// Run to completion within `timeout`, killing the whole process group on
/// expiry, and report the tail of stderr on failure.
fn run(mut command: Command, timeout: Duration, what: &str) -> Result<(), String> {
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{what} could not start: {error}"))?;
    let mut stderr = child.stderr.take();
    let reader = thread::spawn(move || {
        let mut kept = Vec::new();
        let mut chunk = [0u8; 4096];
        while let Some(pipe) = stderr.as_mut() {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) if kept.len() < STDERR_KEPT => kept.extend_from_slice(&chunk[..n]),
                Ok(_) => {}
            }
        }
        kept
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                #[cfg(unix)]
                // SAFETY: the child leads its own process group, has not
                // been reaped, and its PID is that group's ID.
                unsafe {
                    libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stderr = reader.join().unwrap_or_default();
    match status {
        Some(status) if status.success() => Ok(()),
        Some(_) => {
            let text = String::from_utf8_lossy(&stderr);
            let line = text.lines().rev().find(|line| !line.trim().is_empty());
            Err(match line {
                Some(line) => format!("{what} failed: {}", line.trim()),
                None => format!("{what} failed"),
            })
        }
        None => Err(format!(
            "{what} did not finish within {}s",
            timeout.as_secs()
        )),
    }
}

fn private_dir_builder() -> fs::DirBuilder {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
}

fn create_private(path: &Path) -> io::Result<()> {
    match private_dir_builder().create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || !owner_only(&metadata) {
        return Err(io::Error::other(format!(
            "{} is not a private directory owned by the current user",
            path.display()
        )));
    }
    Ok(())
}

fn owner_only(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn open_lease(path: &Path, create: bool) -> io::Result<File> {
    // Read/write, so exclusive locks work even where read-only handles can't
    // take them.
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// The per-repository lock. `wait` blocks until it is free; otherwise a busy
/// lock is an error.
fn lock_file(path: &Path, wait: bool) -> io::Result<File> {
    let file = open_lease(path, true)?;
    if !file.metadata()?.is_file() || !owner_only(&file.metadata()?) {
        return Err(io::Error::other("unsafe checkout cache lock"));
    }
    if wait {
        file.lock()?;
    } else {
        file.try_lock().map_err(io::Error::other)?;
    }
    Ok(file)
}
