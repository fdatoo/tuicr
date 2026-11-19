use std::cell::OnceCell;

use sha2::{Digest, Sha256};

use super::*;
use crate::review_guide::baseline::{self, Baseline, BoundGuide};
use crate::review_guide::resolve::{ResolvedStop, block_text, relocate, resolve_stop};
use crate::review_guide::{
    Author, Basis, GUIDE_VERSION, GuideStore, PrepStatus, Progress, ReviewGuide, ReviewKey, Side,
    Stop, Story, Verdict, VerdictRecord,
};

#[path = "spot_check.rs"]
mod spot_check;

const LANDING_CONTEXT_ROWS: usize = 3;
const GUIDE_POLL_INTERVAL: Duration = Duration::from_millis(500);
const ACTIVITY_WINDOW: Duration = Duration::from_secs(60);
const EXPLICIT_VERDICT_HINT: &str = "Explicit verdict needed: K checks out · x wrong · p park";

#[derive(Default)]
pub(super) struct ReviewMap {
    pending: Option<ReviewGuide>,
    supplied_at: Option<chrono::DateTime<Utc>>,
    pending_wrong: Option<u64>,
    installed: Option<InstalledGuide>,
    publication: u64,
    pub(super) digest: OnceCell<String>,
    poll_at: Option<Instant>,
    rules_digest: Option<String>,
    pending_landing: bool,
    last_ready: Option<(ReviewKey, crate::review_guide::PrepState)>,
    map_area: Option<ratatui::layout::Rect>,
    spot_walk: Option<spot_check::SpotWalk>,
    map_hits: Vec<Option<MapEntry>>,
    local_publication: Option<u64>,
    changed_story: Option<Story>,
}

struct InstalledGuide {
    key: ReviewKey,
    store: GuideStore,
    guide: ReviewGuide,
    progress: Progress,
    spot_sample: Vec<crate::review_guide::HunkRef>,
    bindings: Vec<Binding>,
    reach: OnceCell<(usize, usize)>,
    publication: u64,
    changes_only: bool,
    active: bool,
    entry_cue: Option<GuideEntryCue>,
    show_map: bool,
    current: Option<usize>,
    selected: Option<MapEntry>,
    last_input: Option<Instant>,
    elapsed_at: Option<Instant>,
    elapsed_remainder: Duration,
}

#[derive(Clone, Copy)]
pub(crate) enum GuideEntryCue {
    Start,
    Resume,
}

pub(crate) struct GuideViewport {
    first: usize,
    last: usize,
    context_start: usize,
}

impl GuideViewport {
    pub(crate) fn retained_range(&self, height: usize) -> (usize, usize) {
        (self.context_start, self.first.saturating_add(height))
    }

    pub(crate) fn offset(&self, height: usize, row_height: impl Fn(usize) -> usize) -> usize {
        let mut available = height;
        for row in self.first..=self.last {
            let Some(remaining) = available.checked_sub(row_height(row)) else {
                return self.first;
            };
            available = remaining;
            if available == 0 {
                return self.first;
            }
        }
        let mut offset = self.first;
        for row in (self.context_start..self.first).rev() {
            let Some(remaining) = available.checked_sub(row_height(row)) else {
                break;
            };
            available = remaining;
            offset = row;
        }
        offset
    }
}

struct Binding {
    anchor: Option<ResolvedStop>,
    block: Vec<String>,
    changed: bool,
}

impl Binding {
    fn rebind(&mut self, stop: &Stop, files: &[DiffFile], record: Option<&VerdictRecord>) {
        self.anchor = relocate(&self.block, stop, files);
        if self.anchor.is_none() && record.is_some() {
            self.changed = true;
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MapEntry {
    Story(usize),
    Stop(usize),
    Group(usize),
}

pub(crate) enum MapRow<'a> {
    Story {
        title: &'a str,
        current: bool,
        total: usize,
        collapsed: Option<usize>,
    },
    Notice(&'a str),
    Stop {
        index: usize,
        stop: &'a Stop,
    },
    Supporting,
    Group(usize),
}

impl MapRow<'_> {
    pub fn entry(&self) -> Option<MapEntry> {
        match self {
            Self::Story {
                collapsed: Some(index),
                ..
            } => Some(MapEntry::Story(*index)),
            Self::Stop { index, .. } => Some(MapEntry::Stop(*index)),
            Self::Group(index) => Some(MapEntry::Group(*index)),
            _ => None,
        }
    }
}

impl InstalledGuide {
    fn stop(&self, index: usize) -> Option<&Stop> {
        self.guide.stops().nth(index).map(|(_, stop)| stop)
    }

    fn replace_changed_story(&mut self, story: Option<Story>, files: &[DiffFile]) -> bool {
        self.reach.take();
        let current_id = self
            .current
            .and_then(|index| self.stop(index))
            .map(|stop| stop.id.clone());
        let selected_id = match self.selected {
            Some(MapEntry::Stop(index)) => self.stop(index).map(|stop| stop.id.clone()),
            _ => None,
        };
        let has_old_story = self
            .guide
            .stories
            .first()
            .is_some_and(baseline::is_change_story);
        let old_count = if has_old_story {
            self.guide.stories.remove(0).stops.len()
        } else {
            0
        };
        let mut updated = false;
        let bindings = story
            .as_ref()
            .into_iter()
            .flat_map(|story| &story.stops)
            .map(|stop| {
                let anchor = resolve_stop(stop, files).ok();
                let block = anchor
                    .as_ref()
                    .map_or_else(Vec::new, |anchor| block_text(files, anchor));
                let changed = self
                    .progress
                    .verdicts
                    .get(&stop.id)
                    .is_some_and(|record| record.block != block);
                if changed {
                    self.progress.verdicts.remove(&stop.id);
                    updated = true;
                }
                Binding {
                    anchor,
                    block,
                    changed,
                }
            })
            .collect::<Vec<_>>();
        self.bindings.splice(..old_count, bindings);
        if let Some(story) = story {
            self.guide.stories.insert(0, story);
        }
        self.progress.verdicts.retain(|id, _| {
            let keep = !id.starts_with(baseline::STOP_PREFIX) || self.guide.stop(id).is_some();
            updated |= !keep;
            keep
        });
        let locate = |id: &str| self.guide.stops().position(|(_, stop)| stop.id == id);
        self.current = current_id.as_deref().and_then(locate);
        self.selected = selected_id
            .as_deref()
            .and_then(locate)
            .map(MapEntry::Stop)
            .or(self
                .selected
                .filter(|entry| matches!(entry, MapEntry::Group(_))));
        updated
    }
    fn supporting_open(&self) -> bool {
        self.guide
            .stops()
            .all(|(_, stop)| self.progress.verdicts.contains_key(&stop.id))
    }

    fn rows(&self) -> impl Iterator<Item = MapRow<'_>> {
        let mut index = 0;
        self.guide
            .stories
            .iter()
            .enumerate()
            .flat_map(move |(story_index, story)| {
                let start = index;
                index += story.stops.len();
                let notices = baseline::is_change_story(story).then(|| story.summary.lines());
                let current = self
                    .current
                    .is_some_and(|current| (start..index).contains(&current));
                // A finished story stays open while revisiting one of its
                // stops so the current stop never disappears from the map.
                let collapsed = (!current
                    && !story.stops.is_empty()
                    && story
                        .stops
                        .iter()
                        .all(|stop| self.progress.verdicts.contains_key(&stop.id)))
                .then_some(story_index);
                std::iter::once(MapRow::Story {
                    title: &story.title,
                    current,
                    total: story.stops.len(),
                    collapsed,
                })
                .chain(notices.into_iter().flatten().map(MapRow::Notice))
                .chain(
                    story
                        .stops
                        .iter()
                        .take(if collapsed.is_none() {
                            story.stops.len()
                        } else {
                            0
                        })
                        .enumerate()
                        .map(move |(offset, stop)| MapRow::Stop {
                            index: start + offset,
                            stop,
                        }),
                )
            })
            .chain((!self.changes_only).then_some(MapRow::Supporting))
            .chain(
                (0..if self.supporting_open() {
                    self.guide.supporting.len()
                } else {
                    0
                })
                    .map(MapRow::Group),
            )
    }

    fn tick(&mut self, now: Instant) -> bool {
        let old_minute = self.progress.elapsed_secs / 60;
        if let Some(input) = self.last_input {
            let until = now.min(input + ACTIVITY_WINDOW);
            let from = self.elapsed_at.replace(until).unwrap_or(input);
            self.elapsed_remainder += until.saturating_duration_since(from);
            self.progress.elapsed_secs = self
                .progress
                .elapsed_secs
                .saturating_add(self.elapsed_remainder.as_secs());
            self.elapsed_remainder =
                Duration::from_nanos(u64::from(self.elapsed_remainder.subsec_nanos()));
        }
        self.progress.elapsed_secs / 60 != old_minute
    }
}

impl App {
    pub fn review_key(&self) -> Option<ReviewKey> {
        if !self.guide_review_ready() {
            return None;
        }
        let source = self
            .applied_local_subset
            .as_ref()
            .unwrap_or(&self.diff_source);
        if let DiffSource::PullRequest(pr) = source {
            return Some(ReviewKey::for_repository(&pr.key.repository, pr.key.number));
        }
        let (kind, commits) = local_scope(source);
        let mut scope = Sha256::new();
        digest_field(&mut scope, kind.as_bytes());
        if let Some(path) = self.path_filter.as_deref() {
            digest_field(&mut scope, path.as_bytes());
        }
        self.digest_local_range(&mut scope, commits);
        Some(ReviewKey::for_local(
            &self.vcs_info.root_path,
            &format!("{:x}", scope.finalize()),
        ))
    }

    /// Digest the applied review, excluding synthetic commit-message files.
    pub fn review_digest(&self) -> String {
        self.cached_review_digest().to_owned()
    }

    pub(super) fn cached_review_digest(&self) -> &str {
        self.review_map
            .digest
            .get_or_init(|| self.compute_review_digest())
    }

    fn compute_review_digest(&self) -> String {
        let mut digest = Sha256::new();
        digest_field(&mut digest, b"tuicr-review-guide-v2");
        match self
            .applied_local_subset
            .as_ref()
            .unwrap_or(&self.diff_source)
        {
            DiffSource::PullRequest(pr) => {
                let repository = &pr.key.repository;
                let host = crate::slug::normalized_host(&repository.host);
                for field in [
                    "pr",
                    crate::slug::forge_code(repository.kind),
                    &host,
                    &repository.owner,
                    &repository.name,
                    &pr.base_sha,
                    &pr.key.head_sha,
                ] {
                    digest_field(&mut digest, field.as_bytes());
                }
                digest.update(pr.key.number.to_le_bytes());
                if let Some(range) = &self.applied_pr_range {
                    digest_field(&mut digest, range.start_sha.as_bytes());
                    digest_field(&mut digest, range.end_sha.as_bytes());
                }
            }
            source => {
                let (kind, commits) = local_scope(source);
                digest_field(&mut digest, kind.as_bytes());
                if !matches!(source, DiffSource::CommitRange(_)) {
                    digest_field(&mut digest, self.vcs_info.head_commit.as_bytes());
                }
                self.digest_local_range(&mut digest, commits);
            }
        }
        let files = self
            .diff_files
            .iter()
            .filter(|file| !file.is_commit_message);
        digest.update((files.clone().count() as u64).to_le_bytes());
        for file in files {
            for path in [file.old_path.as_deref(), file.new_path.as_deref()] {
                digest.update([u8::from(path.is_some())]);
                if let Some(path) = path {
                    digest_field(&mut digest, path.as_os_str().as_encoded_bytes());
                }
            }
            digest.update([
                file.status.as_char() as u8,
                u8::from(file.is_binary),
                u8::from(file.is_too_large),
            ]);
            digest.update((file.hunks.len() as u64).to_le_bytes());
            for hunk in &file.hunks {
                for coordinate in [
                    hunk.old_start,
                    hunk.old_count,
                    hunk.new_start,
                    hunk.new_count,
                ] {
                    digest.update(coordinate.to_le_bytes());
                }
                digest.update((hunk.lines.len() as u64).to_le_bytes());
                for line in &hunk.lines {
                    digest.update([line.origin as u8]);
                    digest_field(
                        &mut digest,
                        line.raw_content
                            .as_deref()
                            .unwrap_or(&line.content)
                            .as_bytes(),
                    );
                }
            }
        }
        format!("sha256:{:x}", digest.finalize())
    }

