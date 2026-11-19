use std::path::Path;

use crate::model::{Comment, LineRange, LineSide, ReviewSession, SessionDiffSource};
use crate::review_agent::{LocalTarget, SourceRequest};
use crate::vcs::{detect_vcs_in, traits::VcsType};

use super::CommentTarget;

const NEARBY_LINES: u32 = 12;
const MIN_DISTANCE_LEAD: u32 = 8;

pub(crate) fn target_range(target: &CommentTarget) -> Option<(&Path, LineRange, LineSide)> {
    match target {
        CommentTarget::Line { path, line, side } => Some((path, LineRange::single(*line), *side)),
        CommentTarget::LineRange { path, range, side } => Some((path, *range, *side)),
        _ => None,
    }
}

pub(crate) fn capture(text: &str, range: LineRange) -> Vec<String> {
    let Some(start) = range.start.checked_sub(1) else {
        return Vec::new();
    };
    let count = (range.end - start) as usize;
    let lines: Vec<_> = text
        .lines()
        .skip(start as usize)
        .take(count)
        .map(str::to_owned)
        .collect();
    if lines.len() == count {
        lines
    } else {
        Vec::new()
    }
}

pub(crate) struct AnchorUpdates(Vec<AnchorUpdate>);

struct AnchorUpdate {
    path: std::path::PathBuf,
    id: String,
    from: u32,
    range: Option<LineRange>,
    stale: bool,
}

impl AnchorUpdates {
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn apply(&self, session: &mut ReviewSession) {
        for update in &self.0 {
            let Some(file) = session.files.get_mut(&update.path) else {
                continue;
            };
            let Some(comments) = file.line_comments.get_mut(&update.from) else {
                continue;
            };
            let Some(idx) = comments
                .iter()
                .position(|comment| comment.id == update.id && !comment.is_locked())
            else {
                continue;
            };
            comments[idx].stale = update.stale;
            if let Some(range) = update.range {
                let mut comment = comments.remove(idx);
                if comment.line_range.is_some() {
                    comment.line_range = Some(range);
                }
                if comments.is_empty() {
                    file.line_comments.remove(&update.from);
                }
                file.line_comments
                    .entry(range.end)
                    .or_default()
                    .push(comment);
            }
        }
    }
}

/// Resolve draft anchors without touching persisted review marks or comment
/// bodies. The caller can apply the same updates inside a storage transaction.
pub(crate) fn plan(
    session: &ReviewSession,
    visible: impl Fn(&Comment) -> bool,
    read: impl Fn(&Path, LineSide) -> Option<String>,
) -> AnchorUpdates {
    let mut updates = Vec::new();
    for (path, file) in &session.files {
        for side in [LineSide::Old, LineSide::New] {
            let matches = |comment: &Comment| {
                !comment.original_text.is_empty()
                    && !comment.is_locked()
                    && comment.side.unwrap_or_default() == side
                    && visible(comment)
            };
            if !file.line_comments.values().flatten().any(matches) {
                continue;
            }
            let Some(text) = read(path, side) else {
                continue;
            };
            let lines: Vec<_> = text.lines().collect();
            for (&end, comments) in &file.line_comments {
                for comment in comments.iter().filter(|comment| matches(comment)) {
                    let old = comment.line_range.unwrap_or_else(|| LineRange::single(end));
                    let target = locate(&lines, old.start, &comment.original_text);
                    let stale = target.is_none();
                    let range = target.filter(|start| *start != old.start).map(|start| {
                        LineRange::new(start, start + comment.original_text.len() as u32 - 1)
                    });
                    if comment.stale != stale || range.is_some() {
                        updates.push(AnchorUpdate {
                            path: path.clone(),
                            id: comment.id.clone(),
                            from: end,
                            range,
                            stale,
                        });
                    }
                }
            }
        }
    }
    // Keep ordering stable when several ranges relocate to the same line.
    updates.sort_unstable_by(|a, b| (&a.path, a.from, &a.id).cmp(&(&b.path, b.from, &b.id)));
    AnchorUpdates(updates)
}

fn locate(lines: &[&str], start: u32, original: &[String]) -> Option<u32> {
    let matches = |candidate: &[&str]| candidate.iter().zip(original).all(|(a, b)| *a == b);
    let index = start.checked_sub(1)? as usize;
    if lines
        .get(index..index.checked_add(original.len())?)
        .is_some_and(matches)
    {
        return Some(start);
    }
    let (mut nearest, mut runner_up) = ((u32::MAX, 0), u32::MAX);
    for (index, _) in lines
        .windows(original.len())
        .enumerate()
        .filter(|(_, window)| matches(window))
    {
        let line = index as u32 + 1;
        let distance = start.abs_diff(line);
        if distance < nearest.0 {
            runner_up = nearest.0;
            nearest = (distance, line);
        } else {
            runner_up = runner_up.min(distance);
        }
    }
    (nearest.1 != 0
        && (runner_up == u32::MAX
            || (nearest.0 <= NEARBY_LINES && runner_up - nearest.0 >= MIN_DISTANCE_LEAD)))
        .then_some(nearest.1)
}

