use super::{App, DiffSource, InputMode, PrRangeReloadEvent, Result, TuicrError};
use crate::model::SessionDiffSource;
use crate::prepare;
use crate::review_guide::PrepTarget;
use crate::vcs::RevisionDiffTarget;

impl App {
    pub(crate) fn preparation_target(&self) -> PrepTarget {
        let source = self
            .applied_local_subset
            .as_ref()
            .unwrap_or(&self.diff_source);
        let (source, commits) = match source {
            DiffSource::PullRequest(pr) => {
                return PrepTarget::PullRequest {
                    repository: pr.key.repository.clone(),
                    number: pr.key.number,
                    head_sha: pr.key.head_sha.clone(),
                };
            }
            DiffSource::WorkingTree => (SessionDiffSource::WorkingTree, &[][..]),
            DiffSource::Staged => (SessionDiffSource::Staged, &[][..]),
            DiffSource::Unstaged => (SessionDiffSource::Unstaged, &[][..]),
            DiffSource::StagedAndUnstaged => (SessionDiffSource::StagedAndUnstaged, &[][..]),
            DiffSource::CommitRange(commits) => {
                (SessionDiffSource::CommitRange, commits.as_slice())
            }
            DiffSource::StagedUnstagedAndCommits(commits) => (
                SessionDiffSource::StagedUnstagedAndCommits,
                commits.as_slice(),
            ),
        };
        let revision = self.revision_range.as_ref().and_then(|range| {
            if self.applied_local_subset.is_some() {
                return None;
            }
            match &range.diff_target {
                RevisionDiffTarget::Explicit { base, head }
                    if range.commit_ids.as_ref() == commits =>
                {
                    Some((base.clone(), head.clone()))
                }
                _ => None,
            }
        });
        PrepTarget::Local {
            repo_root: self.vcs_info.root_path.clone(),
            source,
            commits: commits.to_vec(),
            path_filter: self.path_filter.clone(),
            revision,
        }
    }

    pub fn start_preparation(&mut self) {
        if !matches!(self.diff_source, DiffSource::PullRequest(_))
            && self.vcs_info.vcs_type == crate::vcs::traits::VcsType::File
        {
            self.set_warning("Review guides require a diff, not an annotation view");
            return;
        }
        if !self.preparation_review_loaded() {
            self.set_warning("Wait for the selected diff to load before preparing a review guide");
            return;
        }
        let Some(key) = self.review_key() else {
            self.set_warning("Open a diff before preparing a review guide");
            return;
        };
        let started = chrono::Utc::now();
        match prepare::start_open_review(self, &key, prepare::StartMode::Manual) {
            Ok(message) => {
                self.watch_preparation(key, started);
                self.set_message(message);
            }
            Err(error) => self.set_error(format!("Could not prepare the review guide: {error}")),
        }
    }

    pub fn cancel_preparation(&mut self) {
        self.prepare_on_initial_range = None;
        let Some(key) = self.review_key() else {
            self.set_warning("Open the review whose preparation you want to cancel");
            return;
        };
        match prepare::cancel(&key) {
            Ok(()) => self.set_message("Review preparation cancelled"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.set_warning(error.to_string())
            }
            Err(error) => self.set_error(format!("Could not cancel review preparation: {error}")),
        }
    }

    /// Automatic preparation happens on entry to PR mode, never as a diff
    /// watch side effect. A strict range must successfully finish opening first.
    pub fn maybe_prepare_on_open(&mut self) {
        if !matches!(self.diff_source, DiffSource::PullRequest(_))
            || !crate::review_agent::Settings::load().is_ok_and(|settings| settings.prepare_on_open)
        {
            return;
        }
        if let Some(request) = &self.pr_range_reload_state {
            self.prepare_on_initial_range = Some(request.range);
        } else if self.preparation_review_loaded()
            && let Some(key) = self.review_key()
        {
            let started = chrono::Utc::now();
            match prepare::start_open_review(self, &key, prepare::StartMode::Automatic) {
                Ok(_) => self.watch_preparation(key, started),
                Err(error) => {
                    self.set_warning(format!("Could not start review preparation: {error}"));
                }
            }
        }
    }

    pub(super) fn finish_prepare_on_open(&mut self, range: (usize, usize)) {
        if self.prepare_on_initial_range.take() == Some(range) {
            self.maybe_prepare_on_open();
        }
    }

    fn preparation_review_loaded(&self) -> bool {
        if self.input_mode == InputMode::CommitSelect
            || (self.input_mode == InputMode::Command
                && self.command_return_mode == InputMode::CommitSelect)
            || self.pr_open_state.is_some()
            || self.pr_reload_state.is_some()
            || self.pr_range_reload_state.is_some()
            || self.diff_files.is_empty()
        {
            return false;
        }
        if matches!(self.diff_source, DiffSource::PullRequest(_)) {
            if Self::is_strict_commit_selection(self.commit_selection_range, self.pr_commits.len())
            {
                return self.pr_range_sha_pair().is_some_and(|(start, end)| {
                    self.applied_pr_range
                        .as_ref()
                        .is_some_and(|range| range.start_sha == start && range.end_sha == end)
                });
            }
            return self.applied_pr_range.is_none();
        }
        true
    }

    /// Wait for and apply the exact initial PR range in headless commands.
    /// Unlike the TUI's polling path, any load failure is fatal: the cumulative
    /// placeholder cannot be substituted for the requested review.
    pub(crate) fn wait_for_preparation_range(&mut self) -> Result<()> {
        if !matches!(self.diff_source, DiffSource::PullRequest(_))
            || !Self::is_strict_commit_selection(self.commit_selection_range, self.pr_commits.len())
        {
            return Ok(());
        }
        let rx = self.pr_range_reload_rx.take().ok_or_else(|| {
            TuicrError::Forge("Could not start loading the selected PR commit range".into())
        })?;
        let PrRangeReloadEvent::Done { request, result } = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .map_err(|error| {
                TuicrError::Forge(format!(
                    "Could not load the selected PR commit range: {error}"
                ))
            })?;
        self.pr_range_reload_state = None;
        let patches = result.map_err(|error| {
            TuicrError::Forge(format!(
                "Could not load the selected PR commit range: {error}"
            ))
        })?;
        self.finish_pr_range_reload(&request, patches)
    }
}
