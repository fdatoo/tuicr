use std::fmt::Write as _;

use super::*;
use crate::review_guide::resolve::ResolvedStop;
use crate::review_guide::{Basis, Progress, ReviewGuide, Stop, Verdict};

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct VerdictCounts {
    pub verified: usize,
    pub wrong: usize,
    pub parked: usize,
    pub unchecked: usize,
}

impl VerdictCounts {
    fn add(&mut self, verdict: Option<Verdict>) {
        match verdict {
            Some(Verdict::ChecksOut) => self.verified += 1,
            Some(Verdict::Wrong) => self.wrong += 1,
            Some(Verdict::Parked) => self.parked += 1,
            None => self.unchecked += 1,
        }
    }

    pub(crate) fn total(&self) -> usize {
        self.verified + self.wrong + self.parked + self.unchecked
    }

    pub(crate) fn decided(&self) -> usize {
        self.total() - self.unchecked
    }

    fn aggregate(&self) -> Option<Verdict> {
        if self.wrong > 0 {
            Some(Verdict::Wrong)
        } else if self.parked > 0 {
            Some(Verdict::Parked)
        } else if self.unchecked > 0 || self.verified == 0 {
            None
        } else {
            Some(Verdict::ChecksOut)
        }
    }
}

pub(crate) struct StoryCoverage<'a> {
    pub title: &'a str,
    pub stops: VerdictCounts,
}

#[derive(Default)]
pub(crate) struct GroupCoverage<'a> {
    pub label: &'a str,
    pub dismissed: bool,
    pub total: usize,
    pub reviewed: usize,
    pub dismissed_hunks: usize,
    pub spot_verdicts: usize,
}

impl GroupCoverage<'_> {
    pub(crate) fn unreviewed(&self) -> usize {
        self.total - self.reviewed - self.dismissed_hunks
    }
}

#[derive(Default)]
pub(crate) struct Coverage<'a> {
    pub stories: Vec<StoryCoverage<'a>>,
    pub story_totals: VerdictCounts,
    pub stops: VerdictCounts,
    pub risk_verified: usize,
    pub risk_unverified: usize,
    pub guide_hunks: usize,
    pub changed_hunks: usize,
    pub groups: Vec<GroupCoverage<'a>>,
    pub unassigned: GroupCoverage<'a>,
    pub spot_checks: VerdictCounts,
    pub file_marks: usize,
    pub hunk_marks: usize,
    pub elapsed_secs: u64,
    pub stale: bool,
}

