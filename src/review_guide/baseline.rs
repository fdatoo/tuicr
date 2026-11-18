//! Local, immutable new-side contents from the first opening of a review.
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::{Comment, DiffFile, FileStatus, LineSide, ReviewSession};
use crate::review_agent::SnapshotRequest;

use super::{Audit, GuideStore, Risk, RiskKind, Side, Stop, Story, create_private_dir};

pub(crate) const TITLE: &str = "Changed since you started";
pub(crate) const STOP_PREFIX: &str = "local-change:";

pub(crate) fn is_change_story(story: &Story) -> bool {
    story.title == TITLE
        && story
            .stops
            .iter()
            .all(|stop| stop.id.starts_with(STOP_PREFIX))
}

const MAX_BASELINE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 5_000;
// Not `index.json`: that name belongs to the review-session manifest.
const MANIFEST: &str = "baseline.json";
const COMMENT_DISTANCE: u32 = 3;

#[derive(Serialize, Deserialize)]
pub(crate) struct BoundGuide {
    pub guide: super::ReviewGuide,
    pub blocks: Vec<Vec<String>>,
}

impl BoundGuide {
    pub(crate) fn load(store: &GuideStore) -> io::Result<Option<Self>> {
        store.read("local-guide-bindings.json")
    }

    pub(crate) fn save(
        store: &GuideStore,
        guide: &super::ReviewGuide,
        blocks: &[&[String]],
    ) -> io::Result<()> {
        #[derive(Serialize)]
        struct View<'a> {
            guide: &'a super::ReviewGuide,
            blocks: &'a [&'a [String]],
        }
        store.write("local-guide-bindings.json", &View { guide, blocks })
    }
}

#[derive(Serialize, Deserialize)]
struct Index {
    digest: String,
    paths: BTreeMap<String, Option<u32>>,
    truncated: bool,
}

pub(crate) struct Baseline {
    dir: PathBuf,
    index: Index,
}

