mod checkout;
mod paths;
mod vcs;

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::forge::traits::{ForgeKind, ForgeRepository};
use crate::vcs::traits::VcsType;

use checkout::{Cache, Checkout};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const UNAVAILABLE: &str = "No readable checkout is available on this side; use the supplied diff.";
const UNCONFIRMED: &str =
    "No pinned source revision is available on this side; use the supplied diff.";
const NO_CHECKOUT: &str =
    "No matching local Git checkout or supported forge clone is available; use the supplied diff.";
const REVISION_UNAVAILABLE: &str = "The exact reviewed revision is unavailable. This directory is the current working copy, which may differ from the supplied diff; rely on the diff for the reviewed content.";
const ALTERNATE_WORKTREE: &str = "The reviewed side comes from an alternate Git index (GIT_INDEX_FILE), so working-tree files may differ from the supplied diff.";
const STAGED_INDEX: &str = "The reviewed side is the staged Git index. This directory is the working tree, which may contain unstaged edits; use `git show :<path>` for the staged content.";
const ALTERNATE_INDEX: &str = "The reviewed side is an alternate Git index (GIT_INDEX_FILE) that cannot be read exactly from this checkout. This directory is the working tree, which may differ from the supplied diff.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRequest {
    plan: SourcePlan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum SourcePlan {
    Local {
        root: PathBuf,
        vcs: VcsType,
        target: LocalTarget,
        alternate_index: bool,
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

/// The directory a provider runs in, with an optional note about how it
/// relates to the reviewed side. Dropping it releases the checkout so it can
/// be pruned once it is old.
pub struct Workspace {
    dir: PathBuf,
    note: Option<String>,
    _lease: Option<File>,
    _scratch: Option<tempfile::TempDir>,
}

impl Workspace {
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    fn in_place(dir: PathBuf, note: Option<String>) -> Self {
        Self {
            dir,
            note,
            _lease: None,
            _scratch: None,
        }
    }

    fn checkout(checkout: Checkout) -> Self {
        Self {
            dir: checkout.dir,
            note: None,
            _lease: Some(checkout.lease),
            _scratch: None,
        }
    }
}

/// Where a request's source lives, decided without creating anything.
#[derive(Debug)]
enum Resolved<'a> {
    /// The user's own directory (live worktree, or the best available).
    InPlace { root: PathBuf, note: Option<String> },
    /// A checkout of one commit, made from the user's own repository.
    Revision {
        root: PathBuf,
        sha: String,
        /// The remote to fetch `sha` from when the repository lacks it.
        fetch_from: Option<String>,
        /// Used instead when the fetch fails.
        mirror: Option<&'a ForgeRepository>,
    },
    /// A checkout of one commit from a private GitHub mirror.
    Mirror {
        repository: &'a ForgeRepository,
        sha: String,
    },
    /// Nothing readable: an empty directory and an explanation.
    Empty(&'static str),
}

enum Revision {
    Git(String),
    Hg(String),
}

impl SourceRequest {
    pub(crate) fn is_local(&self) -> bool {
        matches!(&self.plan, SourcePlan::Local { .. })
    }

    pub(crate) fn local(
        root: PathBuf,
        vcs: VcsType,
        target: LocalTarget,
        alternate_index: bool,
    ) -> Self {
        Self {
            plan: SourcePlan::Local {
                root,
                vcs,
                target,
                alternate_index,
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

    /// The directory a provider reads the reviewed side from: the live
    /// checkout for the working tree, otherwise a private checkout of the
    /// reviewed revision with its full history.
    pub fn workspace(&self) -> Result<Workspace, String> {
        let dirs = directories::ProjectDirs::from("", "", "tuicr")
            .ok_or("Could not determine the tuicr cache directory")?;
        self.workspace_in(dirs.cache_dir())
            .map_err(|error| format!("Could not prepare the source checkout: {error}"))
    }

    fn workspace_in(&self, base: &Path) -> Result<Workspace, String> {
        let cache = || Cache::open(base).map_err(|error| error.to_string());
        let empty = |note: &str| {
            let scratch = cache()?.empty_dir().map_err(|error| error.to_string())?;
            Ok(Workspace {
                dir: scratch.path().to_owned(),
                note: Some(note.to_owned()),
                _lease: None,
                _scratch: Some(scratch),
            })
        };
        // A pull request's commit may simply be unreachable (offline, no
        // access): tell the provider so and let it use the diff.
        let remote = |result: Result<Checkout, String>| match result {
            Ok(checkout) => Ok(Workspace::checkout(checkout)),
            Err(error) => empty(&format!(
                "The pinned source could not be fetched ({error}); use the supplied diff."
            )),
        };
        match self.resolve() {
            Resolved::InPlace { root, note } => Ok(Workspace::in_place(root, note)),
            Resolved::Revision {
                root,
                sha,
                fetch_from,
                mirror,
            } => {
                let cache = cache()?;
                if let Some(remote_name) = fetch_from
                    && let Err(error) = checkout::fetch(&root, &remote_name, &sha)
                {
                    return match mirror {
                        Some(repository) => remote(cache.github(repository, &sha)),
                        None => remote(Err(error)),
                    };
                }
                cache.local(&root, &sha).map(Workspace::checkout)
            }
            Resolved::Mirror { repository, sha } => remote(cache()?.github(repository, &sha)),
            Resolved::Empty(note) => empty(note),
        }
    }

    /// Read only the requested local files, without creating a workspace.
    /// A remote source is deliberately never resolved by this path.
    pub(crate) fn local_text(&self) -> impl Fn(&Path) -> Option<String> + '_ {
        enum Reader {
            Worktree(PathBuf),
            Index(PathBuf),
            Git(PathBuf, String),
            Hg(PathBuf, String),
        }
        let reader = match &self.plan {
            SourcePlan::Local {
                root,
                target: LocalTarget::Worktree,
                ..
            } => root.canonicalize().ok().map(Reader::Worktree),
            SourcePlan::Local {
                root,
                vcs: VcsType::Git,
                target: LocalTarget::Index,
                ..
            } => root.canonicalize().ok().map(Reader::Index),
            SourcePlan::Local {
                root, vcs, target, ..
            } => root.canonicalize().ok().and_then(|root| {
                match local_revision(&root, *vcs, target)? {
                    Revision::Git(sha) => Some(Reader::Git(root, sha)),
                    Revision::Hg(sha) => Some(Reader::Hg(root, sha)),
                }
            }),
            SourcePlan::PullRequest { .. } => None,
        };
        move |path| {
            if !paths::local_relative(path) {
                return None;
            }
            let bytes = match reader.as_ref()? {
                Reader::Worktree(root) => {
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
                Reader::Index(root) => vcs::git_text(root, "", path)?,
                Reader::Git(root, sha) => vcs::git_text(root, sha, path)?,
                Reader::Hg(root, sha) => vcs::hg_text(root, sha, path)?,
            };
            (bytes.len() as u64 <= MAX_FILE_BYTES && !bytes.contains(&0))
                .then(|| String::from_utf8(bytes).ok())
                .flatten()
        }
    }

    fn resolve(&self) -> Resolved<'_> {
        match &self.plan {
            SourcePlan::Local {
                root,
                vcs,
                target,
                alternate_index,
            } => {
                let Ok(root) = root.canonicalize() else {
                    return Resolved::Empty(UNAVAILABLE);
                };
                let alternate = (*alternate_index || std::env::var_os("GIT_INDEX_FILE").is_some())
                    && matches!(vcs, VcsType::Git | VcsType::Jujutsu);
                let note = |text: &str| Some(text.to_owned());
                match target {
                    LocalTarget::Worktree => Resolved::InPlace {
                        root,
                        note: alternate.then(|| ALTERNATE_WORKTREE.to_owned()),
                    },
                    LocalTarget::Index => Resolved::InPlace {
                        root,
                        note: note(if alternate {
                            ALTERNATE_INDEX
                        } else {
                            STAGED_INDEX
                        }),
                    },
                    _ => match local_revision(&root, *vcs, target) {
                        Some(Revision::Git(sha)) => Resolved::Revision {
                            root,
                            sha,
                            fetch_from: None,
                            mirror: None,
                        },
                        Some(Revision::Hg(rev)) => Resolved::InPlace {
                            root,
                            note: Some(format!(
                                "The reviewed side is Mercurial revision {rev}. This directory is the current working copy, which may differ from the supplied diff; use `hg cat -r {rev} <path>` for that revision's content."
                            )),
                        },
                        None => Resolved::InPlace {
                            root,
                            note: note(REVISION_UNAVAILABLE),
                        },
                    },
                }
            }
            SourcePlan::PullRequest {
                checkout,
                repository,
                sha,
            } => {
                let Some(sha) = sha.as_deref().filter(|sha| is_full_object_id(sha)) else {
                    return Resolved::Empty(UNCONFIRMED);
                };
                let github = (repository.kind == ForgeKind::GitHub).then_some(repository);
                match checkout
                    .as_deref()
                    .and_then(|root| vcs::matching_git_checkout(root, repository))
                {
                    Some((root, remote)) => {
                        let present = vcs::has_commit(&root, sha);
                        Resolved::Revision {
                            root,
                            sha: sha.to_owned(),
                            fetch_from: (!present).then_some(remote),
                            mirror: github,
                        }
                    }
                    None => match github {
                        Some(repository) => Resolved::Mirror {
                            repository,
                            sha: sha.to_owned(),
                        },
                        None => Resolved::Empty(NO_CHECKOUT),
                    },
                }
            }
        }
    }
}

/// The commit (or Mercurial revision) a local target names, when the root is
/// a checkout the target's VCS can read.
fn local_revision(root: &Path, vcs: VcsType, target: &LocalTarget) -> Option<Revision> {
    match vcs {
        VcsType::Git if vcs::is_git_checkout(root) => {
            vcs::git_revision(root, vcs, target).map(Revision::Git)
        }
        VcsType::Jujutsu if vcs::is_colocated_jj(root) => {
            vcs::git_revision(root, vcs, target).map(Revision::Git)
        }
        VcsType::Mercurial if !root.join(".hg").is_symlink() && root.join(".hg").is_dir() => {
            vcs::hg_revision(root, target).map(Revision::Hg)
        }
        _ => None,
    }
}

pub(crate) fn is_object_id(id: &str) -> bool {
    (12..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_full_object_id(id: &str) -> bool {
    matches!(id.len(), 40 | 64) && is_object_id(id)
}

#[cfg(test)]
mod tests;