impl Coverage<'_> {
    pub(crate) fn reach_label(&self) -> String {
        format!(
            "covers {} of {} hunks",
            self.guide_hunks, self.changed_hunks
        )
    }

    pub(crate) fn supporting_total(&self) -> usize {
        self.groups.iter().map(|group| group.total).sum()
    }

    fn dismissed_hunks(&self) -> usize {
        self.groups.iter().map(|group| group.dismissed_hunks).sum()
    }

    pub(crate) fn supporting_unreviewed(&self) -> usize {
        self.groups
            .iter()
            .map(GroupCoverage::unreviewed)
            .sum::<usize>()
            + self.unassigned.unreviewed()
    }

    pub(crate) fn complete(&self) -> bool {
        !self.stale
            && self.stops.verified == self.stops.total()
            && self.supporting_unreviewed() == 0
    }

    pub(crate) fn compact_summary(&self) -> [String; 3] {
        [
            format!(
                "Stops verified: {}/{} · Stories verified: {}/{}",
                self.stops.verified,
                self.stops.total(),
                self.story_totals.verified,
                self.stories.len()
            ),
            format!(
                "Risk items verified: {} · Unverified: {}",
                self.risk_verified, self.risk_unverified
            ),
            format!(
                "Supporting: {} unreviewed · {} dismissed · {} spot-checked",
                self.supporting_unreviewed(),
                self.dismissed_hunks(),
                self.spot_checks.verified + self.spot_checks.wrong
            ),
        ]
    }

    /// Deliberately use only counts and story numbers, never guide prose or Ask content.
    pub(crate) fn note(&self) -> String {
        let mut note = format!(
            "Reviewed with a guide: verified {} of {} stops",
            self.stops.verified,
            self.stops.total()
        );
        if self.story_totals.verified > 0 {
            let _ = write!(note, " ({})", story_numbers(&self.stories, true));
        }
        let risks = self.risk_verified + self.risk_unverified;
        if risks > 0 {
            let label = if risks == 1 {
                "risk item"
            } else {
                "risk items"
            };
            let _ = write!(note, ", {} of {risks} {label} verified", self.risk_verified);
        }
        let supporting_total = self.supporting_total();
        if supporting_total > 0 {
            let label = if supporting_total == 1 {
                "hunk"
            } else {
                "hunks"
            };
            let _ = write!(
                note,
                ", spot-checked {} of {supporting_total} supporting {label}",
                self.spot_checks.verified + self.spot_checks.wrong
            );
        }
        let dismissed = self.dismissed_hunks();
        if dismissed > 0 {
            let label = if dismissed == 1 { "hunk" } else { "hunks" };
            let _ = write!(
                note,
                ", dismissed {dismissed} mechanical supporting {label}"
            );
        }
        note.push('.');
        let _ = write!(note, " Guide {}.", self.reach_label());
        let mut outstanding = Vec::new();
        if self.story_totals.verified < self.stories.len() {
            outstanding.push(story_numbers(&self.stories, false));
        }
        let supporting = self.supporting_unreviewed();
        if supporting > 0 {
            let label = if supporting == 1 { "hunk" } else { "hunks" };
            outstanding.push(format!("{supporting} supporting {label}"));
        }
        if !outstanding.is_empty() {
            let _ = write!(note, " Not verified: {}.", outstanding.join(", "));
        }
        if self.stops.wrong > 0 || self.stops.parked > 0 {
            let _ = write!(
                note,
                " Stop verdicts: {} wrong, {} parked.",
                self.stops.wrong, self.stops.parked
            );
        }
        if self.stale {
            note.push_str(" The guide is for an earlier diff.");
        }
        note
    }

    pub(crate) fn approval_risk_count(&self, event: crate::forge::submit::SubmitEvent) -> usize {
        if event == crate::forge::submit::SubmitEvent::Approve {
            self.risk_unverified
        } else {
            0
        }
    }
}