pub(super) fn saved_source(session: &ReviewSession, side: LineSide) -> Option<SourceRequest> {
    if matches!(session.diff_source, SessionDiffSource::PullRequest) {
        return None;
    }
    let root = &session.repo_path;
    let vcs = detect_vcs_in(
        root,
        crate::vcs::git::GitBackendPreference::Libgit2,
        Default::default(),
    )
    .ok()?
    .info()
    .vcs_type;
    let commits = session.commit_range.as_deref().unwrap_or_default();
    let target = match (session.diff_source, side) {
        (SessionDiffSource::Staged, LineSide::New)
        | (SessionDiffSource::Unstaged, LineSide::Old)
            if vcs == VcsType::Git =>
        {
            LocalTarget::Index
        }
        (SessionDiffSource::CommitRange, LineSide::New) => {
            LocalTarget::Revision(commits.last()?.clone())
        }
        (
            SessionDiffSource::CommitRange
            | SessionDiffSource::WorkingTreeAndCommits
            | SessionDiffSource::StagedUnstagedAndCommits,
            LineSide::Old,
        ) if !commits.is_empty() => LocalTarget::ParentOf(commits.first()?.clone()),
        (_, LineSide::New) => LocalTarget::Worktree,
        (SessionDiffSource::File | SessionDiffSource::Pristine, LineSide::Old) => {
            LocalTarget::Worktree
        }
        (_, LineSide::Old) => LocalTarget::Revision(session.base_commit.clone()),
    };
    Some(SourceRequest::local(root.clone(), vcs, target, false))
}

pub(crate) fn reconcile_saved(session: &mut ReviewSession) {
    if !session.files.values().any(|file| {
        file.line_comments.values().flatten().any(|comment| {
            !comment.original_text.is_empty() && !comment.is_locked() && comment.commit_id.is_none()
        })
    }) {
        return;
    }
    let old = saved_source(session, LineSide::Old);
    let new = saved_source(session, LineSide::New);
    let old = old.as_ref().map(SourceRequest::local_text);
    let new = new.as_ref().map(SourceRequest::local_text);
    let updates = plan(
        session,
        |comment| comment.commit_id.is_none(),
        |path, side| match side {
            LineSide::Old => old.as_ref()?(path),
            LineSide::New => new.as_ref()?(path),
        },
    );
    updates.apply(session);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::comment::CommentLifecycleState;
    use crate::model::{CommentType, FileStatus};

    #[test]
    fn comment_anchor_moves_range_key_and_preserves_locked_or_ambiguous_matches() {
        let mut session = ReviewSession::new(
            ".".into(),
            "base".into(),
            None,
            SessionDiffSource::WorkingTree,
        );
        let path = std::path::PathBuf::from("code.txt");
        session.add_file(path.clone(), FileStatus::Modified, 0);
        let review = session.files.get_mut(&path).unwrap();
        let mut moved = Comment::new_with_range(
            "move".into(),
            CommentType::None,
            Some(LineSide::New),
            LineRange::new(1, 2),
        );
        moved.original_text = vec!["one".into(), "two".into()];
        review.add_line_comment(2, moved);
        for (line, text, locked) in [
            (5, "duplicate", false),
            (6, "absent", false),
            (1, "two", true),
        ] {
            let mut comment = Comment::new(text.into(), CommentType::None, Some(LineSide::New));
            comment.original_text.push(text.into());
            if locked {
                comment.lifecycle_state = CommentLifecycleState::Submitted;
            }
            review.add_line_comment(line, comment);
        }
        let current = "lead\none\ntwo\nduplicate\nmiddle\nduplicate\n";
        let updates = plan(&session, |_| true, |_, _| Some(current.into()));
        assert!(!updates.is_empty());
        updates.apply(&mut session);
        let comments = &session.files[&path].line_comments;
        assert_eq!(comments[&3][0].line_range, Some(LineRange::new(2, 3)));
        assert!(comments[&5][0].stale);
        assert!(comments[&6][0].stale);
        assert!(!comments[&1][0].stale);
        assert!(plan(&session, |_| true, |_, _| Some(current.into())).is_empty());
    }
}