    fn digest_local_range(&self, digest: &mut Sha256, commits: &[String]) {
        digest.update((commits.len() as u64).to_le_bytes());
        for commit in commits {
            digest_field(digest, commit.as_bytes());
        }
        if let Some(range) = &self.revision_range
            && self.applied_local_subset.is_none()
            && range.commit_ids.as_ref() == commits
            && let RevisionDiffTarget::Explicit { base, head } = &range.diff_target
        {
            digest_field(digest, base.as_deref().unwrap_or_default().as_bytes());
            digest_field(digest, head.as_bytes());
        }
    }

    fn guide_review_ready(&self) -> bool {
        !self.showing_target_selector()
            && self.pr_open_state.is_none()
            && self.pr_reload_state.is_none()
            && self.pr_range_reload_state.is_none()
    }

    pub(super) fn guide_diff_published(&mut self) {
        self.stop_spot_check();
        self.review_map.publication = self.review_map.publication.wrapping_add(1);
        self.review_map.digest.take();
        self.review_map.pending_landing = false;
        self.review_map.poll_at = None;
    }

    pub fn queue_supplied_guide(&mut self, mut guide: ReviewGuide) {
        // A file passed with --guide has not been checked by anyone: ignore
        // whatever audit status it claims. Only `tuicr prepare` records an audit.
        guide.provenance.author = Author::Supplied;
        guide.provenance.audited = false;
        for stop in guide.stories.iter_mut().flat_map(|story| &mut story.stops) {
            stop.audit = crate::review_guide::Audit::Unchecked;
        }
        self.review_map.pending = Some(guide);
    }

    pub fn install_guide(&mut self, guide: ReviewGuide) -> Result<()> {
        self.install_map(guide, None, false)
    }

