//! Review guides: a map of a change as stories of stops, each a claim to verify.
//!
//! This module is the contract shared by preparation (which writes guides), the
//! TUI (which walks them and records verdicts) and the companion (which answers
//! from them). It holds data and storage only; no UI and no provider calls.

pub(crate) mod baseline;
pub mod mechanical;
pub mod outline;
pub mod resolve;

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const GUIDE_VERSION: u32 = 2;
/// A review is planned to fit one sitting of this many minutes.
pub const SITTING_MINUTES: u32 = 15;
/// A preparation that reports no progress for this long is treated as failed.
pub const STALL_AFTER_SECS: i64 = 5 * 60;

const GUIDE_FILE: &str = "guide.json";
const STATE_FILE: &str = "state.json";
const PROGRESS_FILE: &str = "progress.json";
pub const LOG_FILE: &str = "prepare.log";
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewGuide {
    pub version: u32,
    /// Digest of the reviewed diff this guide was written for (`App::review_digest`).
    pub diff: String,
    /// One or two sentences on what the change is for.
    #[serde(default)]
    pub summary: String,
    /// Most important first, so stopping at any point leaves the best coverage.
    pub stories: Vec<Story>,
    /// Every changed hunk that no stop covers, grouped by who vouches for it.
    #[serde(default)]
    pub supporting: Vec<SupportingGroup>,
    #[serde(default)]
    pub estimate_minutes: Option<u32>,
    #[serde(default)]
    pub provenance: Provenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Story {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub stops: Vec<Stop>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    /// Unique within the guide and stable across re-preparation of unchanged code.
    pub id: String,
    /// One sentence: what this code is supposed to do. The reviewer verifies it.
    pub claim: String,
    /// Two to four sentences of Markdown that start one level above the code.
    #[serde(default)]
    pub explanation: String,
    /// Display path of the file in the diff.
    pub path: String,
    #[serde(default)]
    pub side: Side,
    /// Inclusive source line numbers on `side`. The block must lie inside the diff's hunks.
    pub start_line: u32,
    pub end_line: u32,
    /// Signature of the enclosing function or type, when the block is inside one.
    #[serde(default)]
    pub symbol: Option<String>,
    /// The test that would fail if the claim were false.
    #[serde(default)]
    pub test: Option<TestRef>,
    /// The reviewing agent looked for a covering test and found none.
    #[serde(default)]
    pub untested: bool,
    #[serde(default)]
    pub risk: Option<Risk>,
    #[serde(default)]
    pub audit: Audit,
    /// Likely follow-up questions answered during preparation.
    #[serde(default)]
    pub followups: Vec<Followup>,
}

impl Stop {
    /// Risky, untested or doubted stops cannot be passed with the default key.
    pub fn needs_explicit_verdict(&self) -> bool {
        self.risk.is_some() || self.untested || matches!(self.audit, Audit::Doubtful { .. })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Old,
    #[default]
    New,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestRef {
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Risk {
    pub kind: RiskKind,
    /// One sentence the reviewer reads on the stop.
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskKind {
    /// The description does not account for this change.
    Unexplained,
    /// An existing test was edited or removed.
    TestChanged,
    CodeRemoved,
    /// Build, CI, dependency, permission or deployment configuration.
    Config,
    ErrorHandling,
    /// Flagged by the reviewer's own rules file.
    Rule,
    Other,
}

/// Outcome of the second pass that tries to refute each claim.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Audit {
    #[default]
    Unchecked,
    Holds,
    Doubtful {
        note: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Followup {
    pub question: String,
    pub answer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportingGroup {
    /// For example "new tests", "docs", "pure moves", "wiring".
    pub label: String,
    pub basis: Basis,
    #[serde(default)]
    pub note: String,
    pub hunks: Vec<HunkRef>,
}

/// Only mechanically verified groups may be dismissed in bulk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Basis {
    /// tuicr checked the label itself, without a model.
    Mechanical,
    /// The label rests on an agent's word.
    Claimed,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct HunkRef {
    pub path: String,
    pub old_start: u32,
    pub new_start: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    #[serde(default)]
    pub author: Author,
    /// Whether a reviewing agent checked the guide against the code.
    #[serde(default)]
    pub audited: bool,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    /// Digest of the rules file used, to detect guides prepared with older rules.
    #[serde(default)]
    pub rules_digest: Option<String>,
    #[serde(default)]
    pub prepared_at: Option<DateTime<Utc>>,
    /// Set when preparation could not read the whole revision, with the reason.
    #[serde(default)]
    pub limits: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Author {
    /// Written by tuicr's reviewing agent.
    #[default]
    Reviewer,
    /// Supplied by the change's author (or their agent).
    Supplied,
}

impl ReviewGuide {
    /// Stops in reading order with their story index.
    pub fn stops(&self) -> impl Iterator<Item = (usize, &Stop)> {
        self.stories
            .iter()
            .enumerate()
            .flat_map(|(story, s)| s.stops.iter().map(move |stop| (story, stop)))
    }

    pub fn stop_count(&self) -> usize {
        self.stories.iter().map(|story| story.stops.len()).sum()
    }

    pub fn stop(&self, id: &str) -> Option<&Stop> {
        self.stops()
            .map(|(_, stop)| stop)
            .find(|stop| stop.id == id)
    }

    /// Structural checks that need no diff. Anchors are resolved by the TUI.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != GUIDE_VERSION {
            return Err(format!(
                "guide version {} is not supported (expected {GUIDE_VERSION})",
                self.version
            ));
        }
        if self.stop_count() == 0 {
            return Err("guide has no stops".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for (index, story) in self.stories.iter().enumerate() {
            if story.title.trim().is_empty() {
                return Err(format!("story {} has no title", index + 1));
            }
            for stop in &story.stops {
                if stop.id.trim().is_empty() || !seen.insert(stop.id.as_str()) {
                    return Err(format!(
                        "story {}: stop id {:?} is empty or repeated",
                        index + 1,
                        stop.id
                    ));
                }
                if stop.claim.trim().is_empty() {
                    return Err(format!("stop {:?} has no claim", stop.id));
                }
                if stop.start_line == 0 || stop.end_line < stop.start_line {
                    return Err(format!(
                        "stop {:?} ({}) has an invalid line range {}-{}",
                        stop.id, stop.path, stop.start_line, stop.end_line
                    ));
                }
            }
        }
        Ok(())
    }
}

// ---------- Reviewer's progress ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    ChecksOut,
    Wrong,
    Parked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictRecord {
    pub verdict: Verdict,
    pub at: DateTime<Utc>,
    /// The block's text when the verdict was given. A verdict only stands while
    /// the same text is still present in the diff.
    pub block: Vec<String>,
    /// The review comment written for a `Wrong` verdict.
    #[serde(default)]
    pub comment_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpotCheck {
    pub hunk: HunkRef,
    pub verdict: Verdict,
}

/// What the reviewer has done with a guide. Stored beside it, never inside it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    #[serde(default)]
    pub verdicts: BTreeMap<String, VerdictRecord>,
    #[serde(default)]
    pub spot_checks: Vec<SpotCheck>,
    /// Diff that the spot-check hunk coordinates belong to.
    #[serde(default)]
    pub spot_diff: Option<String>,
    /// Stop to reopen on.
    #[serde(default)]
    pub current: Option<String>,
    /// Supporting groups dismissed in bulk (mechanical basis only), by label.
    #[serde(default)]
    pub dismissed_groups: Vec<String>,
    #[serde(default)]
    pub elapsed_secs: u64,
}

// ---------- Preparation job state ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrepStatus {
    Preparing,
    Ready,
    Failed,
    Cancelled,
}

/// The target used by a preparation job, independent of whether a review
/// session with user progress has been saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PrepTarget {
    PullRequest {
        repository: crate::forge::traits::ForgeRepository,
        number: u64,
        head_sha: String,
    },
    Local {
        repo_root: PathBuf,
        source: crate::model::SessionDiffSource,
        commits: Vec<String>,
        path_filter: Option<String>,
        revision: Option<(Option<String>, String)>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrepState {
    pub status: PrepStatus,
    /// Short present-tense description of the current step, shown to the reviewer.
    #[serde(default)]
    pub step: String,
    #[serde(default)]
    pub pid: Option<u32>,
    pub started_at: DateTime<Utc>,
    /// Updated on every sign of progress from the reviewing agent.
    pub heartbeat_at: DateTime<Utc>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    /// Digest of the diff being prepared.
    #[serde(default)]
    pub diff: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub stops: Option<u32>,
    #[serde(default)]
    pub estimate_minutes: Option<u32>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub target: Option<PrepTarget>,
}

impl PrepState {
    pub fn started(step: impl Into<String>, diff: Option<String>, title: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            status: PrepStatus::Preparing,
            step: step.into(),
            pid: Some(std::process::id()),
            started_at: now,
            heartbeat_at: now,
            finished_at: None,
            diff,
            title,
            stops: None,
            estimate_minutes: None,
            error: None,
            target: None,
        }
    }

    /// A job that stopped reporting progress is stuck, however long real work may take.
    pub fn is_stalled(&self, now: DateTime<Utc>) -> bool {
        self.status == PrepStatus::Preparing
            && now.signed_duration_since(self.heartbeat_at) > Duration::seconds(STALL_AFTER_SECS)
    }

    /// The status a reader should act on: a stalled job reads as failed.
    pub fn effective_status(&self, now: DateTime<Utc>) -> PrepStatus {
        if self.is_stalled(now) {
            PrepStatus::Failed
        } else {
            self.status
        }
    }
}

// ---------- Storage ----------

/// Identity of a review that stays the same while its diff changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReviewKey(String);

impl ReviewKey {
    pub fn for_pull_request(forge: &str, host: &str, owner: &str, name: &str, number: u64) -> Self {
        Self::digest(&["pr", forge, host, owner, name, &number.to_string()])
    }

    pub(crate) fn for_repository(
        repository: &crate::forge::traits::ForgeRepository,
        number: u64,
    ) -> Self {
        Self::for_pull_request(
            crate::slug::forge_code(repository.kind),
            &crate::slug::normalized_host(&repository.host),
            &repository.owner,
            &repository.name,
            number,
        )
    }

    /// `scope` distinguishes reviews of one checkout, for example `worktree`,
    /// `staged` or `commits:<first>..<last>`.
    pub fn for_local(repo_root: &Path, scope: &str) -> Self {
        Self::digest(&["local", &repo_root.to_string_lossy(), scope])
    }

    pub(crate) fn from_digest(digest: String) -> Option<Self> {
        (digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')))
        .then_some(Self(digest))
    }

    fn digest(fields: &[&str]) -> Self {
        let mut hasher = Sha256::new();
        for field in fields {
            // Length-prefix each field so adjacent values cannot collide.
            hasher.update((field.len() as u64).to_le_bytes());
            hasher.update(field.as_bytes());
        }
        Self(format!("{:x}", hasher.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Directory holding every review's guide, state and progress.
pub fn guides_dir() -> io::Result<PathBuf> {
    #[cfg(test)]
    {
        let reviews = crate::persistence::storage::get_reviews_dir().map_err(io::Error::other)?;
        Ok(reviews.join("guides-test"))
    }
    #[cfg(not(test))]
    {
        let dirs = directories::ProjectDirs::from("", "", "tuicr")
            .ok_or_else(|| io::Error::other("could not determine the data directory"))?;
        Ok(dirs.data_dir().join("guides"))
    }
}

/// One review's files inside `base` (normally `guides_dir()`).
#[derive(Debug, Clone)]
pub struct GuideStore {
    dir: PathBuf,
}

impl GuideStore {
    pub fn open(key: &ReviewKey) -> io::Result<Self> {
        Ok(Self::in_dir(&guides_dir()?, key))
    }

    pub fn in_dir(base: &Path, key: &ReviewKey) -> Self {
        Self {
            dir: base.join(key.as_str()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn log_path(&self) -> PathBuf {
        self.dir.join(LOG_FILE)
    }

    pub fn guide(&self) -> io::Result<Option<ReviewGuide>> {
        self.read(GUIDE_FILE)
    }

    pub fn save_guide(&self, guide: &ReviewGuide) -> io::Result<()> {
        self.write(GUIDE_FILE, guide)
    }

    pub fn state(&self) -> io::Result<Option<PrepState>> {
        self.read(STATE_FILE)
    }

    pub fn save_state(&self, state: &PrepState) -> io::Result<()> {
        self.write(STATE_FILE, state)
    }

    pub fn progress(&self) -> io::Result<Option<Progress>> {
        self.read(PROGRESS_FILE)
    }

    pub fn save_progress(&self, progress: &Progress) -> io::Result<()> {
        self.write(PROGRESS_FILE, progress)
    }

    fn read<T: DeserializeOwned>(&self, name: &str) -> io::Result<Option<T>> {
        let path = self.dir.join(name);
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(err),
        };
        if file.metadata()?.len() > MAX_FILE_BYTES {
            return Err(io::Error::other(format!("{} is too large", path.display())));
        }
        serde_json::from_reader(io::BufReader::new(file))
            .map(Some)
            .map_err(|err| io::Error::other(format!("{}: {err}", path.display())))
    }

    /// Readers in other processes (the TUI, the CLI) never see a partial file.
    fn write<T: Serialize>(&self, name: &str, value: &T) -> io::Result<()> {
        create_private_dir(&self.dir)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.flush()?;
        file.persist(self.dir.join(name)).map_err(|err| err.error)?;
        Ok(())
    }
}

fn create_private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stop(id: &str) -> Stop {
        Stop {
            id: id.into(),
            claim: "Retries reuse the idempotency key.".into(),
            explanation: String::new(),
            path: "src/orders.rs".into(),
            side: Side::New,
            start_line: 10,
            end_line: 14,
            symbol: None,
            test: None,
            untested: false,
            risk: None,
            audit: Audit::Unchecked,
            followups: Vec::new(),
        }
    }

    fn guide(stops: Vec<Stop>) -> ReviewGuide {
        ReviewGuide {
            version: GUIDE_VERSION,
            diff: "sha256:x".into(),
            summary: String::new(),
            stories: vec![Story {
                title: "Retry".into(),
                summary: String::new(),
                stops,
            }],
            supporting: Vec::new(),
            estimate_minutes: None,
            provenance: Provenance::default(),
        }
    }

    #[test]
    fn minimal_author_guide_parses_with_defaults_and_rejects_repeated_ids() {
        let parsed: ReviewGuide = serde_json::from_str(
            r#"{"version":2,"diff":"sha256:x","stories":[{"title":"Retry","stops":[
                {"id":"a","claim":"c","path":"p.rs","start_line":3,"end_line":4}]}]}"#,
        )
        .unwrap();
        assert_eq!(parsed.stories[0].stops[0].side, Side::New);
        assert_eq!(parsed.provenance.author, Author::Reviewer);
        assert!(parsed.validate().is_ok());

        let repeated = guide(vec![stop("a"), stop("a")]);
        assert!(repeated.validate().unwrap_err().contains("repeated"));
    }

    #[test]
    fn risky_untested_and_doubted_stops_need_an_explicit_verdict() {
        let plain = stop("a");
        assert!(!plain.needs_explicit_verdict());
        let mut untested = stop("b");
        untested.untested = true;
        let mut doubted = stop("c");
        doubted.audit = Audit::Doubtful {
            note: "the key is regenerated".into(),
        };
        let mut risky = stop("d");
        risky.risk = Some(Risk {
            kind: RiskKind::TestChanged,
            reason: "assertion removed".into(),
        });
        for stop in [untested, doubted, risky] {
            assert!(stop.needs_explicit_verdict(), "{}", stop.id);
        }
    }

    #[test]
    fn silent_preparation_reads_as_failed_but_recent_progress_does_not() {
        let mut state = PrepState::started("reading the diff", None, None);
        let now = state.heartbeat_at;
        assert_eq!(state.effective_status(now), PrepStatus::Preparing);
        let later = now + Duration::seconds(STALL_AFTER_SECS + 1);
        assert_eq!(state.effective_status(later), PrepStatus::Failed);
        state.heartbeat_at = later;
        assert_eq!(state.effective_status(later), PrepStatus::Preparing);
        state.status = PrepStatus::Ready;
        assert_eq!(
            state.effective_status(later + Duration::days(2)),
            PrepStatus::Ready
        );
    }

    #[test]
    fn store_round_trips_each_file_and_keys_do_not_collide_on_field_boundaries() {
        let base = tempfile::tempdir().unwrap();
        let key = ReviewKey::for_pull_request("gh", "github.com", "octo", "demo", 12);
        let store = GuideStore::in_dir(base.path(), &key);
        assert_eq!(store.guide().unwrap(), None);
        let guide = guide(vec![stop("a")]);
        store.save_guide(&guide).unwrap();
        assert_eq!(store.guide().unwrap(), Some(guide));
        let mut progress = Progress::default();
        progress.current = Some("a".into());
        store.save_progress(&progress).unwrap();
        assert_eq!(store.progress().unwrap(), Some(progress));

        assert_ne!(
            ReviewKey::for_pull_request("gh", "h", "ab", "c", 1),
            ReviewKey::for_pull_request("gh", "h", "a", "bc", 1)
        );
    }
}