fn story_numbers(stories: &[StoryCoverage<'_>], verified: bool) -> String {
    let mut indices = stories
        .iter()
        .enumerate()
        .filter_map(|(index, story)| {
            ((story.stops.aggregate() == Some(Verdict::ChecksOut)) == verified).then_some(index + 1)
        })
        .peekable();
    let Some(first) = indices.next() else {
        return String::new();
    };
    if indices.peek().is_none() {
        return format!("story {first}");
    }
    let mut numbers = String::from("stories ");
    let mut start = first;
    loop {
        let mut end = start;
        while indices.next_if_eq(&(end + 1)).is_some() {
            end += 1;
        }
        let _ = write!(numbers, "{start}");
        if end != start {
            let _ = write!(numbers, "–{end}");
        }
        let Some(next) = indices.next() else {
            return numbers;
        };
        numbers.push_str(", ");
        start = next;
    }
}

fn coverage<'a, 'b>(
    guide: &'a ReviewGuide,
    progress: &Progress,
    anchors: impl Iterator<Item = (&'a Stop, Option<&'b ResolvedStop>)>,
    session: &ReviewSession,
    files: &[DiffFile],
    stale: bool,
) -> Coverage<'a> {
    let mut record = Coverage {
        elapsed_secs: progress.elapsed_secs,
        stale,
        ..Coverage::default()
    };
    let mut stopped = HashSet::new();
    let mut anchors = anchors.map(|(stop, anchor)| {
        if let Some(anchor) = anchor {
            stopped.insert((anchor.file, anchor.hunk));
        }
        anchor
            .and_then(|_| progress.verdicts.get(&stop.id))
            .map(|record| record.verdict)
    });
    for story in &guide.stories {
        let mut stops = VerdictCounts::default();
        for stop in &story.stops {
            let verdict = anchors.next().flatten();
            record.stops.add(verdict);
            stops.add(verdict);
            if stop.risk.is_some() {
                if verdict == Some(Verdict::ChecksOut) {
                    record.risk_verified += 1;
                } else {
                    record.risk_unverified += 1;
                }
            }
        }
        record.story_totals.add(stops.aggregate());
        record.stories.push(StoryCoverage {
            title: &story.title,
            stops,
        });
    }

    let mut group_for = HashMap::new();
    let mut spot_for = HashMap::new();
    if !stale {
        for group in &guide.supporting {
            for hunk in &group.hunks {
                group_for.insert(
                    (hunk.path.as_str(), hunk.old_start, hunk.new_start),
                    record.groups.len(),
                );
            }
            record.groups.push(GroupCoverage {
                label: &group.label,
                dismissed: group.basis == Basis::Mechanical
                    && progress.dismissed_groups.contains(&group.label),
                ..GroupCoverage::default()
            });
        }
        if progress.spot_diff.as_deref() == Some(guide.diff.as_str()) {
            for spot in &progress.spot_checks {
                spot_for.insert(
                    (
                        spot.hunk.path.as_str(),
                        spot.hunk.old_start,
                        spot.hunk.new_start,
                    ),
                    spot.verdict,
                );
            }
        }
    }

    for (file_idx, file) in files
        .iter()
        .enumerate()
        .filter(|(_, file)| !file.is_commit_message)
    {
        let marks = session.files.get(file.display_path());
        let file_mark = marks.is_some_and(|marks| marks.reviewed);
        record.file_marks += usize::from(file_mark);
        let hunk_marks = marks
            .filter(|marks| !marks.reviewed_hunks.is_empty())
            .map(|marks| (marks, file.hunk_review_keys()));
        let path = file.display_path().to_string_lossy();
        let targets = file
            .hunks
            .iter()
            .enumerate()
            .filter_map(|(idx, hunk)| {
                hunk.lines
                    .iter()
                    .any(|line| line.origin != LineOrigin::Context)
                    .then_some((Some(idx), hunk.old_start, hunk.new_start))
            })
            .chain(file.hunks.is_empty().then_some((None, 0, 0)));
        for (index, old, new) in targets {
            record.changed_hunks += usize::from(index.is_some());
            let hunk_mark = index.is_some_and(|index| {
                hunk_marks
                    .as_ref()
                    .is_some_and(|(marks, keys)| marks.reviewed_hunks.contains(&keys[index]))
            });
            record.hunk_marks += usize::from(hunk_mark);
            if index.is_some_and(|index| stopped.contains(&(file_idx, index))) {
                record.guide_hunks += 1;
                continue;
            }
            let key = (path.as_ref(), old, new);
            let group = match group_for.get(&key) {
                Some(&index) => &mut record.groups[index],
                None => &mut record.unassigned,
            };
            group.total += 1;
            let spot = spot_for.get(&key).copied();
            group.spot_verdicts += usize::from(spot.is_some());
            if let Some(spot) = spot {
                record.spot_checks.add(Some(spot));
            }
            match spot {
                Some(Verdict::ChecksOut) => group.reviewed += 1,
                Some(Verdict::Wrong | Verdict::Parked) => {}
                None if file_mark || hunk_mark => group.reviewed += 1,
                None if group.dismissed => group.dismissed_hunks += 1,
                None => {}
            }
        }
    }
    record
}

#[derive(Default)]
pub(crate) struct CoverageView {
    pub(crate) scroll: usize,
    pub(crate) max_scroll: usize,
}