impl Baseline {
    pub(crate) fn open(store: &GuideStore) -> io::Result<Option<Self>> {
        let dir = store.dir().join("baseline");
        let manifest = match fs::File::open(dir.join(MANIFEST)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let index = serde_json::from_reader(manifest.take(super::MAX_FILE_BYTES))?;
        Ok(Some(Self { dir, index }))
    }

    pub(crate) fn open_or_capture(
        store: &GuideStore,
        digest: &str,
        files: &[DiffFile],
        source: &SnapshotRequest,
        snapshot_dir: Option<&Path>,
    ) -> io::Result<Self> {
        if let Some(baseline) = Self::open(store)? {
            return Ok(baseline);
        }
        create_private_dir(store.dir())?;
        let stage = tempfile::tempdir_in(store.dir())?;
        let mut index = Index {
            digest: digest.into(),
            paths: BTreeMap::new(),
            truncated: false,
        };
        let mut remaining = MAX_BASELINE_BYTES;
        let read = source.local_text();
        for file in files.iter().filter(|file| !file.is_commit_message) {
            if index.paths.len() == MAX_ENTRIES {
                index.truncated = true;
                break;
            }
            let Some(path) = file.display_path().to_str() else {
                continue;
            };
            let slot = index.paths.len() as u32;
            let Some((text, cached)) = capture_file(file, snapshot_dir, &read) else {
                index.paths.insert(path.into(), None);
                continue;
            };
            if text.len() as u64 > remaining {
                index.paths.insert(path.into(), None);
                continue;
            }
            let dest = stage.path().join(slot.to_string());
            // The Runner's file is already owner-only and read-only. Retain a
            // link to that inode, even if the snapshot cache later expires.
            if !cached.is_some_and(|cached| fs::hard_link(cached, &dest).is_ok()) {
                let mut data = tempfile::NamedTempFile::new_in(stage.path())?;
                data.write_all(text.as_bytes())?;
                data.persist(&dest).map_err(|error| error.error)?;
            }
            remaining -= text.len() as u64;
            index.paths.insert(path.into(), Some(slot));
        }
        let mut manifest = tempfile::NamedTempFile::new_in(stage.path())?;
        serde_json::to_writer(&mut manifest, &index)?;
        if manifest.as_file().metadata()?.len() > super::MAX_FILE_BYTES {
            return Err(io::Error::other(
                "Review baseline index exceeds its size limit",
            ));
        }
        manifest
            .persist(stage.path().join(MANIFEST))
            .map_err(|error| error.error)?;
        let dir = store.dir().join("baseline");
        if let Err(error) = fs::rename(stage.path(), &dir) {
            return Self::open(store)?.ok_or(error);
        }
        Ok(Self { dir, index })
    }

    fn file_text(&self, slot: u32) -> io::Result<String> {
        let mut value = String::new();
        fs::File::open(self.dir.join(slot.to_string()))?
            .take(MAX_BASELINE_BYTES + 1)
            .read_to_string(&mut value)?;
        if value.len() as u64 > MAX_BASELINE_BYTES {
            return Err(io::Error::other(
                "Review baseline file exceeds its size limit",
            ));
        }
        Ok(value)
    }

    pub(crate) fn story(
        &self,
        digest: &str,
        files: &[DiffFile],
        session: &ReviewSession,
        old: &SnapshotRequest,
        new: &SnapshotRequest,
        visible: impl Fn(&Comment) -> bool,
    ) -> io::Result<Option<Story>> {
        if self.index.digest == digest {
            return Ok(None);
        }
        let old = old.local_text();
        let new = new.local_text();
        let mut stops = Vec::new();
        let mut notices = Vec::new();
        let displayed: std::collections::BTreeSet<_> = files
            .iter()
            .filter_map(|file| file.display_path().to_str())
            .collect();
        for (path, slot) in &self.index.paths {
            if !displayed.contains(path.as_str())
                && let Some(slot) = slot
                && let Some(after) = new(Path::new(path))
                && self.file_text(*slot)? != after
            {
                notices.push(format!("{path}: no longer in diff"));
            }
        }
        for file in files
            .iter()
            .filter(|file| !file.is_commit_message && !file.is_binary && !file.is_too_large)
        {
            let Some(path) = file.display_path().to_str() else {
                continue;
            };
            let before = match self.index.paths.get(path) {
                Some(Some(slot)) => self.file_text(*slot)?,
                Some(None) => continue,
                None if self.index.truncated => continue,
                None => {
                    let Some(text) = file
                        .old_path
                        .as_deref()
                        .map_or_else(|| Some(String::new()), &old)
                    else {
                        continue;
                    };
                    text
                }
            };
            let Some((after, _)) = capture_file(file, None, &new) else {
                continue;
            };
            let new_len = after.lines().count() as u32;
            for region in regions(&before, &after)? {
                let (anchors, partial) = region_anchors(file, &region, new_len);
                if partial {
                    notices.push(format!(
                        "{path}:{}: no longer in diff",
                        region.new_start.max(1)
                    ));
                }
                for (side, start, end) in anchors {
                    let claim = nearby_comment(session, file, &region, &visible).map_or_else(
                        || TITLE.into(),
                        |(line, comment)| {
                            format!(
                                "Changed after your comment on line {line}: “{}”",
                                comment.content.lines().next().unwrap_or_default()
                            )
                        },
                    );
                    let id = format!(
                        "{STOP_PREFIX}{:x}",
                        Sha256::digest(format!(
                            "{path:?}:{side:?}:{}:{}:{}",
                            region.old_start,
                            region.old_count,
                            start - region.new_start.min(start)
                        ))
                    );
                    let explanation = if region.new_count == 0 {
                        "Lines were removed here after you opened this review. Check the surrounding code and verify the removal.".into()
                    } else {
                        format!(
                            "Lines {}–{} differ from the new side you first opened. Check the updated code before recording a verdict.",
                            region.new_start,
                            region.new_start + region.new_count - 1
                        )
                    };
                    stops.push(Stop {
                        id,
                        claim,
                        explanation,
                        path: path.into(),
                        side,
                        start_line: start,
                        end_line: end,
                        symbol: None,
                        test: None,
                        untested: false,
                        risk: Some(Risk {
                            kind: RiskKind::Other,
                            reason: "Changed after the review started".into(),
                        }),
                        audit: Audit::Unchecked,
                        followups: Vec::new(),
                    });
                }
            }
        }
        notices.sort_unstable();
        notices.dedup();
        Ok((!stops.is_empty() || !notices.is_empty()).then(|| Story {
            title: TITLE.into(),
            summary: notices.join("\n"),
            stops,
        }))
    }
}

fn capture_file(
    file: &DiffFile,
    snapshot_dir: Option<&Path>,
    read: &impl Fn(&Path) -> Option<String>,
) -> Option<(String, Option<PathBuf>)> {
    if file.is_binary || file.is_too_large {
        return None;
    }
    let Some(path) = &file.new_path else {
        return Some((String::new(), None));
    };
    let verified = |text: &str| {
        // Reject racing worktree reads or a snapshot that didn't contain this
        // applied version of the file. Check added lines rather than context,
        // where an ignore-whitespace diff may deliberately show old text.
        let mut lines = text.lines();
        let mut previous = 0;
        file.hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .filter(|line| line.origin == crate::model::LineOrigin::Addition)
            .all(|line| {
                line.new_lineno
                    .and_then(|n| {
                        let actual = lines.nth(n.checked_sub(previous + 1)? as usize)?;
                        previous = n;
                        Some(actual == line.raw_content.as_deref().unwrap_or(&line.content))
                    })
                    .unwrap_or(false)
            })
    };
    if let Some(dir) = snapshot_dir {
        let cached = dir.join(path);
        if let Ok(file) = fs::File::open(&cached) {
            let mut text = String::new();
            if file
                .take(MAX_BASELINE_BYTES + 1)
                .read_to_string(&mut text)
                .is_ok()
                && text.len() as u64 <= MAX_BASELINE_BYTES
                && verified(&text)
            {
                return Some((text, Some(cached)));
            }
        }
    }
    let text = read(path)?;
    (verified(&text) || file.status == FileStatus::Deleted).then_some((text, None))
}

#[derive(Debug, PartialEq, Eq)]
struct Region {
    old_start: u32,
    old_count: u32,
    new_start: u32,
    new_count: u32,
}

fn regions(before: &str, after: &str) -> io::Result<Vec<Region>> {
    if before == after {
        return Ok(Vec::new());
    }
    let mut opts = git2::DiffOptions::new();
    opts.context_lines(0).interhunk_lines(0);
    let patch = git2::Patch::from_buffers(
        before.as_bytes(),
        None,
        after.as_bytes(),
        None,
        Some(&mut opts),
    )
    .map_err(io::Error::other)?;
    (0..patch.num_hunks())
        .map(|index| {
            let (hunk, _) = patch.hunk(index).map_err(io::Error::other)?;
            Ok(Region {
                old_start: hunk.old_start(),
                old_count: hunk.old_lines(),
                new_start: hunk.new_start(),
                new_count: hunk.new_lines(),
            })
        })
        .collect()
}

fn region_anchors(file: &DiffFile, region: &Region, new_len: u32) -> (Vec<(Side, u32, u32)>, bool) {
    let (side, start, end) = if new_len == 0 {
        (
            Side::Old,
            region.old_start.max(1),
            region
                .old_start
                .saturating_add(region.old_count.saturating_sub(1))
                .max(1),
        )
    } else if region.new_count == 0 {
        let adjacent = region.new_start.saturating_add(1).min(new_len);
        (Side::New, adjacent, adjacent)
    } else {
        (
            Side::New,
            region.new_start,
            region.new_start + region.new_count - 1,
        )
    };
    let mut ranges = Vec::new();
    let mut covered = 0;
    for hunk in &file.hunks {
        let (at, count) = match side {
            Side::Old => (hunk.old_start, hunk.old_count),
            Side::New => (hunk.new_start, hunk.new_count),
        };
        if count == 0 {
            continue;
        }
        let last = at + count - 1;
        if at <= end && start <= last {
            let first = start.max(at);
            let last = end.min(last);
            covered += last - first + 1;
            ranges.push((side, first, last));
        }
    }
    (ranges, covered < end - start + 1)
}

fn nearby_comment<'a>(
    session: &'a ReviewSession,
    file: &DiffFile,
    region: &Region,
    visible: &impl Fn(&Comment) -> bool,
) -> Option<(u32, &'a Comment)> {
    let review = session.files.get(file.display_path())?;
    review
        .line_comments
        .iter()
        .flat_map(|(&end, comments)| {
            comments
                .iter()
                .filter(|comment| comment.side != Some(LineSide::Old) && visible(comment))
                .filter_map(move |comment| {
                    let start = comment.line_range.map_or(end, |range| range.start);
                    let first = region.new_start.max(1);
                    let last = first + region.new_count.saturating_sub(1);
                    let distance = first.saturating_sub(end).max(start.saturating_sub(last));
                    (distance <= COMMENT_DISTANCE).then_some((distance, start, comment))
                })
        })
        .min_by_key(|(distance, line, comment)| (*distance, *line, &comment.id))
        .map(|(_, line, comment)| (line, comment))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baseline_regions_separate_additions_from_replacements_and_deletions() {
        let actual = regions(
            "first\na\nmiddle\nb\nlast\nend\n",
            "insert\nfirst\na\nmiddle\nreplacement\nlast\n",
        )
        .unwrap();
        assert_eq!(
            actual,
            [
                Region {
                    old_start: 0,
                    old_count: 0,
                    new_start: 1,
                    new_count: 1
                },
                Region {
                    old_start: 4,
                    old_count: 1,
                    new_start: 5,
                    new_count: 1
                },
                Region {
                    old_start: 6,
                    old_count: 1,
                    new_start: 6,
                    new_count: 0
                },
            ]
        );
    }

    #[test]
    fn baseline_story_places_explicit_stops_and_quotes_only_nearby_comments() {
        use crate::model::{CommentType, FilePatch, SessionDiffSource};
        use crate::review_agent::LocalTarget;
        use crate::vcs::traits::VcsType;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let source = SnapshotRequest::local(
            repo.clone(),
            VcsType::Git,
            LocalTarget::Worktree,
            None,
            false,
            Vec::new(),
        );
        let store = GuideStore::in_dir(
            dir.path(),
            &super::super::ReviewKey::for_local(&repo, "worktree"),
        );
        let parse = |line: &str| {
            crate::vcs::diff_parser::parse_file_patches(
                ["api.txt", "out.txt"]
                    .map(|path| {
                        FilePatch::new(
                            Some(path.into()),
                            Some(path.into()),
                            FileStatus::Modified,
                            format!("@@ -1,2 +1,2 @@\n context\n-original\n+{line}\n"),
                        )
                    })
                    .to_vec(),
            )
            .unwrap()
        };
        for path in ["api.txt", "out.txt"] {
            fs::write(repo.join(path), "context\nbefore\n").unwrap();
        }
        let baseline =
            Baseline::open_or_capture(&store, "initial", &parse("before"), &source, None).unwrap();

        let current = parse("response");
        let mut session = ReviewSession::new(
            repo.clone(),
            "head".into(),
            None,
            SessionDiffSource::WorkingTree,
        );
        for file in &current {
            fs::write(repo.join(file.display_path()), "context\nresponse\n").unwrap();
            session.add_diff_file(file);
        }
        session
            .files
            .get_mut(Path::new("api.txt"))
            .unwrap()
            .add_line_comment(
                1,
                Comment::new(
                    "Check the result\nDetails follow".into(),
                    CommentType::None,
                    Some(LineSide::New),
                ),
            );
        let story = baseline
            .story("changed", &current, &session, &source, &source, |_| true)
            .unwrap()
            .unwrap();
        assert_eq!(story.title, TITLE);
        assert_eq!(story.stops.len(), 2);
        let api = story
            .stops
            .iter()
            .find(|stop| stop.path == "api.txt")
            .unwrap();
        let out = story
            .stops
            .iter()
            .find(|stop| stop.path == "out.txt")
            .unwrap();
        assert_eq!(
            api.claim,
            "Changed after your comment on line 1: “Check the result”"
        );
        assert_eq!(out.claim, TITLE);
        assert!(story.stops.iter().all(|stop| stop.start_line == 2
            && stop.end_line == 2
            && stop.needs_explicit_verdict()));
    }

    #[test]
    fn baseline_story_reports_absent_files_and_gaps_without_unrelated_stops() {
        use crate::model::{FilePatch, SessionDiffSource};
        use crate::review_agent::LocalTarget;
        use crate::vcs::traits::VcsType;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let source = SnapshotRequest::local(
            repo.clone(),
            VcsType::Git,
            LocalTarget::Worktree,
            None,
            false,
            Vec::new(),
        );
        let store = GuideStore::in_dir(
            dir.path(),
            &super::super::ReviewKey::for_local(&repo, "worktree"),
        );
        let parse = |paths: &[&str], patch: &str| {
            crate::vcs::diff_parser::parse_file_patches(
                paths
                    .iter()
                    .map(|&path| {
                        FilePatch::new(
                            Some(path.into()),
                            Some(path.into()),
                            FileStatus::Modified,
                            patch,
                        )
                    })
                    .collect(),
            )
            .unwrap()
        };
        for path in ["gap.txt", "gone.txt"] {
            fs::write(repo.join(path), "amended\nkeep\nchanged\n").unwrap();
        }
        let initial = parse(
            &["gap.txt", "gone.txt"],
            "@@ -1 +1 @@\n-original\n+amended\n",
        );
        let baseline =
            Baseline::open_or_capture(&store, "initial", &initial, &source, None).unwrap();
        fs::write(repo.join("gap.txt"), "original\nkeep\nchanged\n").unwrap();
        fs::write(repo.join("gone.txt"), "original\nkeep\nchanged\n").unwrap();
        let current = parse(&["gap.txt"], "@@ -3 +3 @@\n-base\n+changed\n");
        let session = ReviewSession::new(repo, "head".into(), None, SessionDiffSource::WorkingTree);
        let story = baseline
            .story("changed", &current, &session, &source, &source, |_| true)
            .unwrap()
            .unwrap();
        assert!(story.stops.is_empty(), "{:?}", story.stops);
        assert_eq!(
            story.summary,
            "gap.txt:1: no longer in diff\ngone.txt: no longer in diff"
        );
    }
}