    fn install_map(
        &mut self,
        mut guide: ReviewGuide,
        saved_blocks: Option<Vec<Vec<String>>>,
        changes_only: bool,
    ) -> Result<()> {
        let key = self.review_key().ok_or_else(|| {
            TuicrError::InvalidInput("Open a loaded review before installing a review guide".into())
        })?;
        let stale = guide.diff != self.cached_review_digest();
        if stale && saved_blocks.is_none() {
            return Err(TuicrError::InvalidInput(
                "review guide is stale for this diff — use :prepare".into(),
            ));
        }
        if !changes_only {
            guide.validate().map_err(TuicrError::InvalidInput)?;
        }
        if guide.provenance.author == Author::Supplied && !stale && !changes_only {
            complete_supplied_guide(&mut guide, &self.diff_files);
        }
        self.stop_spot_check();
        let store = GuideStore::open(&key)?;
        let mut progress = store.progress()?.unwrap_or_default();
        let previous = self
            .review_map
            .installed
            .as_ref()
            .filter(|previous| previous.key == key);
        if let Some(previous) = previous {
            progress.elapsed_secs = progress.elapsed_secs.max(previous.progress.elapsed_secs);
        }
        let mut verdicts_changed = false;
        let mut saved_blocks = saved_blocks.map(Vec::into_iter);
        let bindings = guide
            .stops()
            .map(|(_, stop)| {
                if stale {
                    let block = saved_blocks
                        .as_mut()
                        .and_then(Iterator::next)
                        .unwrap_or_default();
                    let anchor = relocate(&block, stop, &self.diff_files);
                    let changed = anchor.is_none();
                    if changed
                        || progress
                            .verdicts
                            .get(&stop.id)
                            .is_some_and(|record| record.block != block)
                    {
                        verdicts_changed |= progress.verdicts.remove(&stop.id).is_some();
                    }
                    return Ok(Binding {
                        anchor,
                        block,
                        changed,
                    });
                }
                let exact =
                    resolve_stop(stop, &self.diff_files).map_err(TuicrError::InvalidInput)?;
                let mut exact_block = None;
                let (anchor, changed) = match progress.verdicts.get(&stop.id) {
                    Some(record) if previous.is_some() => {
                        let block = block_text(&self.diff_files, &exact);
                        let changed = block != record.block;
                        exact_block = Some(block);
                        if changed {
                            progress.verdicts.remove(&stop.id);
                            verdicts_changed = true;
                        }
                        (exact, changed)
                    }
                    Some(record) => match relocate(&record.block, stop, &self.diff_files) {
                        Some(anchor) => (anchor, false),
                        None => {
                            progress.verdicts.remove(&stop.id);
                            verdicts_changed = true;
                            (exact, true)
                        }
                    },
                    None => {
                        let changed = previous.is_some_and(|map| {
                            map.guide
                                .stops()
                                .zip(&map.bindings)
                                .any(|((_, prior), binding)| prior.id == stop.id && binding.changed)
                        });
                        (exact, changed)
                    }
                };
                let block = exact_block.unwrap_or_else(|| block_text(&self.diff_files, &anchor));
                Ok(Binding {
                    anchor: Some(anchor),
                    block,
                    changed,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        progress.verdicts.retain(|id, _| {
            let keep = guide.stop(id).is_some()
                || (!changes_only
                    && id.starts_with(baseline::STOP_PREFIX)
                    && self
                        .review_map
                        .changed_story
                        .as_ref()
                        .is_some_and(|story| story.stops.iter().any(|stop| &stop.id == id)));
            verdicts_changed |= !keep;
            keep
        });
        let dismissed_before = progress.dismissed_groups.len();
        progress.dismissed_groups.retain(|label| {
            guide.supporting.iter().any(|group| {
                group.basis == Basis::Mechanical
                    && &group.label == label
                    && previous.is_none_or(|previous| previous.guide.supporting.contains(group))
            })
        });
        verdicts_changed |= progress.dismissed_groups.len() != dismissed_before;
        let (spot_sample, spots_changed) = spot_check::prepare_spot_sample(&guide, &mut progress);
        verdicts_changed |= spots_changed;
        if previous.is_some() {
            let next = guide
                .stops()
                .find(|(_, stop)| !progress.verdicts.contains_key(&stop.id))
                .map(|(_, stop)| stop.id.clone());
            verdicts_changed |= progress.current != next;
            progress.current = next;
        }
        if verdicts_changed {
            store.save_progress(&progress)?;
        }
        let entry_cue = Some(
            if previous.is_some() || progress.current.is_some() || !progress.verdicts.is_empty() {
                GuideEntryCue::Resume
            } else {
                GuideEntryCue::Start
            },
        );
        let show_map = previous.is_none_or(|previous| previous.show_map);
        let elapsed_remainder = previous.map_or(Duration::ZERO, |map| map.elapsed_remainder);
        if !stale && !changes_only && !matches!(self.diff_source, DiffSource::PullRequest(_)) {
            let blocks: Vec<_> = bindings
                .iter()
                .map(|binding| binding.block.as_slice())
                .collect();
            if let Err(error) = BoundGuide::save(&store, &guide, &blocks) {
                self.set_warning(format!("Could not save local guide anchors: {error}"));
            }
        }
        self.review_map.installed = Some(InstalledGuide {
            key,
            store,
            guide,
            progress,
            bindings,
            reach: OnceCell::new(),
            changes_only,
            spot_sample,
            publication: self.review_map.publication,
            active: false,
            entry_cue,
            show_map,
            current: None,
            selected: None,
            last_input: None,
            elapsed_at: None,
            elapsed_remainder,
        });
        self.review_map.pending_landing = false;
        if !changes_only {
            self.refresh_changed_story();
        }
        Ok(())
    }

    pub fn poll_review_guide(&mut self) -> bool {
        let now = Instant::now();
        let mut redraw = self
            .review_map
            .installed
            .as_mut()
            .is_some_and(|map| map.tick(now));
        if !self.guide_review_ready() {
            return redraw;
        }
        if self
            .review_map
            .installed
            .as_ref()
            .is_some_and(|map| map.publication != self.review_map.publication)
        {
            self.reconcile_guide();
            redraw = true;
        }
        if self.review_map.local_publication != Some(self.review_map.publication) {
            self.review_map.local_publication = Some(self.review_map.publication);
            redraw |= self.update_local_second_pass();
        }
        if let Some(guide) = self.review_map.pending.take() {
            let result = if guide.diff != self.cached_review_digest() {
                self.restore_local_guide(Some(&guide)).and_then(|restored| {
                    if restored {
                        Ok(())
                    } else {
                        self.install_guide(guide)
                    }
                })
            } else {
                self.install_guide(guide)
            };
            match result {
                Ok(()) => {
                    self.review_map.supplied_at = Some(Utc::now());
                    self.announce_ready_guide();
                }
                Err(error) => self.set_error(format!("Review guide: {error}")),
            }
            return true;
        }
        if self.review_map.installed.is_none() && self.review_map.poll_at.is_none() {
            match self.restore_local_guide(None) {
                Ok(true) => {
                    self.announce_ready_guide();
                    redraw = true;
                }
                Ok(false) => {}
                Err(error) => {
                    self.set_warning(format!("Could not restore local guide: {error}"));
                    redraw = true;
                }
            }
        }
        if self.input_mode == InputMode::Comment
            || self.review_map.poll_at.is_some_and(|at| at > now)
        {
            return redraw;
        }
        self.review_map.poll_at = Some(now + GUIDE_POLL_INTERVAL);
        if self
            .installed_guide()
            .is_some_and(|guide| guide.provenance.prepared_at.is_some())
        {
            let before = self.installed_guide_has_older_rules();
            self.review_map.rules_digest = crate::prepare::current_rules_digest().ok();
            redraw |= before != self.installed_guide_has_older_rules();
        }
        let Some(key) = self.review_key() else {
            return redraw;
        };
        let Ok(store) = GuideStore::open(&key) else {
            return redraw;
        };
        let Ok(Some(state)) = store.state() else {
            return redraw;
        };
        let digest = self.review_digest();
        if state.status != PrepStatus::Ready
            || self
                .review_map
                .last_ready
                .as_ref()
                .is_some_and(|(last_key, last)| *last_key == key && *last == state)
            || state.diff.as_deref() != Some(digest.as_str())
        {
            return redraw;
        }
        // A launch file is chosen explicitly; a pre-existing stored guide
        // should not replace it. An audit that finishes later can.
        if self
            .review_map
            .supplied_at
            .is_some_and(|at| state.finished_at.is_none_or(|finished| finished < at))
        {
            return redraw;
        }
        let guide = match store.guide() {
            Ok(Some(guide)) if guide.diff == digest => guide,
            Ok(_) => return redraw,
            Err(error) => {
                self.review_map.last_ready = Some((key, state));
                self.set_error(format!("Review guide: {error}"));
                return true;
            }
        };
        self.review_map.last_ready = Some((key, state));
        if let Err(error) = self.install_guide(guide) {
            self.set_error(format!("Review guide: {error}"));
        } else {
            self.review_map.supplied_at = None;
            self.announce_ready_guide();
        }
        true
    }

    fn announce_ready_guide(&mut self) {
        if self.installed_guide_is_stale() {
            self.set_message("Guide prepared for an earlier version · :guide to review unchanged stops · :prepare to update");
        } else if self.installed_guide_has_older_rules() {
            self.set_message("Review guide prepared with older rules · :prepare to update");
        }
    }

    fn reconcile_guide(&mut self) {
        let key = self.review_key();
        let digest = self.review_digest();
        let Some(map) = self.review_map.installed.as_mut() else {
            return;
        };
        if Some(&map.key) != key.as_ref() {
            self.flush_guide_progress();
            self.review_map.installed = None;
            self.review_map.supplied_at = None;
            self.review_map.last_ready = None;
            return;
        }
        map.publication = self.review_map.publication;
        map.reach.take();
        let mut dropped = false;
        let diff_changed = digest != map.guide.diff;
        for ((_, stop), binding) in map.guide.stops().zip(&mut map.bindings) {
            if diff_changed || map.progress.verdicts.contains_key(&stop.id) {
                binding.rebind(stop, &self.diff_files, map.progress.verdicts.get(&stop.id));
            } else {
                binding.anchor = resolve_stop(stop, &self.diff_files).ok();
            }
            if binding.anchor.is_none() && map.progress.verdicts.remove(&stop.id).is_some() {
                dropped = true;
            }
        }
        let dismissed = diff_changed && !map.progress.dismissed_groups.is_empty();
        if dismissed {
            map.progress.dismissed_groups.clear();
        }
        if dropped {
            self.set_warning("Review guide: some stops changed since you checked");
        }
        if dropped || dismissed {
            self.flush_guide_progress();
        }
    }

    fn update_local_second_pass(&mut self) -> bool {
        self.review_map.changed_story = None;
        if matches!(self.diff_source, DiffSource::PullRequest(_)) {
            return self.refresh_changed_story();
        }
        let comments_changed = self.reconcile_local_comments();
        let Some(key) = self.review_key() else {
            return comments_changed;
        };
        let digest = self.review_digest();
        let commits = self.selected_commit_set();
        let old = self.source_request(LineSide::Old);
        let new = self.source_request(LineSide::New);
        let story = GuideStore::open(&key).and_then(|store| {
            Baseline::open_or_capture(&store, &digest, &self.diff_files, &new)?.story(
                &digest,
                &self.diff_files,
                &self.session,
                &old,
                &new,
                |comment| {
                    comment.author == self.username
                        && Self::comment_visible_with(comment, commits.as_ref())
                },
            )
        });
        match story {
            Ok(story) => self.review_map.changed_story = story,
            Err(error) => self.set_warning(format!("Could not compare review baseline: {error}")),
        }
        let stories_changed = self.refresh_changed_story();
        if self.installed_map().is_none()
            && self.review_map.changed_story.is_some()
            && self.input_mode == InputMode::Normal
        {
            self.set_message(format!("{} · :changes to review", baseline::TITLE));
        }
        comments_changed || stories_changed || self.review_map.changed_story.is_some()
    }

    fn refresh_changed_story(&mut self) -> bool {
        let Some(map) = self
            .review_map
            .installed
            .as_mut()
            .filter(|map| map.publication == self.review_map.publication)
        else {
            return false;
        };
        let story = self.review_map.changed_story.clone();
        let changed = story.is_some()
            || map
                .guide
                .stories
                .first()
                .is_some_and(baseline::is_change_story);
        if changed && map.replace_changed_story(story, &self.diff_files) {
            self.flush_guide_progress();
        }
        changed
    }

    fn restore_local_guide(&mut self, supplied: Option<&ReviewGuide>) -> Result<bool> {
        if matches!(self.diff_source, DiffSource::PullRequest(_)) {
            return Ok(false);
        }
        let Some(key) = self.review_key() else {
            return Ok(false);
        };
        let store = GuideStore::open(&key)?;
        let Some(saved) = BoundGuide::load(&store)? else {
            return Ok(false);
        };
        if saved.guide.diff == self.cached_review_digest()
            || supplied.is_some_and(|guide| {
                guide.diff != saved.guide.diff
                    || guide
                        .stops()
                        .any(|(_, stop)| saved.guide.stop(&stop.id).is_none())
            })
        {
            return Ok(false);
        }
        self.install_map(saved.guide, Some(saved.blocks), false)?;
        Ok(true)
    }

    pub(crate) fn enter_changed_story(&mut self) {
        if !self.guide_review_ready() || matches!(self.diff_source, DiffSource::PullRequest(_)) {
            self.set_message(":changes is available for local reviews");
            return;
        }
        let Some(story) = self.review_map.changed_story.as_ref() else {
            self.set_message("No reviewable text changed since you started");
            return;
        };
        let total = story.stops.len();
        if self.installed_map().is_none() {
            let guide = ReviewGuide {
                version: GUIDE_VERSION,
                diff: self.review_digest(),
                summary: String::new(),
                stories: vec![story.clone()],
                supporting: Vec::new(),
                estimate_minutes: None,
                provenance: Default::default(),
            };
            if let Err(error) = self.install_map(guide, None, true) {
                self.set_error(format!("Could not open changes: {error}"));
                return;
            }
        }
        self.stop_spot_check();
        if let Some(map) = self.review_map.installed.as_mut() {
            map.active = true;
            map.show_map = true;
            self.show_file_list = true;
            if total == 0 {
                map.current = None;
                map.selected = None;
            }
        }
        self.guide_input_activity();
        if total == 0 {
            self.set_message("These changes are no longer in the diff. See the first story.");
            return;
        }
        if self.active_map().is_some_and(|map| {
            (0..total).any(|index| {
                map.stop(index)
                    .is_some_and(|stop| !map.progress.verdicts.contains_key(&stop.id))
            })
        }) {
            self.walk_guide(0..total, true);
        } else {
            self.walk_guide(0..total, false);
        }
    }

    pub(crate) fn guide_keys_available(&self) -> bool {
        self.installed_map().is_some()
    }

    pub(crate) fn showing_changes_only(&self) -> bool {
        self.active_map().is_some_and(|map| map.changes_only)
    }

    pub fn active_guide(&self) -> Option<&ReviewGuide> {
        self.active_map().map(|map| &map.guide)
    }

    pub fn installed_guide(&self) -> Option<&ReviewGuide> {
        self.installed_map()
            .filter(|map| !map.changes_only)
            .map(|map| &map.guide)
    }

    pub(crate) fn guide_entry_cue(&self) -> Option<(&ReviewGuide, GuideEntryCue)> {
        let map = self
            .installed_map()
            .filter(|map| !map.active && !map.changes_only)?;
        Some((&map.guide, map.entry_cue?))
    }

    pub(crate) fn installed_guide_is_stale(&self) -> bool {
        self.installed_guide()
            .is_some_and(|guide| guide.diff != self.cached_review_digest())
    }

    pub(crate) fn installed_guide_has_older_rules(&self) -> bool {
        self.installed_guide().is_some_and(|guide| {
            guide.provenance.prepared_at.is_some()
                && self
                    .review_map
                    .rules_digest
                    .as_deref()
                    .is_some_and(|current| {
                        guide.provenance.rules_digest.as_deref() != Some(current)
                    })
        })
    }

    pub fn current_stop(&self) -> Option<&Stop> {
        let map = self.active_map()?;
        map.stop(map.current?)
    }

    pub fn guide_progress(&self) -> Option<&Progress> {
        self.installed_map().map(|map| &map.progress)
    }

    pub(crate) fn guide_reach(&self) -> Option<(usize, usize)> {
        let map = self.installed_map().filter(|map| !map.changes_only)?;
        Some(*map.reach.get_or_init(|| {
            self.coverage_record()
                .map_or((0, 0), |record| (record.guide_hunks, record.changed_hunks))
        }))
    }

    pub(crate) fn guide_resolved_stops(
        &self,
    ) -> impl Iterator<Item = (&Stop, Option<&ResolvedStop>)> {
        self.installed_map().into_iter().flat_map(|map| {
            map.guide
                .stops()
                .zip(&map.bindings)
                .map(|((_, stop), binding)| (stop, binding.anchor.as_ref()))
        })
    }

    pub(crate) fn update_guide_progress(&mut self, edit: impl FnOnce(&mut Progress)) -> Result<()> {
        let map = self
            .review_map
            .installed
            .as_mut()
            .filter(|map| map.publication == self.review_map.publication)
            .ok_or_else(|| TuicrError::InvalidInput("No installed review guide".into()))?;
        map.tick(Instant::now());
        let mut progress = map.progress.clone();
        edit(&mut progress);
        map.store.save_progress(&progress)?;
        map.progress = progress;
        Ok(())
    }

    fn installed_map(&self) -> Option<&InstalledGuide> {
        self.review_map
            .installed
            .as_ref()
            .filter(|map| map.publication == self.review_map.publication)
    }

    fn active_map(&self) -> Option<&InstalledGuide> {
        self.installed_map().filter(|map| map.active)
    }

    pub(crate) fn guide_stop_numbers(&self) -> Option<(usize, usize, usize, usize)> {
        let map = self.active_map()?;
        let current = map.current?;
        let (story, _) = map.guide.stops().nth(current)?;
        let before: usize = map.guide.stories[..story]
            .iter()
            .map(|entry| entry.stops.len())
            .sum();
        Some((
            current - before + 1,
            map.guide.stories[story].stops.len(),
            story + 1,
            map.guide.stories.len(),
        ))
    }

    pub(crate) fn guide_story_summary(&self) -> Option<&str> {
        let map = self.active_map()?;
        let (story, _) = map.guide.stops().nth(map.current?)?;
        Some(map.guide.stories[story].summary.as_str()).filter(|summary| !summary.is_empty())
    }

    pub(crate) fn guide_stop_range(&self) -> Option<(u32, u32)> {
        let map = self.active_map()?;
        let anchor = map.bindings[map.current?].anchor.as_ref()?;
        let hunk = &self.diff_files[anchor.file].hunks[anchor.hunk];
        let side = model_side(anchor.side);
        Some((
            source_lineno(&hunk.lines[anchor.first_line], side)?,
            source_lineno(&hunk.lines[anchor.last_line], side)?,
        ))
    }

    pub(crate) fn guide_stop_changed(&self, index: usize) -> bool {
        self.active_map()
            .is_some_and(|map| map.bindings[index].changed)
    }

    pub(crate) fn guide_current_changed(&self) -> bool {
        self.active_map()
            .and_then(|map| map.current)
            .is_some_and(|index| self.guide_stop_changed(index))
    }

    pub(crate) fn guide_on_stop(&self) -> bool {
        self.guide_cursor_in_stop() && self.is_cursor_visible()
    }

    pub(crate) fn guide_cursor_in_stop(&self) -> bool {
        let Some(side) = self.guide_highlight_row(self.diff_state.cursor_line) else {
            return false;
        };
        self.diff_view_mode != DiffViewMode::SideBySide || self.cursor_side == side
    }

    pub(crate) fn clear_guide_verdict_hint_if_left(&mut self) {
        if self.has_guide_verdict_hint()
            && (self.focused_panel != FocusedPanel::Diff
                || self.spot_position().is_some()
                || !self.guide_on_stop())
        {
            self.message = None;
        }
    }

    fn has_guide_verdict_hint(&self) -> bool {
        self.message
            .as_ref()
            .is_some_and(|message| message.content == EXPLICIT_VERDICT_HINT)
    }

    fn clear_guide_verdict_hint(&mut self) {
        if self.has_guide_verdict_hint() {
            self.message = None;
        }
    }

    fn current_guide_anchor(&self) -> Option<&ResolvedStop> {
        if let Some(anchor) = self.spot_anchor() {
            return Some(anchor);
        }
        let map = self.active_map()?;
        map.bindings[map.current?].anchor.as_ref()
    }

    pub(crate) fn guide_highlight_row(&self, row: usize) -> Option<LineSide> {
        let anchor = self.current_guide_anchor()?;
        annotation_in_block(self.line_annotations.get(row)?, anchor)
            .then_some(model_side(anchor.side))
    }

    pub(crate) fn guide_comment_in_stop(&self, row: usize) -> bool {
        let Some(anchor) = self.current_guide_anchor() else {
            return false;
        };
        let side = model_side(anchor.side);
        let line = match self.line_annotations.get(row) {
            Some(&AnnotatedLine::LineComment {
                file_idx,
                line,
                side: comment_side,
                ..
            }) if file_idx == anchor.file && comment_side == side => line,
            Some(&AnnotatedLine::RemoteThreadLine { thread_idx, .. }) => {
                use crate::forge::remote_comments::RemoteCommentSide;
                let Some(thread) = self.forge_review_threads.get(thread_idx) else {
                    return false;
                };
                let comment_side = match thread.side {
                    RemoteCommentSide::Left => LineSide::Old,
                    RemoteCommentSide::Right => LineSide::New,
                };
                if comment_side != side
                    || self.diff_files[anchor.file].display_path() != Path::new(&thread.path)
                {
                    return false;
                }
                let Some(line) = thread.line else {
                    return false;
                };
                line
            }
            _ => return false,
        };
        self.guide_stop_range()
            .is_some_and(|(first, last)| (first..=last).contains(&line))
    }

    pub(crate) fn guide_primary_key(&mut self, explicit: bool) {
        self.clear_guide_verdict_hint();
        if self.spot_position().is_some() {
            self.record_spot_verdict(Verdict::ChecksOut);
            return;
        }
        if self.active_map().is_none() {
            if !explicit {
                self.enter_guide();
            }
        } else if self.current_stop().is_none() {
            self.resume_guide();
        } else if !self.guide_on_stop() {
            self.return_to_guide_stop();
        } else if !explicit
            && self
                .current_stop()
                .is_some_and(Stop::needs_explicit_verdict)
        {
            self.set_warning(EXPLICIT_VERDICT_HINT);
        } else {
            self.record_guide_verdict(Verdict::ChecksOut, None);
        }
    }

    pub(crate) fn enter_guide(&mut self) {
        if !self.guide_review_ready() || self.input_mode != InputMode::Normal {
            self.set_warning("Open a loaded review before entering a review guide");
            return;
        }
        self.stop_spot_check();
        if self.active_map().is_some_and(|map| map.current.is_some()) {
            self.return_to_guide_stop();
            return;
        }
        let Some(map) = self
            .review_map
            .installed
            .as_mut()
            .filter(|map| !map.changes_only)
        else {
            self.set_message("No review guide — use :prepare or start with --guide PATH");
            return;
        };
        if !map.active {
            map.active = true;
            map.entry_cue = None;
            map.show_map = true;
            self.show_file_list = true;
            self.guide_input_activity();
        }
        self.resume_guide();
    }

    fn resume_guide(&mut self) {
        let Some(map) = self.active_map() else { return };
        let changed_pending = map
            .guide
            .stories
            .first()
            .filter(|story| baseline::is_change_story(story))
            .is_some_and(|story| {
                story
                    .stops
                    .iter()
                    .any(|stop| !map.progress.verdicts.contains_key(&stop.id))
            });
        let first = if changed_pending {
            0
        } else {
            map.progress
                .current
                .as_deref()
                .and_then(|id| map.guide.stops().position(|(_, stop)| stop.id == id))
                .unwrap_or(0)
        };
        self.walk_guide((first..map.bindings.len()).chain(0..first), true);
    }

    pub(crate) fn pause_guide(&mut self) {
        self.stop_spot_check();
        self.guide_focus_lost();
        if let Some(map) = self.review_map.installed.as_mut() {
            map.entry_cue = map.active.then_some(GuideEntryCue::Resume);
            map.active = false;
            map.current = None;
            let hint = if map.changes_only {
                ":changes to resume"
            } else {
                ":guide to resume"
            };
            self.set_message(format!("Review paused — {hint}"));
        }
    }

    pub(crate) fn previous_guide_stop(&mut self) {
        self.clear_guide_verdict_hint();
        self.stop_spot_check();
        if let Some(map) = self.active_map() {
            let end = map.current.unwrap_or(map.bindings.len());
            self.walk_guide((0..end).rev(), false);
        }
    }

    pub(crate) fn return_to_guide_stop(&mut self) {
        if self.spot_position().is_some() {
            self.land_current_spot();
            return;
        }
        if let Some(map) = self.active_map() {
            if let Some(current) = map.current {
                if self.guide_stop_visible(current, true) {
                    self.land_guide_stop(current, true);
                } else {
                    let end = map.bindings.len();
                    self.warn_guide_skipped(current);
                    self.walk_guide((current + 1..end).chain(0..current), true);
                }
            } else {
                self.resume_guide();
            }
        }
    }

    pub(crate) fn park_guide_stop(&mut self) {
        self.clear_guide_verdict_hint();
        if self.spot_position().is_some() {
            self.record_spot_verdict(Verdict::Parked);
            return;
        }
        if self.guide_on_stop() {
            self.record_guide_verdict(Verdict::Parked, None);
        } else {
            self.return_to_guide_stop();
        }
    }

    fn walk_guide(&mut self, indices: impl Iterator<Item = usize>, unchecked: bool) {
        let Some(map) = self.active_map() else { return };
        let mut skipped = Vec::new();
        let mut next = None;
        for index in indices {
            let Some(stop) = map.stop(index) else {
                continue;
            };
            if unchecked && map.progress.verdicts.contains_key(&stop.id) {
                continue;
            }
            if self.guide_stop_visible(index, !unchecked) {
                next = Some(index);
                break;
            }
            if !skipped.contains(&stop.path.as_str()) {
                skipped.push(stop.path.as_str());
            }
        }
        let warning = (!skipped.is_empty())
            .then(|| format!("Skipped hidden or changed stop: {}", skipped.join(", ")));
        if let Some(index) = next {
            self.land_guide_stop(index, !unchecked);
        } else if unchecked && let Some(map) = self.review_map.installed.as_mut() {
            map.current = None;
            map.progress.current = None;
            self.flush_guide_progress();
        }
        if let Some(warning) = warning {
            self.set_warning(warning);
        }
    }

    fn warn_guide_skipped(&mut self, index: usize) {
        if let Some(stop) = self.active_map().and_then(|map| map.stop(index)) {
            self.set_warning(format!("Skipped hidden or changed stop: {}", stop.path));
        }
    }

    fn guide_stop_visible(&self, index: usize, revisit: bool) -> bool {
        let Some(map) = self.active_map() else {
            return false;
        };
        let Some(anchor) = map.bindings[index].anchor.as_ref() else {
            return false;
        };
        let reveal = map.stop(index).is_some_and(|stop| {
            stop.id.starts_with(baseline::STOP_PREFIX)
                || (revisit && map.progress.verdicts.contains_key(&stop.id))
        });
        self.file_idx_passes_filter(anchor.file)
            && (reveal
                || (!self.should_collapse_file(anchor.file)
                    && !self.should_collapse_hunk(anchor.file, anchor.hunk)))
    }

    fn land_guide_stop(&mut self, index: usize, revisit: bool) {
        self.clear_guide_verdict_hint();
        let Some(map) = self.active_map() else { return };
        let Some(anchor) = map.bindings[index].anchor.as_ref() else {
            return;
        };
        let (file, hunk, first, last, side) = (
            anchor.file,
            anchor.hunk,
            anchor.first_line,
            anchor.last_line,
            anchor.side,
        );
        let Some(stop_id) = map.stop(index).map(|stop| stop.id.clone()) else {
            return;
        };
        let side = model_side(side);
        if revisit || stop_id.starts_with(baseline::STOP_PREFIX) {
            if self.should_collapse_file(file) {
                self.reveal_reviewed_file(file);
            }
            if self.should_collapse_hunk(file, hunk) {
                self.reveal_reviewed_hunk(file, hunk);
            }
            self.rebuild_annotations();
        }
        self.jump_to_file(file);
        self.expand_guide_context(file, hunk, first, side);
        let Some(row) = self.line_annotations.iter().position(|annotation| {
            block_annotation(annotation, file, hunk, side)
                .is_some_and(|index| first <= index && index <= last)
        }) else {
            self.warn_guide_skipped(index);
            return;
        };
        self.focused_panel = FocusedPanel::Diff;
        self.set_cursor_side(side);
        self.diff_state.scroll_x = 0;
        self.move_cursor_to_annotation(row);
        self.diff_state.scroll_offset = self
            .scroll_offset_for_rows_above(row, LANDING_CONTEXT_ROWS)
            .min(self.max_scroll_offset());
        self.review_map.pending_landing = true;
        if let Some(map) = self.review_map.installed.as_mut() {
            map.current = Some(index);
            map.selected = Some(MapEntry::Stop(index));
            map.progress.current = Some(stop_id);
        }
        self.flush_guide_progress();
    }

    pub fn keep_guide_stop_in_view(&mut self) {
        if matches!(
            self.input_mode,
            InputMode::Normal | InputMode::Command | InputMode::Companion
        ) && self.guide_on_stop()
        {
            self.review_map.pending_landing = true;
        }
    }

    pub(crate) fn guide_preview_height(&self, width: usize, limit: u16) -> u16 {
        let Some(anchor) = self.current_guide_anchor() else {
            return 0;
        };
        let Some(first) = self.guide_first_annotation(anchor) else {
            return 0;
        };
        let side = model_side(anchor.side);
        let mut stop_lines = 0;
        let mut height: u16 = 0;
        for (row, annotation) in self.line_annotations.iter().enumerate().skip(first) {
            let source = block_annotation(annotation, anchor.file, anchor.hunk, side);
            if source.is_some_and(|line| line > anchor.last_line) {
                break;
            }
            if matches!(
                annotation,
                AnnotatedLine::FileHeader { .. } | AnnotatedLine::HunkHeader { .. }
            ) {
                break;
            }
            let rows = crate::ui::row_height::annotation_row_height_at_width(self, row, width);
            height = height.saturating_add(u16::try_from(rows).unwrap_or(u16::MAX));
            if height >= limit {
                return limit;
            }
            if let Some(line) = source {
                stop_lines += 1;
                if stop_lines == super::companion::DOCK_PREVIEW_LINES || line == anchor.last_line {
                    break;
                }
            }
        }
        height
    }

    pub(crate) fn guide_first_row_height(&self, width: usize) -> u16 {
        self.current_guide_anchor()
            .and_then(|anchor| self.guide_first_annotation(anchor))
            .map_or(1, |first| {
                let rows =
                    crate::ui::row_height::annotation_row_height_at_width(self, first, width);
                u16::try_from(rows).unwrap_or(u16::MAX)
            })
    }

    fn guide_first_annotation(&self, anchor: &ResolvedStop) -> Option<usize> {
        let side = model_side(anchor.side);
        let index = |row| block_annotation(row, anchor.file, anchor.hunk, side);
        let cursor = self.diff_state.cursor_line;
        if self.line_annotations.get(cursor).and_then(index) == Some(anchor.first_line) {
            Some(cursor)
        } else {
            self.line_annotations
                .iter()
                .position(|row| index(row) == Some(anchor.first_line))
        }
    }

    fn guide_annotation_bounds(&self, anchor: &ResolvedStop) -> Option<(usize, usize)> {
        let side = model_side(anchor.side);
        let index = |row| block_annotation(row, anchor.file, anchor.hunk, side);
        let first = self.guide_first_annotation(anchor)?;
        let length = self.line_annotations[first..]
            .iter()
            .position(|row| index(row) == Some(anchor.last_line))?;
        Some((first, first + length))
    }

    /// Resolve the block after syncing the renderer's actual viewport width.
    /// The renderer retains this small window and measures it with its own
    /// wrapping logic, rather than estimating from the previous frame.
    pub(crate) fn prepare_guide_viewport(&mut self) -> Option<GuideViewport> {
        if !matches!(
            self.input_mode,
            InputMode::Normal | InputMode::Command | InputMode::Companion
        ) || !std::mem::take(&mut self.review_map.pending_landing)
        {
            return None;
        }
        let anchor = self.current_guide_anchor()?;
        let side = model_side(anchor.side);
        let (first, last) = self.guide_annotation_bounds(anchor)?;
        let mut context_start = first;
        let mut context_lines = 0;
        while context_start > 0 && context_lines < LANDING_CONTEXT_ROWS {
            match &self.line_annotations[context_start - 1] {
                AnnotatedLine::DiffLine { file_idx, .. }
                | AnnotatedLine::SideBySideLine { file_idx, .. }
                    if *file_idx == anchor.file =>
                {
                    context_lines += 1
                }
                AnnotatedLine::ExpandedContext { gap_id, .. } if gap_id.file_idx == anchor.file => {
                    context_lines += 1
                }
                AnnotatedLine::HunkHeader { file_idx, .. } if *file_idx == anchor.file => {}
                _ => break,
            }
            context_start -= 1;
        }
        self.set_cursor_side(side);
        self.move_cursor_to_annotation(first);
        Some(GuideViewport {
            first,
            last,
            context_start,
        })
    }

    pub(crate) fn open_guide_details(&mut self) {
        if self.current_stop().is_none() {
            self.set_message("Enter a review guide and select a stop first");
            return;
        }
        self.overlay_return_mode = self.input_mode;
        self.input_mode = InputMode::GuideDetails;
        self.help_state.scroll_offset = 0;
    }

    fn expand_guide_context(&mut self, file: usize, hunk: usize, first: usize, side: LineSide) {
        let diff_hunk = &self.diff_files[file].hunks[hunk];
        let in_hunk = diff_hunk.lines[..first]
            .iter()
            .filter(|line| source_lineno(line, side).is_some())
            .count();
        let gap = GapId {
            file_idx: file,
            hunk_idx: hunk,
        };
        let already_expanded = self.expanded_bottom.get(&gap).map_or(0, Vec::len);
        let needed = LANDING_CONTEXT_ROWS.saturating_sub(in_hunk + already_expanded);
        let gap_start = hunk.checked_sub(1).map_or(1, |prev| {
            let previous = &self.diff_files[file].hunks[prev];
            previous.new_start + previous.new_count
        });
        if needed > 0
            && diff_hunk.new_start > gap_start
            && !self.gap_is_hidden(&gap)
            && let Err(error) = self.expand_gap(gap, ExpandDirection::Up, Some(needed))
        {
            self.set_warning(format!("Could not reveal context above stop: {error}"));
        }
    }

    pub(crate) fn open_wrong_guide_comment(&mut self) {
        self.clear_guide_verdict_hint();
        if self.spot_position().is_some() {
            self.open_wrong_spot_comment();
            return;
        }
        if !self.guide_on_stop() {
            self.return_to_guide_stop();
            return;
        }
        let Some(map) = self.active_map() else { return };
        let Some(stop) = map.current.and_then(|index| map.stop(index)) else {
            return;
        };
        let Some((start, end)) = self.guide_stop_range() else {
            return;
        };
        let side = model_side(stop.side);
        let content = format!("This does not check out: {}", stop.claim);
        if let Some(row) = self.line_annotations.iter().rposition(|annotation| {
            self.current_guide_anchor()
                .is_some_and(|anchor| annotation_in_block(annotation, anchor))
        }) {
            self.move_cursor_to_annotation(row);
        }
        self.enter_comment_mode(false, Some((end, side)));
        self.comment_line_range = Some((LineRange::new(start, end), side));
        self.comment_cursor = content.len();
        self.comment_buffer = content;
        self.review_map.pending_wrong = Some(self.review_map.publication);
    }

    pub(super) fn guide_comment_can_save(&self) -> bool {
        self.review_map.pending_wrong.is_none_or(|publication| {
            publication == self.review_map.publication
                && self
                    .guide_highlight_row(self.diff_state.cursor_line)
                    .is_some()
        })
    }

    pub(super) fn cancel_guide_comment(&mut self) {
        self.review_map.pending_wrong = None;
    }

    pub(super) fn guide_comment_pending(&self) -> bool {
        self.review_map.pending_wrong.is_some()
    }

    pub(super) fn finish_guide_comment(&mut self, comment_id: String) {
        if self.spot_position().is_some() {
            self.record_spot_verdict(Verdict::Wrong);
        } else {
            self.record_guide_verdict(Verdict::Wrong, Some(comment_id));
        }
    }

    fn record_guide_verdict(&mut self, verdict: Verdict, comment_id: Option<String>) {
        let Some(map) = self.active_map() else { return };
        let Some(index) = map.current else { return };
        let Some(anchor) = map.bindings[index].anchor.as_ref() else {
            return;
        };
        let Some(stop_id) = map.stop(index).map(|stop| stop.id.clone()) else {
            return;
        };
        let block = block_text(&self.diff_files, anchor);
        let (file, hunk) = (anchor.file, anchor.hunk);
        let mark_reviewed = verdict == Verdict::ChecksOut
            && covers_changed_lines(&self.diff_files[file].hunks[hunk], anchor);
        let end = map.bindings.len();
        let Some(map) = self.review_map.installed.as_mut() else {
            return;
        };
        let previous = map.progress.verdicts.insert(
            stop_id.clone(),
            VerdictRecord {
                verdict,
                at: chrono::Utc::now(),
                block,
                comment_id,
            },
        );
        if let Err(error) = map.store.save_progress(&map.progress) {
            if let Some(previous) = previous {
                map.progress.verdicts.insert(stop_id, previous);
            } else {
                map.progress.verdicts.remove(&stop_id);
            }
            self.set_error(format!("Could not save review guide verdict: {error}"));
            return;
        }
        map.bindings[index].changed = false;
        if mark_reviewed && !self.is_hunk_reviewed(file, hunk) {
            self.toggle_hunk_reviewed();
            if let Err(error) = self.save_current_session_merging_external() {
                self.set_error(format!(
                    "Verdict saved; could not save hunk reviewed mark: {error}"
                ));
            }
        }
        self.walk_guide((index + 1..end).chain(0..index), true);
    }

    pub(crate) fn showing_review_map(&self) -> bool {
        self.show_file_list
            && !self.file_tree_prompt_editing()
            && self.active_map().is_some_and(|map| map.show_map)
    }

    pub(crate) fn toggle_review_map(&mut self) {
        self.keep_guide_stop_in_view();
        if let Some(map) = self.review_map.installed.as_mut().filter(|map| map.active) {
            map.show_map = !map.show_map || !self.show_file_list;
            self.show_file_list = true;
        } else {
            self.set_message("Enter a review guide first with :guide");
        }
    }

    pub(crate) fn guide_map_rows(&self) -> impl Iterator<Item = MapRow<'_>> {
        self.active_map().into_iter().flat_map(InstalledGuide::rows)
    }

    pub(crate) fn guide_map_selection(&self) -> (Option<MapEntry>, Option<MapEntry>) {
        self.active_map().map_or((None, None), |map| {
            (map.selected, map.current.map(MapEntry::Stop))
        })
    }

    pub(crate) fn guide_supporting_open(&self) -> bool {
        self.active_map()
            .is_some_and(InstalledGuide::supporting_open)
    }

    pub(crate) fn move_guide_map_selection(&mut self, delta: isize) {
        let Some(map) = self.active_map() else { return };
        let entries: Vec<_> = map.rows().filter_map(|row| row.entry()).collect();
        let selected = map
            .selected
            .and_then(|selected| entries.iter().position(|entry| *entry == selected))
            .unwrap_or(0);
        let next = selected
            .saturating_add_signed(delta)
            .min(entries.len().saturating_sub(1));
        if let Some(map) = self.review_map.installed.as_mut() {
            map.selected = entries.get(next).copied();
        }
    }

    pub(crate) fn enter_guide_map_selection(&mut self) {
        self.stop_spot_check();
        let Some(map) = self.active_map() else { return };
        match map.selected {
            Some(MapEntry::Story(index)) => {
                let start = map.guide.stories[..index]
                    .iter()
                    .map(|story| story.stops.len())
                    .sum::<usize>();
                let end = start + map.guide.stories[index].stops.len();
                self.walk_guide(start..end, false);
            }
            Some(MapEntry::Stop(index)) if self.guide_stop_visible(index, true) => {
                self.land_guide_stop(index, true)
            }
            Some(MapEntry::Stop(index)) => self.warn_guide_skipped(index),
            Some(MapEntry::Group(index)) if map.supporting_open() => {
                let group = &map.guide.supporting[index];
                let note = if group.note.is_empty() {
                    "Review its hunks with :spot"
                } else {
                    &group.note
                };
                self.set_message(format!("{}: {note}", group.label));
            }
            _ => {}
        }
    }

    pub(crate) fn dismiss_guide_group(&mut self) {
        let Some(map) = self.active_map().filter(|map| map.supporting_open()) else {
            return;
        };
        let Some(MapEntry::Group(index)) = map.selected else {
            return;
        };
        let group = &map.guide.supporting[index];
        if group.basis != Basis::Mechanical {
            self.set_warning("Claimed supporting changes cannot be dismissed in bulk — use :spot");
        } else if !map.progress.dismissed_groups.contains(&group.label) {
            if map.guide.diff != self.review_digest() {
                self.set_warning("Supporting changes are from an older diff — use :prepare");
                return;
            }
            let label = group.label.clone();
            match self.update_guide_progress(|progress| progress.dismissed_groups.push(label)) {
                Ok(()) => self.set_message("Mechanical supporting group dismissed"),
                Err(error) => {
                    self.set_error(format!("Could not dismiss supporting changes: {error}"))
                }
            }
        }
    }

    pub(crate) fn set_guide_map_viewport(
        &mut self,
        area: Option<ratatui::layout::Rect>,
        entries: Vec<Option<MapEntry>>,
    ) {
        self.review_map.map_area = area;
        self.review_map.map_hits = entries;
    }

    pub(crate) fn click_guide_map(&mut self, position: ratatui::layout::Position) -> bool {
        let Some(area) = self
            .review_map
            .map_area
            .filter(|area| area.contains(position))
        else {
            return false;
        };
        let row = usize::from(position.y - area.y);
        let selected = self.review_map.map_hits.get(row).copied().flatten();
        if let Some(map) = self.review_map.installed.as_mut()
            && selected.is_some()
        {
            map.selected = selected;
        }
        true
    }

    pub fn guide_input_activity(&mut self) {
        let now = Instant::now();
        let sampling = self.spot_position().is_some();
        if let Some(map) = self
            .review_map
            .installed
            .as_mut()
            .filter(|map| map.active || sampling)
        {
            map.tick(now);
            map.last_input = Some(now);
            map.elapsed_at = Some(now);
        }
    }

    pub fn guide_focus_lost(&mut self) {
        if let Some(map) = self
            .review_map
            .installed
            .as_mut()
            .filter(|map| map.last_input.is_some())
        {
            map.tick(Instant::now());
            map.last_input = None;
            map.elapsed_at = None;
            self.flush_guide_progress();
        }
    }

    fn flush_guide_progress(&mut self) {
        if let Some(map) = self.review_map.installed.as_mut() {
            map.tick(Instant::now());
            if let Err(error) = map.store.save_progress(&map.progress) {
                self.set_error(format!("Could not save review guide progress: {error}"));
            }
        }
    }
}

fn digest_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn local_scope(source: &DiffSource) -> (&'static str, &[String]) {
    match source {
        DiffSource::WorkingTree => ("worktree", &[]),
        DiffSource::Staged => ("staged", &[]),
        DiffSource::Unstaged => ("unstaged", &[]),
        DiffSource::StagedAndUnstaged => ("staged-and-unstaged", &[]),
        DiffSource::CommitRange(commits) => ("commits", commits),
        DiffSource::StagedUnstagedAndCommits(commits) => ("worktree-and-commits", commits),
        DiffSource::PullRequest(_) => unreachable!("local sources only"),
    }
}

fn model_side(side: Side) -> LineSide {
    match side {
        Side::Old => LineSide::Old,
        Side::New => LineSide::New,
    }
}

fn source_lineno(line: &DiffLine, side: LineSide) -> Option<u32> {
    match side {
        LineSide::Old => line.old_lineno,
        LineSide::New => line.new_lineno,
    }
}

fn annotation_in_block(annotation: &AnnotatedLine, anchor: &ResolvedStop) -> bool {
    block_annotation(
        annotation,
        anchor.file,
        anchor.hunk,
        model_side(anchor.side),
    )
    .is_some_and(|index| anchor.first_line <= index && index <= anchor.last_line)
}

fn block_annotation(
    annotation: &AnnotatedLine,
    file: usize,
    hunk: usize,
    side: LineSide,
) -> Option<usize> {
    match *annotation {
        AnnotatedLine::DiffLine {
            file_idx,
            hunk_idx,
            line_idx,
            old_lineno,
            new_lineno,
            ..
        } if (file_idx, hunk_idx) == (file, hunk) => match side {
            LineSide::Old => old_lineno,
            LineSide::New => new_lineno,
        }
        .map(|_| line_idx),
        AnnotatedLine::SideBySideLine {
            file_idx,
            hunk_idx,
            del_line_idx,
            add_line_idx,
            ..
        } if (file_idx, hunk_idx) == (file, hunk) => match side {
            LineSide::Old => del_line_idx,
            LineSide::New => add_line_idx,
        },
        _ => None,
    }
}

fn covers_changed_lines(hunk: &DiffHunk, anchor: &ResolvedStop) -> bool {
    let mut changes = hunk
        .lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.origin != LineOrigin::Context)
        .peekable();
    changes.peek().is_some()
        && changes.all(|(index, line)| {
            (anchor.first_line..=anchor.last_line).contains(&index)
                && source_lineno(line, model_side(anchor.side)).is_some()
        })
}

fn complete_supplied_guide(guide: &mut ReviewGuide, files: &[DiffFile]) {
    let files = &files[files.partition_point(|file| file.is_commit_message)..];
    let mechanical = crate::review_guide::mechanical::classify(files);
    crate::review_guide::mechanical::complete(guide, files, &mechanical);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FilePatch;
    use crate::review_guide::{GUIDE_VERSION, Risk, RiskKind, Story};

    const REPLACEMENT: &str = "@@ -1,3 +1,3 @@\n context\n-before\n+after\n tail\n";

    fn file(path: &str, patch: &str) -> FilePatch {
        FilePatch::new(
            Some(path.into()),
            Some(path.into()),
            FileStatus::Modified,
            patch,
        )
    }

    fn review(root: &Path, patches: Vec<FilePatch>) -> App {
        let info = VcsInfo {
            root_path: root.to_path_buf(),
            head_commit: "base".into(),
            branch_name: None,
            vcs_type: VcsType::Git,
        };
        let session = ReviewSession::new(
            root.to_path_buf(),
            "base".into(),
            None,
            SessionDiffSource::WorkingTree,
        );
        let mut app = App::build(
            Box::new(PrNoopVcs::new(info.clone())),
            info,
            Theme::dark(),
            None,
            false,
            crate::vcs::diff_parser::parse_file_patches(patches).unwrap(),
            session,
            DiffSource::WorkingTree,
            InputMode::Normal,
            Vec::new(),
            None,
            None,
        )
        .unwrap();
        app.diff_state.viewport_height = 25;
        app.diff_state.viewport_width = 100;
        app
    }

    fn stop(id: &str, start_line: u32, end_line: u32) -> Stop {
        Stop {
            id: id.into(),
            claim: format!("Confirm {id}"),
            path: format!("{id}.txt"),
            side: Side::New,
            start_line,
            end_line,
            explanation: String::new(),
            symbol: None,
            test: None,
            risk: None,
            untested: false,
            audit: Default::default(),
            followups: Vec::new(),
        }
    }

    fn guide(app: &App, stops: Vec<Stop>) -> ReviewGuide {
        ReviewGuide {
            version: GUIDE_VERSION,
            diff: app.review_digest(),
            summary: String::new(),
            stories: vec![Story {
                title: "Walk the change".into(),
                summary: String::new(),
                stops,
            }],
            supporting: Vec::new(),
            estimate_minutes: Some(10),
            provenance: Default::default(),
        }
    }

    fn current(app: &App) -> Option<&str> {
        app.current_stop().map(|stop| stop.id.as_str())
    }

    #[test]
    fn guide_map_navigation_uses_story_order_and_skips_hidden_files() {
        let root = tempfile::tempdir().unwrap();
        let mut app = review(
            root.path(),
            ["api", "bridge", "data", "result"]
                .into_iter()
                .map(|name| file(&format!("{name}.txt"), REPLACEMENT))
                .collect(),
        );
        let guide = guide(
            &app,
            ["bridge", "api", "data", "result"]
                .into_iter()
                .map(|name| stop(name, 2, 2))
                .collect(),
        );
        let original = app.diff_state.cursor_line;
        app.install_guide(guide).unwrap();
        assert_eq!(app.diff_state.cursor_line, original);
        assert!(app.active_guide().is_none());
        let data = app
            .diff_files
            .iter()
            .position(|file| file.display_path() == Path::new("data.txt"))
            .unwrap();
        app.toggle_reviewed_for_file_idx(data, false);
        app.set_show_reviewed(false);
        app.begin_file_tree_prompt(FileTreePrompt::Exclude);
        app.file_tree_prompt_insert_str(r"^api\.txt$");
        app.commit_file_tree_prompt();

        app.enter_guide();
        assert_eq!(current(&app), Some("bridge"));
        app.guide_primary_key(false);
        assert_eq!(current(&app), Some("result"));
        app.previous_guide_stop();
        assert_eq!(current(&app), Some("bridge"));
        let progress = app.guide_progress().unwrap();
        assert_eq!(progress.verdicts.len(), 1);
        assert_eq!(progress.verdicts["bridge"].verdict, Verdict::ChecksOut);
    }

    #[test]
    fn guide_map_risk_requires_an_explicit_verdict() {
        let root = tempfile::tempdir().unwrap();
        let mut app = review(root.path(), vec![file("api.txt", REPLACEMENT)]);
        let mut stop = stop("api", 2, 2);
        stop.risk = Some(Risk {
            kind: RiskKind::ErrorHandling,
            reason: "Error path changed".into(),
        });
        app.install_guide(guide(&app, vec![stop])).unwrap();
        app.enter_guide();

        app.guide_primary_key(false);
        assert_eq!(current(&app), Some("api"));
        assert!(app.guide_progress().unwrap().verdicts.is_empty());
        assert!(app.has_guide_verdict_hint());
        app.guide_primary_key(true);
        assert_eq!(current(&app), None);
        assert!(!app.has_guide_verdict_hint());
        assert_eq!(
            app.guide_progress().unwrap().verdicts["api"].verdict,
            Verdict::ChecksOut
        );
    }

    #[test]
    fn guide_map_clears_the_verdict_hint_on_navigation_and_other_verdict_keys() {
        let root = tempfile::tempdir().unwrap();
        let mut app = review(
            root.path(),
            vec![
                file("before.txt", REPLACEMENT),
                file("api.txt", REPLACEMENT),
            ],
        );
        let mut risky = stop("api", 2, 2);
        risky.risk = Some(Risk {
            kind: RiskKind::ErrorHandling,
            reason: "Error path changed".into(),
        });
        app.install_guide(guide(&app, vec![stop("before", 2, 2), risky]))
            .unwrap();
        app.enter_guide();
        app.guide_primary_key(false);
        app.guide_primary_key(false);
        assert!(app.has_guide_verdict_hint());
        app.previous_guide_stop();
        assert!(!app.has_guide_verdict_hint());

        app.guide_primary_key(false);
        app.guide_primary_key(false);
        assert!(app.has_guide_verdict_hint());
        app.open_wrong_guide_comment();
        assert_eq!(app.input_mode, InputMode::Comment);
        assert!(!app.has_guide_verdict_hint());
        app.exit_comment_mode();
        app.return_to_guide_stop();

        app.guide_primary_key(false);
        assert!(app.has_guide_verdict_hint());
        app.park_guide_stop();
        assert!(!app.has_guide_verdict_hint());
    }

    #[test]
    fn guide_map_persists_verdicts_and_resumes_at_the_next_unchecked_stop() {
        let root = tempfile::tempdir().unwrap();
        let fixture = || {
            review(
                root.path(),
                vec![file("one.txt", REPLACEMENT), file("two.txt", REPLACEMENT)],
            )
        };
        let mut app = fixture();
        let guide = guide(&app, vec![stop("two", 2, 2), stop("one", 2, 2)]);
        app.install_guide(guide.clone()).unwrap();
        app.enter_guide();
        app.park_guide_stop();
        assert_eq!(current(&app), Some("one"));
        let persisted = GuideStore::open(&app.review_key().unwrap())
            .unwrap()
            .progress()
            .unwrap()
            .unwrap();
        assert_eq!(persisted.current.as_deref(), Some("one"));
        assert_eq!(persisted.verdicts["two"].verdict, Verdict::Parked);

        let mut reopened = fixture();
        reopened.install_guide(guide).unwrap();
        assert!(reopened.current_stop().is_none());
        reopened.enter_guide();
        assert_eq!(current(&reopened), Some("one"));
        assert_eq!(
            reopened.guide_progress().unwrap().verdicts,
            persisted.verdicts
        );
    }

    #[test]
    fn guide_map_reload_keeps_moved_text_and_drops_changed_verdicts() {
        let root = tempfile::tempdir().unwrap();
        let mut app = review(
            root.path(),
            vec![file("one.txt", REPLACEMENT), file("two.txt", REPLACEMENT)],
        );
        app.install_guide(guide(&app, vec![stop("one", 2, 2), stop("two", 2, 2)]))
            .unwrap();
        app.enter_guide();
        app.guide_primary_key(false);
        app.guide_primary_key(false);
        assert_eq!(app.guide_progress().unwrap().verdicts.len(), 2);

        app.diff_files = crate::vcs::diff_parser::parse_file_patches(vec![
            file(
                "one.txt",
                "@@ -1,3 +1,4 @@\n context\n-before\n+another\n+after\n tail\n",
            ),
            file(
                "two.txt",
                "@@ -1,3 +1,3 @@\n context\n-before\n+gone\n tail\n",
            ),
        ])
        .unwrap();
        app.guide_diff_published();
        app.rebuild_annotations();
        assert!(app.poll_review_guide());

        let mut stops = app.guide_resolved_stops();
        let (_, moved) = stops.next().unwrap();
        assert_eq!(moved.unwrap().first_line, 3);
        let (_, changed) = stops.next().unwrap();
        assert!(changed.is_none());
        assert!(app.guide_stop_changed(1));
        let progress = app.guide_progress().unwrap();
        assert!(progress.verdicts.contains_key("one"));
        assert!(!progress.verdicts.contains_key("two"));
        assert_eq!(
            GuideStore::open(&app.review_key().unwrap())
                .unwrap()
                .progress()
                .unwrap()
                .unwrap()
                .verdicts,
            progress.verdicts,
        );
    }

    #[test]
    fn stale_local_guide_reopens_and_navigates_only_unchanged_stops() {
        let root = tempfile::tempdir().unwrap();
        let mut original = review(
            root.path(),
            vec![file("one.txt", REPLACEMENT), file("two.txt", REPLACEMENT)],
        );
        let guide = guide(&original, vec![stop("one", 2, 2), stop("two", 2, 2)]);
        original.install_guide(guide).unwrap();

        let mut reopened = review(
            root.path(),
            vec![
                file(
                    "one.txt",
                    "@@ -1,3 +1,3 @@\n context\n-before\n+gone\n tail\n",
                ),
                file(
                    "two.txt",
                    "@@ -1,3 +1,4 @@\n context\n-before\n+inserted\n+after\n tail\n",
                ),
            ],
        );
        assert!(reopened.poll_review_guide());
        assert!(reopened.installed_guide_is_stale());
        reopened.enter_guide();
        assert_eq!(current(&reopened), Some("two"));
        assert_eq!(reopened.guide_stop_range(), Some((3, 3)));
    }

    #[test]
    fn guide_map_only_marks_a_hunk_when_the_block_covers_all_changes() {
        let root = tempfile::tempdir().unwrap();
        const ADDITIONS: &str = "@@ -1,2 +1,4 @@\n context\n+first\n+second\n tail\n";
        let mut app = review(
            root.path(),
            vec![
                file("partial.txt", ADDITIONS),
                file("whole.txt", ADDITIONS),
                file("mixed.txt", REPLACEMENT),
            ],
        );
        let stops = vec![
            stop("partial", 2, 2),
            stop("whole", 2, 3),
            stop("mixed", 1, 3),
        ];
        app.install_guide(guide(&app, stops)).unwrap();
        app.enter_guide();
        for _ in 0..3 {
            app.guide_primary_key(false);
        }

        assert_eq!(app.guide_progress().unwrap().verdicts.len(), 3);
        for (index, file) in app.diff_files.iter().enumerate() {
            let should_mark = file.display_path() == Path::new("whole.txt");
            assert_eq!(app.is_hunk_reviewed(index, 0), should_mark);
            assert!(!app.session.is_file_reviewed(file.display_path()));
        }
    }

    #[test]
    fn guide_map_keeps_the_paired_old_side_focused() {
        let root = tempfile::tempdir().unwrap();
        let mut app = review(root.path(), vec![file("api.txt", REPLACEMENT)]);
        app.diff_view_mode = DiffViewMode::SideBySide;
        app.rebuild_annotations();
        let mut stop = stop("api", 2, 2);
        stop.side = Side::Old;
        app.install_guide(guide(&app, vec![stop])).unwrap();
        app.enter_guide();
        assert_eq!(current(&app), Some("api"));
        assert_eq!(app.effective_cursor_side(), LineSide::Old);
        assert_eq!(
            app.guide_highlight_row(app.diff_state.cursor_line),
            Some(LineSide::Old)
        );
    }

    #[test]
    fn guide_map_polls_a_ready_guide_without_moving_the_cursor() {
        use crate::review_guide::PrepState;

        let root = tempfile::tempdir().unwrap();
        let mut app = review(root.path(), vec![file("api.txt", REPLACEMENT)]);
        let guide = guide(&app, vec![stop("api", 2, 2)]);
        let store = GuideStore::open(&app.review_key().unwrap()).unwrap();
        store.save_guide(&guide).unwrap();
        let mut state = PrepState::started("Audit", Some(guide.diff.clone()), None);
        store.save_state(&state).unwrap();
        assert!(!app.poll_review_guide());
        assert!(app.installed_guide().is_none());

        state.status = PrepStatus::Ready;
        state.finished_at = Some(Utc::now());
        store.save_state(&state).unwrap();
        app.review_map.poll_at = None;
        let original = (
            app.diff_state.cursor_line,
            app.diff_state.scroll_offset,
            app.focused_panel,
        );
        assert!(app.poll_review_guide());
        assert_eq!(app.installed_guide(), Some(&guide));
        assert!(app.active_guide().is_none());
        assert_eq!(
            (
                app.diff_state.cursor_line,
                app.diff_state.scroll_offset,
                app.focused_panel
            ),
            original
        );
    }

    #[test]
    fn guide_map_disregards_audit_claims_from_a_launch_file() {
        use crate::review_guide::Audit;

        let root = tempfile::tempdir().unwrap();
        let mut app = review(
            root.path(),
            vec![
                file("api.txt", REPLACEMENT),
                file("bridge.txt", REPLACEMENT),
            ],
        );
        let mut claimed = stop("api", 2, 2);
        claimed.audit = Audit::Holds;
        let mut doubtful = stop("bridge", 2, 2);
        doubtful.audit = Audit::Doubtful {
            note: "An audit the author claims to have run".into(),
        };
        let mut guide = guide(&app, vec![claimed, doubtful]);
        guide.provenance.audited = true;

        app.queue_supplied_guide(guide);
        assert!(app.poll_review_guide());
        let installed = app.installed_guide().unwrap();
        assert_eq!(installed.provenance.author, Author::Supplied);
        assert!(!installed.provenance.audited);
        assert_eq!(installed.stop_count(), 2);
        assert!(
            installed
                .stops()
                .all(|(_, stop)| stop.audit == Audit::Unchecked)
        );
    }

    #[test]
    fn guide_map_replacement_keeps_only_matching_verdicts_and_waits_to_land() {
        use crate::review_guide::PrepState;

        let root = tempfile::tempdir().unwrap();
        let mut app = review(
            root.path(),
            vec![
                file("api.txt", REPLACEMENT),
                file("bridge.txt", REPLACEMENT),
            ],
        );
        let first = stop("api", 2, 2);
        let keep = stop("bridge", 2, 2);
        app.install_guide(guide(&app, vec![first.clone(), keep.clone()]))
            .unwrap();
        app.enter_guide();
        app.guide_primary_key(false);
        app.guide_primary_key(false);
        app.previous_guide_stop();
        assert_eq!(current(&app), Some("bridge"));
        let cursor = app.diff_state.cursor_line;
        let offset = app.diff_state.scroll_offset;

        let mut changed = first;
        changed.start_line = 3;
        changed.end_line = 3;
        let mut new = stop("bridge", 1, 1);
        new.id = "new".into();
        let replacement = guide(&app, vec![new, keep, changed]);
        let mut prep = PrepState::started("Audit", Some(replacement.diff.clone()), None);
        prep.status = PrepStatus::Ready;
        prep.finished_at = Some(Utc::now());
        let store = GuideStore::open(&app.review_key().unwrap()).unwrap();
        store.save_guide(&replacement).unwrap();
        store.save_state(&prep).unwrap();
        assert!(app.poll_review_guide());
        assert_eq!(
            (app.diff_state.cursor_line, app.diff_state.scroll_offset),
            (cursor, offset)
        );
        assert!(app.active_guide().is_none());
        assert!(matches!(
            app.guide_entry_cue(),
            Some((_, GuideEntryCue::Resume))
        ));
        let progress = app.guide_progress().unwrap();
        assert_eq!(progress.current.as_deref(), Some("new"));
        assert_eq!(progress.verdicts.len(), 1);
        assert!(progress.verdicts.contains_key("bridge"));
        app.enter_guide();
        assert_eq!(current(&app), Some("new"));
    }

    fn guide_frame_buffer(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| crate::ui::render(frame, app))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn draw_guide_frame(app: &mut App, width: u16, height: u16) -> String {
        let buffer = guide_frame_buffer(app, width, height);
        let mut screen = String::new();
        for y in 0..height {
            for x in 0..width {
                screen.push_str(buffer[(x, y)].symbol());
            }
            screen.push('\n');
        }
        screen
    }

    #[test]
    fn guide_map_wrapped_stop_card_clears_the_detour_on_the_first_return_frame() {
        let root = tempfile::tempdir().unwrap();
        let tail = "long wrapped context ".repeat(40);
        let patch = format!(
            concat!(
                "@@ -1,6 +1,12 @@\n context one\n context two\n context three\n",
                "+STOPCODE one\n+stop two\n+stop three\n+stop four\n+stop five\n+stop six\n",
                " {}\n {}\n {}\n",
            ),
            tail, tail, tail
        );
        let mut app = review(root.path(), vec![file("api.txt", &patch)]);
        let mut risky = stop("api", 4, 9);
        risky.risk = Some(Risk {
            kind: RiskKind::ErrorHandling,
            reason: "Error path changed".into(),
        });
        app.install_guide(guide(&app, vec![risky])).unwrap();
        app.enter_guide();
        app.set_diff_wrap(true);
        draw_guide_frame(&mut app, 60, 15);
        app.guide_primary_key(false);
        assert!(app.has_guide_verdict_hint());

        app.page_down(app.diff_state.viewport_height);
        let detour = draw_guide_frame(&mut app, 60, 15);
        assert!(detour.contains("Off stop"), "{detour}");
        assert!(!detour.contains("Explicit verdict needed"), "{detour}");

        app.return_to_guide_stop();
        let landed = draw_guide_frame(&mut app, 60, 15);
        assert!(
            landed
                .lines()
                .any(|row| row.contains('▶') && row.contains("STOPCODE one")),
            "{landed}"
        );
        assert!(!landed.contains("Off stop"), "{landed}");
    }

    #[test]
    fn guide_map_wrapped_block_and_full_risk_survive_layout_and_dock_changes() {
        use crate::input::Action;

        let root = tempfile::tempdir().unwrap();
        let patch = concat!(
            "@@ -1,3 +1,6 @@\n",
            " def quote_order(order, postcode, config):\n",
            "   The incoming checkout arrives from the narrow storefront preview with a serialized postcode.\n",
            "   The subtotal is derived from items, prices, discounts and any item-level conditions.\n",
            "+STOPCODE calculates the purchase total before delivery using the serialized postcode.\n",
            "+   pre-delivery contractual configuration remains visible across wrapped continuation rows.\n",
            "+   return computed_checkout_total\n",
        );
        let mut app = review(root.path(), vec![file("api.txt", patch)]);
        let mut stop = stop("api", 4, 6);
        stop.claim = "Customers see a checkout quote before delivery and keep the existing lookup for overseas orders.".into();
        stop.explanation = "The configuration already belongs to the caller. The same subtotal is reused for receipts. Nothing is written until the payment provider accepts the transaction. The provider receives the exact subtotal from the quote. The checkout shows a receipt when it returns.".into();
        stop.risk = Some(Risk {
            kind: RiskKind::ErrorHandling,
            reason: "An absent postcode currently reaches the fallback used by overseas customers. The caller should not have to submit twice. Confirm the final contract.".into(),
        });
        app.install_guide(guide(&app, vec![stop])).unwrap();
        app.enter_guide();
        app.set_diff_wrap(true);

        for (width, height) in [(150, 42), (100, 30), (80, 24), (60, 15)] {
            for mode in [DiffViewMode::Unified, DiffViewMode::SideBySide] {
                if app.diff_view_mode != mode {
                    app.toggle_diff_view_mode();
                }
                for dock_open in [false, true] {
                    if dock_open {
                        app.open_companion(false);
                        assert_eq!(app.input_mode, InputMode::Companion);
                    }
                    let screen = draw_guide_frame(&mut app, width, height);
                    assert!(
                        screen
                            .lines()
                            .any(|row| row.contains('▶') && row.contains("STOPCODE")),
                        "{mode:?} {dock_open} {width}×{height}\n{screen}"
                    );
                    assert!(
                        screen.contains("contract."),
                        "{mode:?} {dock_open} {width}×{height}\n{screen}"
                    );
                    if width == 150 && !dock_open {
                        assert!(
                            screen.lines().any(|row| row
                                .chars()
                                .skip(40)
                                .collect::<String>()
                                .trim_end()
                                .ends_with('…')),
                            "{screen}"
                        );
                        assert!(!screen.contains("checkout shows a receipt"), "{screen}");
                    }
                    if width == 80 && mode == DiffViewMode::Unified && !dock_open {
                        assert!(
                            screen.contains("delivery using the serialized postcode."),
                            "{screen}"
                        );
                    }
                    if dock_open {
                        assert!(screen.contains("Question:"), "{screen}");
                        app.handle_companion_action(Action::ExitMode);
                    }
                }
            }
        }
    }

    #[test]
    fn guide_map_ask_dock_reserves_only_a_preview_of_the_stop() {
        use std::fmt::Write as _;

        let root = tempfile::tempdir().unwrap();
        let mut patch = String::from("@@ -0,0 +1,45 @@\n");
        for line in 1..=45 {
            writeln!(
                patch,
                "+stop {line:02} code begins here and has enough words to wrap across a narrow viewport"
            )
            .unwrap();
        }
        for stop_len in [3, 6, 45] {
            let mut app = review(root.path(), vec![file("api.txt", &patch)]);
            app.install_guide(guide(&app, vec![stop("api", 1, stop_len)]))
                .unwrap();
            app.enter_guide();
            app.open_companion(false);
            for (pane, width, wrap) in [
                (53, 70, false),
                (30, 24, true),
                (11, 35, false),
                (9, 24, true),
            ] {
                app.set_diff_wrap(wrap);
                let preview = app.guide_preview_height(width, pane);
                let first = app.guide_first_row_height(width);
                assert_eq!(preview, (first * stop_len.min(6) as u16).min(pane));
                let preferred = app.companion_preferred_height(pane);
                let dock = super::super::companion::fit_companion_height(
                    pane,
                    preferred,
                    first,
                    preview.saturating_add(2),
                );
                assert_eq!(
                    dock,
                    match pane {
                        53 => 18,
                        30 => 10,
                        11 if stop_len == 3 => 6,
                        11 => 5,
                        _ => (pane - first).clamp(2, 5),
                    },
                    "{pane} rows, {stop_len} lines, wrap={wrap}",
                );
            }
        }
    }

    #[test]
    fn guide_map_groups_stories_and_clicks_the_second_line_of_a_stop() {
        let root = tempfile::tempdir().unwrap();
        let mut app = review(
            root.path(),
            vec![
                file("before.txt", REPLACEMENT),
                file("api.txt", REPLACEMENT),
            ],
        );
        let mut next = stop("api", 2, 2);
        next.claim = "The API reuses the existing quote when an overseas customer returns to finish the order".into();
        let mut guide = guide(&app, vec![stop("before", 2, 2)]);
        guide.summary = "The guide summary is not part of the map UI.".into();
        guide.stories[0].summary = "First story context belongs in the detail view.".into();
        guide.stories.push(Story {
            title: "Keep the existing quote".into(),
            summary: "Second story context belongs in the detail view.".into(),
            stops: vec![next],
        });
        guide.estimate_minutes = Some(crate::review_guide::SITTING_MINUTES + 1);
        app.install_guide(guide).unwrap();
        app.enter_guide();

        let screen = draw_guide_frame(&mut app, 125, 55);
        assert!(
            screen.contains("2 stops · ~") && screen.contains("2 of 2 hunks"),
            "{screen}"
        );
        assert!(
            screen.contains("stop 1 of 1 · story 1 of 2 · 2 left"),
            "{screen}"
        );
        assert!(
            screen.contains("Long: now · approve verified · split"),
            "{screen}"
        );
        assert!(
            !screen.contains("belongs in the detail view.")
                && !screen.contains("part of the map UI.")
        );
        let rows: Vec<_> = screen
            .lines()
            .map(|row| row.chars().skip(1).take(38).collect::<String>())
            .collect();
        let first = rows
            .iter()
            .position(|row| row.contains("Walk the change"))
            .unwrap();
        let second = rows
            .iter()
            .position(|row| row.contains("Keep the existing quote"))
            .unwrap();
        assert!(rows[first - 1].trim().is_empty());
        assert!(rows[second - 1].trim().is_empty());
        assert_eq!(
            app.review_map
                .map_hits
                .iter()
                .filter(|entry| **entry == Some(MapEntry::Stop(1)))
                .count(),
            1,
            "inactive stops use one row"
        );

        app.guide_primary_key(false);
        let screen = draw_guide_frame(&mut app, 125, 55);
        assert!(!screen.contains("approve verified"), "{screen}");
        assert!(screen.contains("Walk the change ✓ 1/1"), "{screen}");
        assert!(!screen.contains("Confirm before"), "{screen}");
        let area = app.review_map.map_area.unwrap();
        let row = |entry| {
            app.review_map
                .map_hits
                .iter()
                .enumerate()
                .filter(move |(_, hit)| **hit == Some(entry))
                .map(|(index, _)| index)
        };
        let first_hit = row(MapEntry::Story(0)).next().unwrap();
        let hit = row(MapEntry::Stop(1))
            .nth(1)
            .expect("current stop has a continuation row");
        let line = screen.lines().nth(usize::from(area.y) + hit).unwrap();
        assert!(
            line.chars()
                .skip(1)
                .take(38)
                .collect::<String>()
                .starts_with("     ")
        );
        assert!(line.chars().take(39).any(|ch| ch == '…'));
        // Click the previous stop and then the current stop's continuation.
        // Clicking changes map selection, not the current stop or its wrapping.
        assert!(app.click_guide_map(ratatui::layout::Position::new(
            area.x + 5,
            area.y + first_hit as u16
        )));
        assert!(matches!(
            app.guide_map_selection().0,
            Some(MapEntry::Story(0))
        ));
        assert!(app.click_guide_map(ratatui::layout::Position::new(
            area.x + 5,
            area.y + hit as u16
        )));
        assert!(matches!(
            app.guide_map_selection().0,
            Some(MapEntry::Stop(1))
        ));
        app.enter_guide_map_selection();
        assert_eq!(current(&app), Some("api"));
        app.open_guide_details();
        let screen = draw_guide_frame(&mut app, 125, 55);
        assert!(
            screen.contains("Second story context belongs in the detail view."),
            "{screen}"
        );
    }

    #[test]
    fn guide_map_risk_mark_disappears_only_after_a_decision() {
        let root = tempfile::tempdir().unwrap();
        let mut app = review(root.path(), vec![file("api.txt", REPLACEMENT)]);
        let mut risky = stop("api", 2, 2);
        risky.claim = "Risky stop".into();
        risky.untested = true;
        app.install_guide(guide(&app, vec![risky])).unwrap();
        app.enter_guide();

        for (verdict, marked) in [
            (None, true),
            (Some(Verdict::Parked), true),
            (Some(Verdict::ChecksOut), false),
            (Some(Verdict::Wrong), false),
        ] {
            app.update_guide_progress(|progress| {
                if let Some(verdict) = verdict {
                    progress.verdicts.insert(
                        "api".into(),
                        VerdictRecord {
                            verdict,
                            block: vec!["after".into()],
                            comment_id: (verdict == Verdict::Wrong).then(|| "wrong-comment".into()),
                            at: Utc::now(),
                        },
                    );
                }
            })
            .unwrap();
            let screen = draw_guide_frame(&mut app, 60, 15);
            let row = screen
                .lines()
                .find(|row| row.contains('▏'))
                .expect("current stop is visible");
            assert_eq!(
                row.chars().nth(4) == Some('!'),
                marked,
                "{verdict:?}: {row}"
            );
        }
    }

    #[test]
    fn guide_map_and_card_render_the_same_emphasis_hierarchy_in_light_and_dark() {
        use ratatui::buffer::{Buffer, Cell};
        use ratatui::style::Modifier;

        fn cell_for<'a>(buffer: &'a Buffer, range: std::ops::Range<u16>, text: &str) -> &'a Cell {
            (0..buffer.area.height)
                .find_map(|y| {
                    let row = range
                        .clone()
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>();
                    let offset = row[..row.find(text)?].chars().count() as u16;
                    Some(&buffer[(range.start + offset, y)])
                })
                .unwrap_or_else(|| panic!("missing {text:?}"))
        }

        for theme in [Theme::dark(), Theme::light()] {
            let root = tempfile::tempdir().unwrap();
            let mut app = review(
                root.path(),
                vec![
                    file("before.txt", REPLACEMENT),
                    file("api.txt", REPLACEMENT),
                ],
            );
            app.set_theme(theme.clone());
            let mut first = stop("before", 2, 2);
            first.claim = "Earlier verified statement".into();
            let mut next = stop("api", 2, 2);
            next.claim = "Current risk claim".into();
            next.explanation = "Explanation evidence is secondary.".into();
            next.untested = true;
            let mut later = stop("later", 3, 3);
            later.path = "api.txt".into();
            later.claim = "Later unchecked clause".into();
            let mut guide = guide(&app, vec![first, next]);
            guide.stories[0].title = "Active story".into();
            guide.stories.push(Story {
                title: "Later story".into(),
                summary: String::new(),
                stops: vec![later],
            });
            app.install_guide(guide).unwrap();
            app.enter_guide();
            app.guide_primary_key(false);
            let buffer = guide_frame_buffer(&mut app, 125, 55);
            let sidebar = app.file_list_area.unwrap();
            let map = sidebar.left() + 1..sidebar.right() - 1;
            let card = sidebar.right()..buffer.area.right();
            let style = |range, text, fg, bold| {
                let cell = cell_for(&buffer, range, text);
                assert_eq!(cell.fg, fg, "{text}");
                assert_eq!(cell.modifier.contains(Modifier::BOLD), bold, "{text}");
            };

            style(map.clone(), "3 stops", theme.fg_dim, false);
            style(map.clone(), "stop 2 of 2", theme.fg_dim, false);
            style(map.clone(), "Later story", theme.fg_secondary, false);
            style(
                map.clone(),
                "Later unchecked clause",
                theme.fg_secondary,
                false,
            );
            style(
                map.clone(),
                "Earlier verified statement",
                theme.fg_dim,
                false,
            );
            style(map.clone(), "Active story", theme.fg_primary, true);
            style(map.clone(), "Current risk claim", theme.fg_primary, true);
            assert_eq!(
                cell_for(&buffer, map.clone(), "▏").fg,
                theme.selection_marker.unwrap_or(theme.border_focused)
            );
            assert_ne!(cell_for(&buffer, map.clone(), "✓").fg, theme.fg_dim);
            assert_eq!(cell_for(&buffer, map, "!").fg, theme.pending);

            style(card.clone(), "Current risk claim", theme.fg_primary, true);
            style(card.clone(), "api.txt:2", theme.fg_dim, false);
            style(
                card.clone(),
                "Explanation evidence",
                theme.fg_secondary,
                false,
            );
            style(
                card.clone(),
                "Untested: no covering test found",
                theme.pending,
                false,
            );
            style(card.clone(), "K checks out", theme.fg_secondary, false);
            style(card, "checks out", theme.fg_dim, false);
        }
    }

