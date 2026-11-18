use super::{App, DiffSource, PullRequestDiffSource};
use crate::model::{LineSide, SessionDiffSource};
use crate::review_agent::{LocalTarget, SnapshotRequest, is_object_id};
use crate::vcs::{RevisionDiffTarget, traits::VcsType};

impl App {
    /// Capture the applied review side without reading files or spawning commands.
    pub fn snapshot_request(&self, side: LineSide) -> SnapshotRequest {
        if let DiffSource::PullRequest(pr) = &self.diff_source {
            return self.pr_snapshot_request(pr, side);
        }

        let source = self
            .applied_local_subset
            .as_ref()
            .unwrap_or(&self.diff_source);
        let vcs = if self.is_pristine_mode {
            VcsType::Git
        } else {
            self.vcs_info.vcs_type
        };
        let target = match source {
            DiffSource::CommitRange(ids) => {
                let explicit = self.revision_range.as_ref().filter(|range| {
                    vcs == VcsType::Git && range.commit_ids.as_ref() == ids.as_slice()
                });
                match explicit.map(|range| &range.diff_target) {
                    Some(RevisionDiffTarget::Explicit { base, head }) => match side {
                        LineSide::Old => revision(base.as_deref()),
                        LineSide::New => revision(Some(head)),
                    },
                    _ => commit_list_target(ids, side),
                }
            }
            DiffSource::Staged if vcs == VcsType::Git && side == LineSide::New => {
                LocalTarget::Index
            }
            DiffSource::Unstaged if vcs == VcsType::Git && side == LineSide::Old => {
                LocalTarget::Index
            }
            _ if side == LineSide::New => LocalTarget::Worktree,
            DiffSource::StagedUnstagedAndCommits(ids) if !ids.is_empty() => {
                commit_list_target(ids, side)
            }
            _ if matches!(vcs, VcsType::Git | VcsType::Mercurial)
                && is_object_id(&self.vcs_info.head_commit) =>
            {
                LocalTarget::RecordedHead(self.vcs_info.head_commit.clone())
            }
            _ if vcs == VcsType::Jujutsu => {
                LocalTarget::JjRecordedParent(self.vcs_info.head_commit.clone())
            }
            _ => LocalTarget::Unavailable,
        };
        let single_file = (self.session.diff_source == SessionDiffSource::File)
            .then(|| {
                self.diff_files
                    .first()
                    .map(|file| file.display_path().clone())
            })
            .flatten();
        let alternate_index = std::env::var_os("GIT_INDEX_FILE").is_some();
        let anchors = if alternate_index && target == LocalTarget::Worktree {
            self.diff_files
                .iter()
                .filter_map(|file| file.new_path.clone())
                .collect()
        } else {
            Vec::new()
        };
        SnapshotRequest::local(
            self.vcs_info.root_path.clone(),
            vcs,
            target,
            single_file,
            alternate_index,
            anchors,
        )
    }

    fn pr_snapshot_request(&self, pr: &PullRequestDiffSource, side: LineSide) -> SnapshotRequest {
        let (base, head, base_confirmed) = match &self.applied_pr_range {
            Some(range) => (&range.start_sha, &range.end_sha, range.start_sha_confirmed),
            None => (
                &pr.base_sha,
                &pr.key.head_sha,
                self.pr_cumulative_base_confirmed(pr),
            ),
        };
        let sha = match side {
            LineSide::Old => base_confirmed.then_some(base),
            LineSide::New => Some(head),
        };
        SnapshotRequest::pull_request(
            self.local_repo_root.clone(),
            pr.key.repository.clone(),
            sha.cloned(),
        )
    }
}

fn revision(id: Option<&str>) -> LocalTarget {
    id.filter(|id| is_object_id(id))
        .map_or(LocalTarget::Unavailable, |id| {
            LocalTarget::Revision(id.to_owned())
        })
}

fn commit_list_target(ids: &[String], side: LineSide) -> LocalTarget {
    match side {
        LineSide::New => revision(ids.last().map(String::as_str)),
        LineSide::Old => ids
            .first()
            .filter(|id| is_object_id(id))
            .map_or(LocalTarget::Unavailable, |id| {
                LocalTarget::ParentOf(id.clone())
            }),
    }
}
