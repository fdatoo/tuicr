mod cache;
mod paths;
mod vcs;

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::forge::traits::{ForgeKind, ForgeRepository};
use crate::vcs::traits::VcsType;

use cache::Cache;

pub(super) const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TREE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FILES: usize = 20_000;
const MAX_CATALOG_BYTES: usize = 16 * 1024 * 1024;
const UNAVAILABLE: &str =
    "No independently readable tree is available on this side; use the supplied diff.";
const UNSAFE_PATH: &str = "Private, unsupported or unsafe paths were omitted.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotRequest {
    plan: SourcePlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum SourcePlan {
    Local {
        root: PathBuf,
        vcs: VcsType,
        target: LocalTarget,
        single_file: Option<PathBuf>,
        alternate_index: bool,
        anchors: Vec<PathBuf>,
    },
    PullRequest {
        checkout: Option<PathBuf>,
        repository: ForgeRepository,
        sha: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum LocalTarget {
    Worktree,
    Index,
    Revision(String),
    RecordedHead(String),
    ParentOf(String),
    JjRecordedParent(String),
    Unavailable,
}

pub struct Snapshot {
    dir: PathBuf,
    limits: Option<String>,
    _lease: File,
}

impl Snapshot {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn limits(&self) -> Option<&str> {
        self.limits.as_deref()
    }
}

impl SnapshotRequest {
    pub(crate) fn is_local(&self) -> bool {
        matches!(&self.plan, SourcePlan::Local { .. })
    }

    pub(crate) fn local(
        root: PathBuf,
        vcs: VcsType,
        target: LocalTarget,
        single_file: Option<PathBuf>,
        alternate_index: bool,
        anchors: Vec<PathBuf>,
    ) -> Self {
        Self {
            plan: SourcePlan::Local {
                root,
                vcs,
                target,
                single_file,
                alternate_index,
                anchors,
            },
        }
    }

    pub(crate) fn pull_request(
        checkout: Option<PathBuf>,
        repository: ForgeRepository,
        sha: Option<String>,
    ) -> Self {
        Self {
            plan: SourcePlan::PullRequest {
                checkout,
                repository,
                sha,
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn pinned_pr_sha(&self) -> Option<&str> {
        match &self.plan {
            SourcePlan::PullRequest { sha, .. } => sha.as_deref(),
            SourcePlan::Local { .. } => None,
        }
    }

    pub fn materialize(&self) -> Result<Snapshot, String> {
        let dirs = directories::ProjectDirs::from("", "", "tuicr")
            .ok_or("Could not determine the private snapshot cache directory")?;
        self.materialize_in(dirs.cache_dir().join("review-agent-snapshots"))
            .map_err(|err| format!("Could not build private review snapshot: {err}"))
    }

    /// Read only the requested local files, without creating an agent snapshot.
    /// A remote source is deliberately never resolved by this path.
    pub(crate) fn local_text(&self) -> impl Fn(&Path) -> Option<String> + '_ {
        let source = match &self.plan {
            SourcePlan::Local {
                root,
                vcs,
                target: LocalTarget::Worktree,
                single_file,
                ..
            } => root.canonicalize().ok().map(|root| Source::Worktree {
                root,
                single: single_file.as_deref(),
                vcs: *vcs,
                anchors: None,
            }),
            SourcePlan::Local {
                root,
                vcs: VcsType::Git,
                target: LocalTarget::Index,
                ..
            } => root.canonicalize().ok().map(Source::Index),
            SourcePlan::Local { .. } => Some(self.resolve()),
            SourcePlan::PullRequest { .. } => None,
        };
        move |path| {
            if !paths::local_relative(path) {
                return None;
            }
            let bytes = match source.as_ref()? {
                Source::Worktree { root, .. } => {
                    let mut bytes = Vec::new();
                    match paths::open_local_regular(root, path) {
                        Ok(file) => {
                            if file.metadata().ok()?.len() > MAX_FILE_BYTES {
                                return None;
                            }
                            file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes).ok()?;
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                        Err(_) => return None,
                    }
                    bytes
                }
                Source::Index(root) => vcs::git_text(root, "", path)?,
                Source::GitRevision { root, sha, .. } => vcs::git_text(root, sha, path)?,
                Source::HgRevision { root, sha, .. } => vcs::hg_text(root, sha, path)?,
                _ => return None,
            };
            (bytes.len() as u64 <= MAX_FILE_BYTES && !bytes.contains(&0))
                .then(|| String::from_utf8(bytes).ok())
                .flatten()
        }
    }

    fn materialize_in(&self, base: PathBuf) -> io::Result<Snapshot> {
        let cache = Cache::open(base)?;
        let source = self.resolve();
        let key = source.immutable_key();
        if let Some(key) = key.as_deref()
            && let Some(snapshot) = cache.get(key)?
        {
            return Ok(snapshot);
        }
        let stage = cache.stage()?;
        let mut build = Build::new(&stage.tree, key.is_none());
        let outcome = source.fill(&mut build);
        if let Err(notice) = outcome {
            // Never reuse a failed transfer. Discard partial bytes on failed
            // commands/parsing, and give this run a clean diff-only directory.
            drop(stage);
            let stage = cache.stage()?;
            let key = identity(format_args!("not-available:{notice}"));
            return cache.publish(&key, stage, notice);
        }
        let (content, notice) = build.finish();
        let key =
            key.unwrap_or_else(|| identity(format_args!("mutable:{source:?}:{content}:{notice}")));
        cache.publish(&key, stage, &notice)
    }

    fn resolve(&self) -> Source<'_> {
        match &self.plan {
            SourcePlan::Local {
                root,
                vcs,
                target,
                single_file,
                alternate_index,
                anchors,
            } => {
                let Ok(root) = root.canonicalize() else {
                    return Source::DiffOnly(UNAVAILABLE);
                };
                let alternate_index =
                    *alternate_index || std::env::var_os("GIT_INDEX_FILE").is_some();
                let git_checkout = match vcs {
                    VcsType::Git => vcs::is_git_checkout(&root),
                    VcsType::Jujutsu => vcs::is_colocated_jj(&root),
                    _ => false,
                };
                match *vcs {
                    VcsType::Git | VcsType::Jujutsu if git_checkout => match target {
                        LocalTarget::Worktree => Source::Worktree {
                            root,
                            single: single_file.as_deref(),
                            vcs: *vcs,
                            anchors: alternate_index.then_some(anchors.as_slice()),
                        },
                        LocalTarget::Index if alternate_index => Source::DiffOnly(
                            "Git uses an alternate index. On the index side, use the supplied diff.",
                        ),
                        LocalTarget::Index => Source::Index(root),
                        _ => vcs::git_revision(&root, *vcs, target).map_or(
                            Source::DiffOnly(UNAVAILABLE),
                            |sha| Source::GitRevision {
                                root,
                                sha,
                                single: single_file.as_deref(),
                                remote: None,
                            },
                        ),
                    },
                    VcsType::Mercurial
                        if !root.join(".hg").is_symlink() && root.join(".hg").is_dir() =>
                    {
                        if *target == LocalTarget::Worktree {
                            Source::Worktree {
                                root,
                                single: single_file.as_deref(),
                                vcs: *vcs,
                                anchors: None,
                            }
                        } else {
                            vcs::hg_revision(&root, target).map_or(
                                Source::DiffOnly(UNAVAILABLE),
                                |sha| Source::HgRevision {
                                    root,
                                    sha,
                                    single: single_file.as_deref(),
                                },
                            )
                        }
                    }
                    VcsType::File if *target == LocalTarget::Worktree && single_file.is_some() => {
                        Source::Worktree {
                            root,
                            single: single_file.as_deref(),
                            vcs: *vcs,
                            anchors: None,
                        }
                    }
                    _ => Source::DiffOnly(UNAVAILABLE),
                }
            }
            SourcePlan::PullRequest {
                checkout,
                repository,
                sha,
            } => {
                let Some(sha) = sha.as_deref().filter(|sha| is_full_object_id(sha)) else {
                    return Source::DiffOnly(
                        "No pinned source revision is available on this side; use the supplied diff.",
                    );
                };
                let local = checkout
                    .as_deref()
                    .and_then(|root| vcs::matching_git_checkout(root, repository))
                    .filter(|root| vcs::has_commit(root, sha));
                match local {
                    Some(root) => Source::GitRevision {
                        root,
                        sha: sha.to_owned(),
                        single: None,
                        remote: (repository.kind == ForgeKind::GitHub).then_some(repository),
                    },
                    None if repository.kind == ForgeKind::GitHub => {
                        Source::GitHub { repository, sha }
                    }
                    None => Source::DiffOnly(
                        "No matching local Git objects or supported forge archive are available; use the supplied diff.",
                    ),
                }
            }
        }
    }
}

pub(crate) fn is_object_id(id: &str) -> bool {
    (12..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_full_object_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && is_object_id(id)
}

fn identity(value: std::fmt::Arguments<'_>) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("review-snapshot-v1\0{value}"))
    )
}

#[derive(Debug)]
enum Source<'a> {
    DiffOnly(&'static str),
    Worktree {
        root: PathBuf,
        single: Option<&'a Path>,
        vcs: VcsType,
        anchors: Option<&'a [PathBuf]>,
    },
    Index(PathBuf),
    GitRevision {
        root: PathBuf,
        sha: String,
        single: Option<&'a Path>,
        remote: Option<&'a ForgeRepository>,
    },
    GitHub {
        repository: &'a ForgeRepository,
        sha: &'a str,
    },
    HgRevision {
        root: PathBuf,
        sha: String,
        single: Option<&'a Path>,
    },
}

impl Source<'_> {
    fn immutable_key(&self) -> Option<String> {
        match self {
            Self::Worktree { .. } | Self::Index(_) => None,
            _ => Some(identity(format_args!("{self:?}"))),
        }
    }

    fn fill(&self, build: &mut Build<'_>) -> Result<(), &'static str> {
        match self {
            Self::DiffOnly(notice) => {
                build.limit(notice);
                Ok(())
            }
            Self::Worktree {
                root,
                single,
                vcs,
                anchors,
            } => vcs::worktree(root, *single, *vcs, *anchors, build),
            Self::Index(root) => vcs::git_index(root, build),
            Self::GitRevision {
                root,
                sha,
                single,
                remote,
            } => match vcs::git_archive(root, sha, *single, build) {
                Err(_) if remote.is_some() => {
                    build.reset()?;
                    vcs::github_archive(remote.ok_or(UNAVAILABLE)?, sha, build)
                }
                result => result,
            },
            Self::GitHub { repository, sha } => vcs::github_archive(repository, sha, build),
            Self::HgRevision { root, sha, single } => vcs::hg_archive(root, sha, *single, build),
        }
    }
}

struct Build<'a> {
    root: &'a Path,
    limits: BTreeSet<&'static str>,
    hash: Option<Sha256>,
    files: usize,
    bytes: u64,
}

impl<'a> Build<'a> {
    fn new(root: &'a Path, mutable: bool) -> Self {
        Self {
            root,
            limits: BTreeSet::new(),
            hash: mutable.then(Sha256::new),
            files: 0,
            bytes: 0,
        }
    }

    fn reset(&mut self) -> Result<(), &'static str> {
        for entry in fs::read_dir(self.root).map_err(|_| "Could not reset the snapshot")? {
            let entry = entry.map_err(|_| "Could not reset the snapshot")?;
            let result = if entry.file_type().is_ok_and(|ty| ty.is_dir()) {
                fs::remove_dir_all(entry.path())
            } else {
                fs::remove_file(entry.path())
            };
            result.map_err(|_| "Could not reset the snapshot")?;
        }
        *self = Self::new(self.root, self.hash.is_some());
        Ok(())
    }

    fn limit(&mut self, notice: &'static str) {
        self.limits.insert(notice);
    }

    fn finish(self) -> (String, String) {
        let content = self
            .hash
            .map_or_else(String::new, |hash| format!("{:x}", hash.finalize()));
        (
            content,
            self.limits.into_iter().collect::<Vec<_>>().join(" "),
        )
    }

    fn within_budget(&mut self) -> bool {
        if self.files >= MAX_FILES {
            self.limit("Snapshot stopped at 20,000 files.");
            false
        } else if self.bytes >= MAX_TREE_BYTES {
            self.limit("Snapshot stopped at 64 MiB of file content.");
            false
        } else {
            true
        }
    }

    fn write(&mut self, rel: &Path, size: u64, mut contents: impl Read) -> io::Result<()> {
        if !paths::safe_relative(rel) {
            self.limit(UNSAFE_PATH);
            return Ok(());
        }
        if !self.within_budget() {
            return Ok(());
        }
        if size > MAX_FILE_BYTES {
            self.limit("Files larger than 8 MiB were omitted.");
            return Ok(());
        }
        if size > MAX_TREE_BYTES - self.bytes {
            self.limit("Files exceeding the 64 MiB snapshot byte budget were omitted.");
            return Ok(());
        }
        let dest = self.root.join(rel);
        let parent = dest
            .parent()
            .ok_or_else(|| io::Error::other("invalid snapshot path"))?;
        fs::create_dir_all(parent)?;
        // No archive entry creates a directory, link or device. The stage is
        // privately owned and create_new also rejects colliding filenames.
        let mut opts = OpenOptions::new();
        opts.create_new(true).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let file = match opts.open(&dest) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                self.limit("Duplicate or case-colliding snapshot paths were omitted.");
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        let mut file = io::BufWriter::new(file);
        let mut bytes = 0;
        let mut digest = self.hash.as_ref().map(|_| Sha256::new());
        let mut buf = [0u8; 16 * 1024];
        let mut contents = contents.by_ref().take(size + 1);
        let valid = loop {
            let n = contents.read(&mut buf)?;
            if n == 0 {
                break bytes == size;
            }
            bytes += n as u64;
            if bytes > size {
                break false;
            }
            file.write_all(&buf[..n])?;
            if let Some(digest) = &mut digest {
                digest.update(&buf[..n]);
            }
        };
        file.flush()?;
        if !valid {
            drop(file);
            fs::remove_file(dest)?;
            self.limit("Files that changed while copying were omitted.");
            return Ok(());
        }
        let mut perms = file.get_ref().metadata()?.permissions();
        #[cfg(unix)]
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o400);
        #[cfg(not(unix))]
        perms.set_readonly(true);
        file.get_ref().set_permissions(perms)?;
        self.files += 1;
        self.bytes += size;
        if let Some((hash, digest)) = self.hash.as_mut().zip(digest) {
            hash.update(rel.as_os_str().as_encoded_bytes());
            hash.update([0]);
            hash.update(digest.finalize());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
