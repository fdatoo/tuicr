use super::*;
use crate::review_guide::{HunkRef, SpotCheck};

const SAMPLE_SIZE: usize = 5;

/// Rank hunks by a guide-seeded hash. Keep only the best five; no allocation
/// proportional to the supporting pile and no dependency on group order.
fn spot_sample(guide: &ReviewGuide) -> Vec<HunkRef> {
    let mut seed = Sha256::new();
    digest_field(&mut seed, b"tuicr-spot-check-v1");
    digest_field(&mut seed, guide.diff.as_bytes());
    let mut sample = Vec::with_capacity(SAMPLE_SIZE);
    for group in &guide.supporting {
        for hunk in &group.hunks {
            if (hunk.old_start == 0 && hunk.new_start == 0)
                || sample.iter().any(|(_, picked)| *picked == hunk)
            {
                continue;
            }
            let mut hash = seed.clone();
            digest_field(&mut hash, hunk.path.as_bytes());
            hash.update(hunk.old_start.to_le_bytes());
            hash.update(hunk.new_start.to_le_bytes());
            let rank = ((group.basis == Basis::Mechanical, hash.finalize()), hunk);
            let index = sample.partition_point(|picked| picked < &rank);
            if index < SAMPLE_SIZE {
                if sample.len() == SAMPLE_SIZE {
                    sample.pop();
                }
                sample.insert(index, rank);
            }
        }
    }
    sample.into_iter().map(|(_, hunk)| hunk.clone()).collect()
}

pub(super) fn prepare_spot_sample(
    guide: &ReviewGuide,
    progress: &mut Progress,
) -> (Vec<HunkRef>, bool) {
    let sample = spot_sample(guide);
    let previous_len = progress.spot_checks.len();
    if progress.spot_diff.as_deref() == Some(guide.diff.as_str()) {
        let mut seen = [false; SAMPLE_SIZE];
        progress.spot_checks.retain(|spot| {
            sample
                .iter()
                .position(|hunk| hunk == &spot.hunk)
                .is_some_and(|index| !std::mem::replace(&mut seen[index], true))
        });
    } else {
        progress.spot_checks.clear();
    }
    (sample, progress.spot_checks.len() != previous_len)
}

pub(super) struct SpotWalk {
    index: usize,
    anchor: ResolvedStop,
}

impl App {
    pub(crate) fn start_spot_check(&mut self) {
        if !self.guide_review_ready() {
            self.set_warning("Open a loaded review before starting a spot-check");
            return;
        }
        let Some(map) = self.installed_map() else {
            self.set_message("No review guide — use :prepare or start with --guide PATH");
            return;
        };
        if self.installed_guide_is_stale() {
            self.set_warning("Spot-checks need a guide for this diff — use :prepare");
            return;
        }
        if map.spot_sample.is_empty() {
            self.set_message("No supporting hunks to spot-check");
            return;
        }
        self.advance_spot_check();
    }

    pub(crate) fn spot_position(&self) -> Option<(usize, usize)> {
        let map = self.installed_map()?;
        Some((
            self.review_map.spot_walk.as_ref()?.index + 1,
            map.spot_sample.len(),
        ))
    }

    pub(super) fn spot_anchor(&self) -> Option<&ResolvedStop> {
        self.review_map.spot_walk.as_ref().map(|walk| &walk.anchor)
    }

    pub(crate) fn stop_spot_check(&mut self) {
        if self.review_map.spot_walk.take().is_none() {
            return;
        }
        if self.installed_map().is_some_and(|map| !map.active) {
            self.guide_focus_lost();
        }
        self.review_map.pending_landing = false;
    }

    pub(super) fn land_current_spot(&mut self) {
        if let Some(walk) = self.review_map.spot_walk.as_ref() {
            self.land_spot_at(walk.index);
        }
    }

    fn advance_spot_check(&mut self) {
        let Some(map) = self.installed_map() else {
            return;
        };
        let next = map.spot_sample.iter().position(|hunk| {
            !map.progress
                .spot_checks
                .iter()
                .any(|spot| &spot.hunk == hunk)
        });
        if let Some(index) = next {
            self.land_spot_at(index);
        } else {
            let checks = map.progress.spot_checks.iter();
            let passed = checks
                .clone()
                .filter(|spot| spot.verdict == Verdict::ChecksOut)
                .count();
            let wrong = checks.filter(|spot| spot.verdict == Verdict::Wrong).count();
            let parked = map.spot_sample.len() - passed - wrong;
            self.stop_spot_check();
            self.set_message(format!(
                "Spot-check complete: {passed} checked out, {wrong} wrong, {parked} parked"
            ));
        }
    }

