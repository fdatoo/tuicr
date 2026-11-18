//! Companion exchanges are stored separately from ordinary review sessions.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_segmentation::UnicodeSegmentation;

use crate::model::review::path_codec;
use crate::model::{LineRange, LineSide, ReviewSession};

// Accommodate the largest runner response after JSON escaping and metadata.
const RECORD_LIMIT: u64 = 2 * 1024 * 1024;
pub(crate) const GUIDE_PROVIDER: &str = "review guide";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReviewId([u8; 32]);

impl ReviewId {
    pub(super) fn from_session(session: &ReviewSession) -> Self {
        let mut hash = Sha256::new();
        if let Some(key) = &session.pr_session_key {
            // History belongs to the PR; individual threads include their revision.
            hash.update(b"pr1\0");
            hash.update(format!("{:?}\0", key.repository.kind).as_bytes());
            hash.update(key.repository.slug().as_bytes());
            hash.update(b"\0");
            hash.update(crate::slug::normalized_host(&key.repository.host).as_bytes());
            hash.update(key.number.to_le_bytes());
        } else {
            hash.update(b"local1\0");
            hash.update(session.repo_path.as_os_str().as_encoded_bytes());
            hash.update(b"\0");
            hash.update(format!("{:?}\0", session.diff_source).as_bytes());
            hash.update(session.base_commit.as_bytes());
            hash.update(b"\0");
            if let Some(branch) = &session.branch_name {
                hash.update(branch.as_bytes());
            }
            if let Some(commits) = &session.commit_range {
                for commit in commits {
                    hash.update(b"\0");
                    hash.update(commit.as_bytes());
                }
            }
        }
        Self(hash.finalize().into())
    }

    fn directory(self, base: &Path) -> PathBuf {
        use std::fmt::Write as _;
        let hex = self
            .0
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
                hex
            });
        base.join(hex)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub(crate) enum AnchorTarget {
    Review,
    File {
        #[serde(with = "path_codec")]
        path: PathBuf,
        side: LineSide,
    },
    Line {
        #[serde(with = "path_codec")]
        path: PathBuf,
        side: LineSide,
        range: LineRange,
    },
}

impl AnchorTarget {
    pub(super) fn path(&self) -> Option<&Path> {
        match self {
            Self::File { path, .. } | Self::Line { path, .. } => Some(path),
            Self::Review => None,
        }
    }

    pub(super) fn side(&self) -> LineSide {
        match self {
            Self::File { side, .. } | Self::Line { side, .. } => *side,
            Self::Review => LineSide::New,
        }
    }

    pub(super) fn focus_line(&self) -> Option<u32> {
        match self {
            Self::Line { range, .. } => Some(range.end),
            _ => None,
        }
    }