impl App {
    pub(crate) fn coverage_record(&self) -> Option<Coverage<'_>> {
        Some(coverage(
            self.installed_guide()?,
            self.guide_progress()?,
            self.guide_resolved_stops(),
            &self.session,
            &self.diff_files,
            self.installed_guide_is_stale(),
        ))
    }

    pub(crate) fn open_coverage(&mut self) {
        if self.installed_guide().is_some() {
            self.coverage_view = CoverageView::default();
            self.input_mode = InputMode::Coverage;
        } else {
            self.set_message("No review guide — use :prepare or start with --guide PATH");
        }
    }

    pub(crate) fn scroll_coverage(&mut self, delta: isize) {
        self.coverage_view.scroll = self
            .coverage_view
            .scroll
            .saturating_add_signed(delta)
            .min(self.coverage_view.max_scroll);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::submit::SubmitEvent;
    use crate::model::FilePatch;
    use crate::review_guide::resolve::{hunk_ref, resolve_stop};
    use crate::review_guide::{
        GUIDE_VERSION, Risk, RiskKind, Side, SpotCheck, Story, SupportingGroup, VerdictRecord,
    };

    fn file(path: &str) -> DiffFile {
        crate::vcs::diff_parser::parse_file_patches(vec![FilePatch::new(
            Some(path.into()),
            Some(path.into()),
            FileStatus::Modified,
            "@@ -1 +1 @@\n-before\n+after\n",
        )])
        .unwrap()
        .pop()
        .unwrap()
    }

    fn stop(path: &str, risk: bool) -> Stop {
        Stop {
            id: path.into(),
            path: path.into(),
            claim: "Check changed value".into(),
            explanation: String::new(),
            side: Side::New,
            start_line: 1,
            end_line: 1,
            symbol: None,
            test: None,
            untested: false,
            audit: Default::default(),
            followups: vec![],
            risk: risk.then(|| Risk {
                kind: RiskKind::Other,
                reason: "Check this independently".into(),
            }),
        }
    }

    fn guide(stories: Vec<Story>) -> ReviewGuide {
        ReviewGuide {
            version: GUIDE_VERSION,
            diff: "fixture".into(),
            summary: String::new(),
            stories,
            supporting: vec![],
            estimate_minutes: None,
            provenance: Default::default(),
        }
    }

    fn story(title: &str, stops: Vec<Stop>) -> Story {
        Story {
            title: title.into(),
            summary: String::new(),
            stops,
        }
    }

    fn verdict(verdict: Verdict) -> VerdictRecord {
        VerdictRecord {
            verdict,
            at: Utc::now(),
            block: vec![],
            comment_id: None,
        }
    }

    fn session() -> ReviewSession {
        ReviewSession::new(
            "/tmp/repo".into(),
            "head".into(),
            None,
            SessionDiffSource::WorkingTree,
        )
    }

    fn compute<'a>(
        guide: &'a ReviewGuide,
        progress: &Progress,
        files: &[DiffFile],
        session: &ReviewSession,
    ) -> Coverage<'a> {
        let anchors: Vec<_> = guide
            .stops()
            .map(|(_, stop)| resolve_stop(stop, files).unwrap())
            .collect();
        coverage(
            guide,
            progress,
            guide
                .stops()
                .zip(&anchors)
                .map(|((_, stop), anchor)| (stop, Some(anchor))),
            session,
            files,
            false,
        )
    }

    #[test]
    fn coverage_counts_verdicts_risk_marks_dismissals_and_spot_checks_separately() {
        let mut guide = guide(vec![
            story("Verified", vec![stop("verified.rs", true)]),
            story("Wrong", vec![stop("wrong.rs", false)]),
            story("Parked", vec![stop("parked.rs", true)]),
            story("Unchecked", vec![stop("unchecked.rs", true)]),
        ]);
        let files = [
            "verified.rs",
            "wrong.rs",
            "parked.rs",
            "unchecked.rs",
            "docs.md",
            "marked-file.rs",
            "marked-hunk.rs",
            "spot.rs",
            "parked-spot.rs",
            "untouched.rs",
        ]
        .map(file);
        let href = |index: usize| hunk_ref(&files[index], &files[index].hunks[0]);
        guide.supporting = vec![
            SupportingGroup {
                label: "Docs".into(),
                basis: Basis::Mechanical,
                note: String::new(),
                hunks: vec![href(4)],
            },
            SupportingGroup {
                label: "Wiring".into(),
                basis: Basis::Claimed,
                note: String::new(),
                hunks: (5..10).map(href).collect(),
            },
        ];
        let mut progress = Progress {
            verdicts: [
                ("verified.rs".into(), verdict(Verdict::ChecksOut)),
                ("wrong.rs".into(), verdict(Verdict::Wrong)),
                ("parked.rs".into(), verdict(Verdict::Parked)),
            ]
            .into(),
            spot_checks: vec![
                SpotCheck {
                    hunk: href(7),
                    verdict: Verdict::ChecksOut,
                },
                SpotCheck {
                    hunk: href(8),
                    verdict: Verdict::Parked,
                },
            ],
            spot_diff: Some(guide.diff.clone()),
            dismissed_groups: vec!["Docs".into(), "Wiring".into()],
            elapsed_secs: 91,
            ..Progress::default()
        };
        let mut session = session();
        for (index, marked) in [(3, true), (5, true), (6, false)] {
            let file = &files[index];
            let mut marks =
                FileReview::new(file.display_path().clone(), file.status, file.content_hash);
            marks.reviewed = marked;
            if !marked {
                marks
                    .reviewed_hunks
                    .insert(file.hunk_review_key(0).unwrap());
            }
            session.files.insert(marks.path.clone(), marks);
        }
        let record = compute(&guide, &progress, &files, &session);
        assert_eq!(
            record.stops,
            VerdictCounts {
                verified: 1,
                wrong: 1,
                parked: 1,
                unchecked: 1
            }
        );
        assert_eq!(record.story_totals, record.stops);
        assert_eq!((record.risk_verified, record.risk_unverified), (1, 2));
        assert_eq!(
            (record.file_marks, record.hunk_marks, record.elapsed_secs),
            (2, 1, 91)
        );
        assert_eq!(
            record.spot_checks,
            VerdictCounts {
                verified: 1,
                parked: 1,
                ..VerdictCounts::default()
            }
        );
        assert_eq!(record.supporting_total(), 6);
        assert_eq!(record.groups[0].dismissed_hunks, 1);
        assert!(record.groups[0].dismissed);
        assert!(
            !record.groups[1].dismissed,
            "claimed groups cannot be dismissed"
        );
        assert_eq!(
            (record.groups[1].reviewed, record.groups[1].unreviewed()),
            (3, 2)
        );
        assert_eq!(record.approval_risk_count(SubmitEvent::Approve), 2);
        assert_eq!(record.approval_risk_count(SubmitEvent::RequestChanges), 0);
        assert!(!record.complete());

        progress.spot_diff = Some("different diff".into());
        assert_eq!(
            compute(&guide, &progress, &files, &session)
                .spot_checks
                .decided(),
            0
        );
    }

    #[test]
    fn coverage_note_changes_with_verification_and_never_uses_guide_prose() {
        let guide = guide(vec![
            story("Never include these titles", vec![stop("first", true)]),
            story("or this one", vec![stop("second", false)]),
        ]);
        let files = [file("first"), file("second")];
        let session = session();
        let mut progress = Progress::default();
        let untouched = compute(&guide, &progress, &files, &session);
        assert_eq!(
            untouched.note(),
            "Reviewed with a guide: verified 0 of 2 stops, 0 of 1 risk item verified. Guide covers 2 of 2 hunks. Not verified: stories 1–2."
        );
        assert_eq!(untouched.approval_risk_count(SubmitEvent::Approve), 1);

        progress
            .verdicts
            .insert("first".into(), verdict(Verdict::ChecksOut));
        let partial = compute(&guide, &progress, &files, &session);
        assert_eq!(
            partial.note(),
            "Reviewed with a guide: verified 1 of 2 stops (story 1), 1 of 1 risk item verified. Guide covers 2 of 2 hunks. Not verified: story 2."
        );
        assert_eq!(partial.approval_risk_count(SubmitEvent::Approve), 0);
        assert!(!partial.complete());

        progress
            .verdicts
            .insert("second".into(), verdict(Verdict::ChecksOut));
        let complete = compute(&guide, &progress, &files, &session);
        assert_eq!(
            complete.note(),
            "Reviewed with a guide: verified 2 of 2 stops (stories 1–2), 1 of 1 risk item verified. Guide covers 2 of 2 hunks."
        );
        assert!(complete.complete());
    }
}