    fn land_spot_at(&mut self, index: usize) {
        let Some(target) = self
            .installed_map()
            .and_then(|map| map.spot_sample.get(index))
        else {
            return;
        };
        let Some(anchor) = resolve_spot(target, &self.diff_files) else {
            self.stop_spot_check();
            self.set_warning("Could not locate supporting hunk — use :prepare");
            return;
        };
        let (file, hunk) = (anchor.file, anchor.hunk);
        if !self.file_idx_passes_filter(file) {
            self.stop_spot_check();
            self.set_warning(
                "Spot-check hunk is hidden — reveal the file, then use :spot to resume",
            );
            return;
        }
        if self.should_collapse_file(file) {
            self.reveal_reviewed_file(file);
        }
        if self.should_collapse_hunk(file, hunk) {
            self.reveal_reviewed_hunk(file, hunk);
        }
        self.jump_to_file(file);
        self.rebuild_annotations();
        let Some(row) = self
            .line_annotations
            .iter()
            .position(|annotation| annotation_in_block(annotation, &anchor))
        else {
            self.stop_spot_check();
            self.set_warning("Could not display supporting hunk — use :spot to resume");
            return;
        };
        self.focused_panel = FocusedPanel::Diff;
        self.set_cursor_side(model_side(anchor.side));
        self.diff_state.scroll_x = 0;
        self.move_cursor_to_annotation(row);
        self.diff_state.scroll_offset = self
            .scroll_offset_for_rows_above(row, LANDING_CONTEXT_ROWS)
            .min(self.max_scroll_offset());
        self.review_map.pending_landing = true;
        self.review_map.spot_walk = Some(SpotWalk { index, anchor });
        self.guide_input_activity();
    }

    pub(super) fn open_wrong_spot_comment(&mut self) {
        if !self.guide_on_stop() {
            self.land_current_spot();
            return;
        }
        let Some(anchor) = self.spot_anchor() else {
            return;
        };
        let side = model_side(anchor.side);
        let hunk = &self.diff_files[anchor.file].hunks[anchor.hunk];
        let Some(end) = source_lineno(&hunk.lines[anchor.last_line], side) else {
            return;
        };
        let Some(start) = source_lineno(&hunk.lines[anchor.first_line], side) else {
            return;
        };
        if let Some(row) = self
            .line_annotations
            .iter()
            .rposition(|annotation| annotation_in_block(annotation, anchor))
        {
            self.move_cursor_to_annotation(row);
        }
        self.enter_comment_mode(false, Some((end, side)));
        self.comment_line_range = Some((LineRange::new(start, end), side));
        self.review_map.pending_wrong = Some(self.review_map.publication);
    }

    pub(super) fn record_spot_verdict(&mut self, verdict: Verdict) {
        if verdict != Verdict::Wrong && !self.guide_on_stop() {
            self.land_current_spot();
            return;
        }
        let Some(walk) = self.review_map.spot_walk.as_ref() else {
            return;
        };
        let Some(map) = self.installed_map() else {
            return;
        };
        let hunk = map.spot_sample[walk.index].clone();
        let digest = map.guide.diff.clone();
        if let Err(error) = self.update_guide_progress(|progress| {
            progress.spot_diff = Some(digest);
            progress.spot_checks.push(SpotCheck { hunk, verdict });
        }) {
            self.set_error(format!("Could not save spot-check: {error}"));
            return;
        }
        self.advance_spot_check();
    }
}

fn resolve_spot(target: &HunkRef, files: &[DiffFile]) -> Option<ResolvedStop> {
    let (file, changed) = files.iter().enumerate().find(|(_, file)| {
        !file.is_commit_message && file.display_path().to_string_lossy() == target.path
    })?;
    let (hunk, change) = changed.hunks.iter().enumerate().find(|(_, hunk)| {
        hunk.old_start == target.old_start && hunk.new_start == target.new_start
    })?;
    for (origin, side) in [
        (LineOrigin::Addition, Side::New),
        (LineOrigin::Deletion, Side::Old),
    ] {
        let mut rows = change
            .lines
            .iter()
            .enumerate()
            .filter_map(|(row, line)| (line.origin == origin).then_some(row));
        if let Some(first_line) = rows.next() {
            let last_line = rows.next_back().unwrap_or(first_line);
            return Some(ResolvedStop {
                file,
                hunk,
                first_line,
                last_line,
                side,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review_guide::{GUIDE_VERSION, SupportingGroup};

    #[test]
    fn spot_sample_is_stable_prioritizes_claimed_and_caps_at_five() {
        let group = |path: &str, basis, count| SupportingGroup {
            label: path.into(),
            basis,
            note: String::new(),
            hunks: (1..=count)
                .map(|line| HunkRef {
                    path: path.into(),
                    old_start: line,
                    new_start: line,
                })
                .collect(),
        };
        let mut guide = ReviewGuide {
            version: GUIDE_VERSION,
            diff: "fixture-digest".into(),
            summary: String::new(),
            stories: vec![],
            supporting: vec![
                group("docs", Basis::Mechanical, 100),
                group("code", Basis::Claimed, 3),
            ],
            estimate_minutes: None,
            provenance: Default::default(),
        };
        let first = spot_sample(&guide);
        assert_eq!(first.len(), SAMPLE_SIZE);
        assert!(first[..3].iter().all(|hunk| hunk.path == "code"));
        assert!(first[3..].iter().all(|hunk| hunk.path == "docs"));
        assert_eq!(first.iter().collect::<HashSet<_>>().len(), SAMPLE_SIZE);
        guide.supporting.reverse();
        guide.supporting[0].hunks.reverse();
        assert_eq!(spot_sample(&guide), first);
        guide.supporting.push(group("wiring", Basis::Claimed, 6));
        let claimed_only = spot_sample(&guide);
        assert_eq!(claimed_only.len(), SAMPLE_SIZE);
        assert!(claimed_only.iter().all(|hunk| hunk.path != "docs"));
    }
}