    #[test]
    fn guide_spotlight_preserves_block_styles_and_mutes_only_the_surroundings() {
        use crate::model::{Comment, CommentType};
        use ratatui::{
            buffer::{Buffer, Cell},
            style::Color,
        };

        fn cell_at(buffer: &Buffer, app: &App, ann: usize, symbol: &str) -> Cell {
            let area = app.diff_inner_area.unwrap();
            app.diff_row_to_annotation
                .iter()
                .enumerate()
                .filter(|(_, idx)| **idx == ann)
                .flat_map(|(row, _)| {
                    (area.x..area.right()).map(move |x| &buffer[(x, area.y + row as u16)])
                })
                .find(|cell| cell.symbol() == symbol)
                .unwrap_or_else(|| panic!("missing {symbol:?} on row {ann}\n{buffer:?}"))
                .clone()
        }

        let patch = concat!(
            "@@ -1,4 +1,7 @@\n",
            " BEFORE_CONTEXT\n",
            "+B Active stop uses a long text literal with many words so this source line wraps across the old and new viewport styles\n",
            "+OUTSIDE_ADD\n",
            "+AFTER_ADD\n",
            " SEARCH_CONTEXT\n",
            " DETOUR_CONTEXT\n",
            " TRAILING_CONTEXT\n",
        );
        let dark_ink = (Color::Rgb(211, 208, 200), Color::Rgb(131, 129, 126));
        let light_ink = (Color::Rgb(79, 91, 102), Color::Rgb(102, 112, 120));
        for (theme, (normal_ink, muted_ink)) in [
            (Theme::dark(), dark_ink),
            (Theme::light(), light_ink),
            (Theme::dark().with_transparent_background(), dark_ink),
            (Theme::light().with_transparent_background(), light_ink),
        ] {
            for mode in [DiffViewMode::Unified, DiffViewMode::SideBySide] {
                for wrap in [false, true] {
                    let root = tempfile::tempdir().unwrap();
                    let mut app = review(root.path(), vec![file("api.txt", patch)]);
                    app.set_theme(theme.clone());
                    app.cursor_line_highlight = true;
                    app.search_highlight_enabled = true;
                    for (line, body) in [(2, "Σ inside comment"), (3, "Ω outside comment")] {
                        app.session
                            .get_file_mut(&PathBuf::from("api.txt"))
                            .unwrap()
                            .add_line_comment(
                                line,
                                Comment::new(body.into(), CommentType::from_id("note"), None),
                            );
                    }
                    app.rebuild_annotations();
                    if mode != app.diff_view_mode {
                        app.toggle_diff_view_mode();
                    }
                    app.set_diff_wrap(wrap);
                    app.install_guide(guide(&app, vec![stop("api", 2, 2)]))
                        .unwrap();
                    app.enter_guide();
                    app.show_file_list = false; // Same code geometry with and without the guide.
                    app.search_buffer = "SEARCH".into();
                    assert!(app.search_in_diff_from_cursor());
                    app.return_to_guide_stop();
                    let active = guide_frame_buffer(&mut app, 100, 45);
                    let block = app.diff_state.cursor_line;
                    let row = |app: &App, line| {
                        app.line_annotations.iter().position(|row| {
                        matches!(row, AnnotatedLine::DiffLine { new_lineno: Some(num), .. }
                            | AnnotatedLine::SideBySideLine { new_lineno: Some(num), .. } if *num == line)
                    }).unwrap()
                    };
                    let comment = |app: &App, line| {
                        app.line_annotations.iter().enumerate()
                        .filter(|(_, row)| matches!(row, AnnotatedLine::LineComment { line: num, .. } if *num == line))
                        .nth(1).unwrap().0
                    };
                    let context = row(&app, 1);
                    let added = row(&app, 3);
                    let search = row(&app, 5);
                    let detour = row(&app, 6);
                    let outside_comment = comment(&app, 3);
                    let inside_comment = comment(&app, 2);
                    let first_visible = app.diff_state.scroll_offset;
                    let muted_context = cell_at(&active, &app, context, "B");
                    let block_cells = |buffer: &Buffer, app: &App| {
                        let area = app.diff_inner_area.unwrap();
                        let pane = app.pane_geometry(area, LineSide::New);
                        app.diff_row_to_annotation
                            .iter()
                            .enumerate()
                            .filter(|(_, index)| **index == block)
                            .flat_map(|(row, _)| {
                                (pane.content_x_start..pane.content_x_end)
                                    .map(move |x| buffer[(x, area.y + row as u16)].clone())
                            })
                            .collect::<Vec<_>>()
                    };
                    let selected_block = block_cells(&active, &app);
                    assert_eq!(cell_at(&active, &app, block, "B").bg, theme.cursor_line_bg);
                    assert_eq!(
                        cell_at(&active, &app, search, "S").bg,
                        theme.search_match_bg
                    );
                    let muted_add = cell_at(&active, &app, added, "O");
                    let muted_comment = cell_at(&active, &app, outside_comment, "│");
                    let normal_comment = cell_at(&active, &app, inside_comment, "│");

                    let point = SelPoint {
                        annotation_idx: added,
                        char_offset: 0,
                        side: LineSide::New,
                    };
                    app.visual_selection = Some(VisualSelection {
                        anchor: point,
                        head: SelPoint {
                            char_offset: 1,
                            ..point
                        },
                    });
                    let selected = guide_frame_buffer(&mut app, 100, 45);
                    assert_eq!(cell_at(&selected, &app, added, "O").bg, theme.bg_highlight);
                    assert_eq!(
                        cell_at(&selected, &app, added, "U"),
                        cell_at(&active, &app, added, "U")
                    );
                    app.visual_selection = None;

                    app.move_cursor_to_annotation(detour);
                    app.diff_state.scroll_offset = first_visible;
                    let wandering = guide_frame_buffer(&mut app, 100, 45);
                    let normal_context = cell_at(&wandering, &app, context, "B");
                    assert_eq!(normal_context.fg, normal_ink, "{mode:?}, wrap {wrap}");
                    assert_eq!(muted_context.fg, muted_ink, "{mode:?}, wrap {wrap}");
                    let normal_add = cell_at(&wandering, &app, added, "O");
                    assert_ne!(muted_add.fg, normal_add.fg);
                    assert_ne!(muted_add.bg, normal_add.bg);
                    assert_ne!(
                        muted_comment.fg,
                        cell_at(&wandering, &app, outside_comment, "│").fg
                    );
                    assert_eq!(
                        normal_comment,
                        cell_at(&wandering, &app, inside_comment, "│")
                    );

                    app.return_to_guide_stop();
                    let returned = guide_frame_buffer(&mut app, 100, 45);
                    assert_eq!(cell_at(&returned, &app, context, "B"), muted_context);
                    app.pause_guide();
                    let paused = guide_frame_buffer(&mut app, 100, 45);
                    assert_eq!(cell_at(&paused, &app, context, "B"), normal_context);
                    assert_eq!(block_cells(&paused, &app), selected_block);
                }
            }
        }
    }
}