    pub(super) fn label(&self) -> String {
        match self {
            Self::Review => "Review".into(),
            Self::File { path, .. } => path.display().to_string(),
            Self::Line { path, range, side } => {
                let side = if *side == LineSide::Old { "old" } else { "new" };
                if range.is_single() {
                    format!("{}:{} ({side})", path.display(), range.end)
                } else {
                    format!("{}:{}–{} ({side})", path.display(), range.start, range.end)
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Anchor {
    pub(super) target: AnchorTarget,
    pub(super) revision: String,
    pub(super) file_hash: Option<u64>,
    pub(super) comment_id: Option<String>,
}

impl Anchor {
    pub(crate) fn label(&self) -> String {
        self.target.label()
    }

    pub(super) fn same_thread(&self, other: &Self) -> bool {
        self.target == other.target && self.comment_id == other.comment_id
    }
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Exchange {
    pub(super) time: DateTime<Utc>,
    pub(crate) anchor: Anchor,
    pub(crate) question: String,
    pub(crate) answer: String,
    pub(crate) provider: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) fold_answer: bool,
}

impl Exchange {
    pub(super) fn new(anchor: Anchor, question: String, answer: String, provider: &str) -> Self {
        Self {
            time: Utc::now(),
            anchor,
            question,
            answer,
            provider: provider.into(),
            fold_answer: false,
        }
    }

    pub(crate) fn short_preview(&self) -> Option<&str> {
        if self.fold_answer {
            short_answer_preview(&self.answer)
        } else {
            None
        }
    }
}

pub(super) fn short_answer_preview(answer: &str) -> Option<&str> {
    let mut sentences = answer.split_sentence_bounds();
    let mut end = 0;
    for _ in 0..3 {
        end += sentences.next()?.len();
    }
    sentences.next().map(|_| answer[..end].trim_end())
}

pub(super) fn base_directory() -> io::Result<PathBuf> {
    #[cfg(test)]
    {
        let reviews = crate::persistence::storage::get_reviews_dir().map_err(io::Error::other)?;
        Ok(reviews.join("companion-test"))
    }
    #[cfg(not(test))]
    {
        let dirs = directories::ProjectDirs::from("", "", "tuicr")
            .ok_or_else(|| io::Error::other("could not determine the data directory"))?;
        Ok(dirs.data_dir().join("companion"))
    }
}

pub(super) fn load(base: &Path, review: ReviewId) -> io::Result<Vec<Exchange>> {
    let dir = review.directory(base);
    for path in [base, dir.as_path()] {
        match verify_private_dir(path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err),
        }
    }
    let mut history = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.path().extension().is_none_or(|ext| ext != "json") || !entry.file_type()?.is_file()
        {
            continue;
        }
        let file = fs::File::open(entry.path())?;
        if file.metadata()?.len() > RECORD_LIMIT {
            return Err(io::Error::other("private history entry is too large"));
        }
        history.push(serde_json::from_reader::<_, Exchange>(
            file.take(RECORD_LIMIT),
        )?);
    }
    history.sort_by_key(|exchange| exchange.time);
    Ok(history)
}

pub(super) fn save(base: &Path, review: ReviewId, exchange: &Exchange) -> io::Result<()> {
    if let Some(parent) = base.parent() {
        fs::create_dir_all(parent)?;
    }
    create_private_dir(base)?;
    let dir = review.directory(base);
    create_private_dir(&dir)?;
    // Each completed answer is an independent atomic write. Two panes can
    // contribute to the same conversation without overwriting one another.
    let mut file = tempfile::NamedTempFile::new_in(&dir)?;
    serde_json::to_writer(&mut file, exchange)?;
    file.flush()?;
    file.as_file().sync_all()?;
    let record = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
    file.persist_noclobber(record).map_err(|err| err.error)?;
    Ok(())
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    match builder.create(path) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err),
    }
    verify_private_dir(path)
}

fn verify_private_dir(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Err(io::Error::other("private history path is not a directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "private history directory is accessible by other users",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SessionDiffSource;

    #[test]
    fn companion_history_persists_without_creating_a_review_session() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("companion");
        let session = ReviewSession::new(
            dir.path().join("repo"),
            "reviewed-sha".into(),
            Some("main".into()),
            SessionDiffSource::CommitRange,
        );
        let review = ReviewId::from_session(&session);
        let anchor = Anchor {
            target: AnchorTarget::Line {
                path: "src/main.rs".into(),
                side: LineSide::Old,
                range: LineRange::new(9, 11),
            },
            revision: "base-sha".into(),
            file_hash: Some(42),
            comment_id: None,
        };
        let exchange = Exchange::new(anchor.clone(), "question".into(), "answer".into(), "claude");
        assert!(load(&base, review).unwrap().is_empty());
        save(&base, review, &exchange).unwrap();
        let mut reopened = session.clone();
        reopened.id = "another ephemeral review session id".into();
        let history = load(&base, ReviewId::from_session(&reopened)).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].anchor, anchor);
        assert_eq!(history[0].answer, "answer");
        assert!(!session.has_comments());
        assert!(!dir.path().join("reviews").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = fs::read_dir(review.directory(&base))
                .unwrap()
                .next()
                .unwrap()
                .unwrap();
            let mode = file.metadata().unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
