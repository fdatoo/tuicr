use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use unicode_segmentation::UnicodeSegmentation;

use super::PromptKind;
use super::evidence::Evidence;
use super::prompts::{REPAIR, Rules};
use crate::error::{Result, TuicrError};
use crate::model::DiffFile;
use crate::review_agent::{self, AgentEvent, AgentRequest, Cancel, Settings, SnapshotRequest};
use crate::review_guide::{
    Audit, Author, Basis, Followup, GuideStore, PrepState, Provenance, ReviewGuide, Risk, RiskKind,
    Side, Stop, Story, SupportingGroup, TestRef, mechanical, outline, resolve,
};

const TINY_HUNKS: usize = 3;
const TINY_CHANGED_LINES: usize = 60;
const MAX_STORY_STOPS: usize = 12;
const WARN_STOP_COUNT: usize = 16;
const MAX_SUPPLEMENTAL_AUDITS: usize = 2;
const MAX_EXPLANATION_WORDS: usize = 60;
const MAX_EXPLANATION_SENTENCES: usize = 4;
const MAX_FOLLOWUP_SENTENCES: usize = 3;
const MAX_AUDIT_NOTE_SENTENCES: usize = 2;
const MAX_GUIDE_BYTES: u64 = 4 * 1024 * 1024;

type Runner<'a> = dyn Fn(
        &AgentRequest,
        &Cancel,
        &mut dyn FnMut(AgentEvent<'_>),
    ) -> std::result::Result<String, String>
    + 'a;

#[derive(Serialize, Deserialize)]
pub(super) struct Job<'a> {
    pub(super) files: Cow<'a, [DiffFile]>,
    pub(super) snapshot: SnapshotRequest,
    pub(super) diff: String,
    pub(super) title: String,
    pub(super) context: Value,
    pub(super) supplied: Option<PathBuf>,
}

pub(super) fn prepare(
    job: Job<'_>,
    store: &GuideStore,
    state: &mut PrepState,
    cancel: &Cancel,
    mut persist: impl FnMut(&PrepState) -> Result<()>,
) -> Result<ReviewGuide> {
    step(state, &mut persist, "Loading reviewer settings")?;
    let settings = Settings::load().map_err(TuicrError::InvalidInput)?;
    require_provider(settings.provider)?;
    let rules = Rules::load()?;
    step(state, &mut persist, "Building the source snapshot")?;
    let snapshot = job.snapshot.materialize().map_err(|error| {
        TuicrError::InvalidInput(format!("Could not build the source snapshot: {error}"))
    })?;
    if job.snapshot.is_local() {
        // When preparation opens a local review first, retain the actual
        // snapshot's file inodes rather than copying its source a second time.
        let _ = crate::review_guide::baseline::Baseline::open_or_capture(
            store,
            &job.diff,
            &job.files,
            &job.snapshot,
            Some(snapshot.dir()),
        );
    }
    step(state, &mut persist, "Classifying hunks and risks")?;
    let mechanical = mechanical::classify(&job.files);
    let evidence = Evidence::collect(
        &job.files,
        snapshot.dir(),
        !review_agent::can_read_files(settings.provider),
    );
    let context = ReviewContext {
        files: &job.files,
        diff: &job.diff,
        mechanical: &mechanical,
        outline: outline::outline(&job.files, &job.diff),
        context: &job.context,
        rules: &rules,
        evidence: &evidence.text,
        source_limit: snapshot.limits(),
    };
    let supplied = job.supplied.as_deref().map(read_guide).transpose()?;
    let previous = if supplied.is_none() {
        store
            .guide()?
            .filter(|guide| !super::guide_is_current(guide, &job.diff, &rules.digest))
    } else {
        None
    };
    let mut model = Model {
        settings: &settings,
        dir: snapshot.dir(),
        cancel,
        runner: &review_agent::run,
    };
    let mut call = |stage: &str, prompt: Option<String>| {
        step(state, &mut persist, stage)?;
        match prompt {
            Some(prompt) => model.call(prompt, state, &mut persist),
            None => Ok(String::new()),
        }
    };
    let (mut guide, dropped) =
        context.write_audit_and_validate(supplied, previous.as_ref(), &mut call)?;
    let unaudited = if guide.provenance.audited {
        0
    } else {
        unaudited_count(&guide)
    };
    let limits = preparation_limits(
        snapshot.limits(),
        &evidence.limits,
        rules.truncated,
        dropped,
        unaudited,
    );
    let provider = match settings.provider {
        review_agent::Provider::Claude => "claude",
        review_agent::Provider::Codex => "codex",
    };
    let default_model = if provider == "claude" {
        "sonnet"
    } else {
        "default"
    };
    guide.provenance = Provenance {
        author: guide.provenance.author,
        audited: guide.provenance.audited,
        provider: provider.into(),
        model: settings
            .reviewer_model
            .unwrap_or_else(|| default_model.into()),
        rules_digest: Some(rules.digest),
        prepared_at: Some(Utc::now()),
        limits,
    };
    step(state, &mut persist, "Saving the review guide")?;
    Ok(guide)
}

fn require_provider(provider: review_agent::Provider) -> Result<()> {
    let name = provider.name();
    let found = std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            let Ok(metadata) = dir.join(name).metadata() else {
                return false;
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            metadata.is_file()
        })
    });
    if !found {
        return Err(invalid(format!(
            "Install `{name}` and put it on PATH, or choose a different reviewer in config.toml"
        )));
    }
    Ok(())
}

fn preparation_limits(
    snapshot: Option<&str>,
    evidence: &[&str],
    truncated: bool,
    dropped: usize,
    unaudited: usize,
) -> Option<String> {
    let mut limits = snapshot
        .into_iter()
        .chain(evidence.iter().copied())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if truncated {
        limits.push("reviewer.md exceeded 16 KiB; only the first 16 KiB were used".into());
    }
    if dropped != 0 {
        limits.push(format!(
            "Dropped {dropped} unresolvable stop(s) after one anchor repair pass"
        ));
    }
    if unaudited != 0 {
        limits.push(format!("{unaudited} stop(s) were not audited"));
    }
    (!limits.is_empty()).then(|| limits.join("; "))
}

fn unaudited_count(guide: &ReviewGuide) -> usize {
    guide
        .stops()
        .filter(|(_, stop)| stop.audit == Audit::Unchecked)
        .count()
}

fn step(
    state: &mut PrepState,
    persist: &mut dyn FnMut(&PrepState) -> Result<()>,
    label: &str,
) -> Result<()> {
    state.step.clear();
    state.step.push_str(label);
    state.heartbeat_at = Utc::now();
    persist(state)
}

struct Model<'a> {
    settings: &'a Settings,
    dir: &'a Path,
    cancel: &'a Cancel,
    runner: &'a Runner<'a>,
}

impl Model<'_> {
    fn call(
        &mut self,
        prompt: String,
        state: &mut PrepState,
        persist: &mut dyn FnMut(&PrepState) -> Result<()>,
    ) -> Result<String> {
        let request = AgentRequest {
            provider: self.settings.provider,
            model: self.settings.reviewer_model.clone(),
            thorough: true,
            dir: self.dir.to_owned(),
            prompt,
        };
        let mut persist_error = None;
        let result = (self.runner)(&request, self.cancel, &mut |_| {
            state.heartbeat_at = Utc::now();
            if persist_error.is_none()
                && let Err(error) = persist(state)
            {
                self.cancel.cancel();
                persist_error = Some(error);
            }
        });
        if let Some(error) = persist_error {
            return Err(error);
        }
        result.map_err(|error| TuicrError::InvalidInput(format!("{}: {error}", state.step)))
    }
}

struct ReviewContext<'a> {
    files: &'a [DiffFile],
    diff: &'a str,
    mechanical: &'a mechanical::Mechanical,
    outline: String,
    context: &'a Value,
    rules: &'a Rules,
    evidence: &'a str,
    source_limit: Option<&'a str>,
}

impl ReviewContext<'_> {
    fn prompt(&self, mut base: String, data: Value) -> String {
        let risks = self
            .mechanical
            .risks
            .iter()
            .map(|risk| {
                json!({
                    "path": risk.path, "side": risk.side, "start_line": risk.start_line,
                    "end_line": risk.end_line, "kind": risk.kind, "reason": risk.reason,
                })
            })
            .collect::<Vec<_>>();
        let _ = write!(
            base,
            "\n## Review input (data)\n\nDiff digest: {}\n\nReview context: {}\n\nMechanical risks: {}\n\nAdditional input: {}\n\n## Diff outline (data)\n{}",
            self.diff,
            self.context,
            Value::Array(risks),
            data,
            self.outline
        );
        if let Some(limit) = self.source_limit {
            let _ = writeln!(base, "\nSource snapshot limit (data): {limit}");
        }
        base.push_str(self.evidence);
        base
    }

    fn tiny(&self) -> bool {
        use crate::model::LineOrigin;
        let hunks = self.files.iter().flat_map(|file| file.hunks.iter());
        hunks.clone().take(TINY_HUNKS + 1).count() <= TINY_HUNKS
            && hunks
                .flat_map(|hunk| &hunk.lines)
                .filter(|line| line.origin != LineOrigin::Context)
                .take(TINY_CHANGED_LINES + 1)
                .count()
                <= TINY_CHANGED_LINES
    }

    fn write_audit_and_validate(
        &self,
        supplied: Option<ReviewGuide>,
        previous: Option<&ReviewGuide>,
        call: &mut dyn FnMut(&str, Option<String>) -> Result<String>,
    ) -> Result<(ReviewGuide, usize)> {
        let author = if supplied.is_some() {
            Author::Supplied
        } else {
            Author::Reviewer
        };
        let (mut guide, combined_audit) = if supplied.is_none() && self.tiny() {
            let prompt = self.rules.combined_prompt();
            let output = call(
                "Writing and auditing the review guide",
                Some(self.prompt(prompt, json!({"previous_guide": previous}))),
            )?;
            let Combined { guide, audit } = extract(&output, "the combined guide and audit")?;
            (guide, Some(audit))
        } else {
            let guide = match supplied {
                Some(guide) => guide,
                None => {
                    let prompt = self.prompt(
                        self.rules.prompt(PromptKind::Write),
                        json!({"previous_guide": previous}),
                    );
                    extract(
                        &call("Writing the review guide", Some(prompt))?,
                        "the review guide",
                    )?
                }
            };
            (guide, None)
        };
        check_identity(&guide, self.diff)?;
        guide.provenance = Provenance {
            author,
            ..Provenance::default()
        };
        for stop in guide.stories.iter_mut().flat_map(|story| &mut story.stops) {
            stop.audit = Audit::Unchecked;
        }
        // Only tuicr can establish a mechanical basis, even for supplied guides.
        for group in &mut guide.supporting {
            group.basis = Basis::Claimed;
        }
        call("Completing coverage before audit", None)?;
        mechanical::complete(&mut guide, self.files, self.mechanical);
        let audit = match combined_audit {
            Some(audit) => audit,
            None => self.audit(&guide, None, call)?,
        };
        let count_before_audit = guide.stop_count();
        merge_audit(&mut guide, audit, None);
        call("Validating stops", None)?;
        let dropped = self.repair_and_drop(&mut guide, call)?;
        if dropped.is_some() || guide.stop_count() != count_before_audit {
            call("Refreshing coverage after audit", None)?;
            mechanical::complete(&mut guide, self.files, self.mechanical);
        }
        let dropped = dropped.unwrap_or_default();
        for _ in 0..MAX_SUPPLEMENTAL_AUDITS {
            let unchecked: HashSet<_> = guide
                .stops()
                .filter(|(_, stop)| stop.audit == Audit::Unchecked)
                .map(|(_, stop)| stop.id.clone())
                .collect();
            if unchecked.is_empty() {
                break;
            }
            let audit = self.audit(&guide, Some(&unchecked), call)?;
            merge_audit(&mut guide, audit, Some(&unchecked));
        }
        for stop in guide.stories.iter_mut().flat_map(|story| &mut story.stops) {
            trim_prose(
                &mut stop.explanation,
                MAX_EXPLANATION_SENTENCES,
                MAX_EXPLANATION_WORDS,
            );
            for followup in &mut stop.followups {
                trim_prose(&mut followup.answer, MAX_FOLLOWUP_SENTENCES, usize::MAX);
            }
        }
        if author == Author::Reviewer && enforce_stop_budget(&mut guide, self.files) {
            // Every known risk is already attached to a retained stop.
            mechanical::complete_supporting(&mut guide, self.files, self.mechanical);
        }
        if guide.stop_count() == 0 {
            if dropped != 0 {
                return Err(invalid(format!(
                    "All {dropped} stops remained unresolvable after an anchor repair pass. Try `:prepare` again or supply a guide using `tuicr guide outline`."
                )));
            }
            if !guide.supporting.is_empty()
                && guide
                    .supporting
                    .iter()
                    .all(|group| group.basis == Basis::Mechanical)
            {
                let labels = guide
                    .supporting
                    .iter()
                    .map(|group| group.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(invalid(format!(
                    "Nothing to guide: all changes are supporting changes ({labels})"
                )));
            }
        }
        guide
            .validate()
            .map_err(|error| invalid(format!("Invalid prepared guide: {error}")))?;
        let stop_count = guide.stop_count();
        let estimate = guide.estimate_minutes.unwrap_or_default();
        guide.estimate_minutes = Some(if estimate == 0 || stop_count > WARN_STOP_COUNT {
            let estimate_floor = guide
                .stops()
                .map(|(_, stop)| 1_u32 + u32::from(stop.needs_explicit_verdict()))
                .fold(0, u32::saturating_add);
            estimate.max(estimate_floor)
        } else {
            estimate
        });
        let (covered, total) = mechanical::stop_coverage(&guide, self.files);
        let _ = write!(
            guide.summary,
            "\nStops cover {covered} of {total} changed hunks. Estimate is for the stops only."
        );
        if stop_count > WARN_STOP_COUNT {
            let _ = write!(
                guide.summary,
                "\nThe guide has {stop_count} stops, over the {WARN_STOP_COUNT}-stop target; it may take more than one sitting."
            );
        }
        guide.provenance.audited = unaudited_count(&guide) == 0;
        Ok((guide, dropped))
    }

    fn audit(
        &self,
        guide: &ReviewGuide,
        only: Option<&HashSet<String>>,
        call: &mut dyn FnMut(&str, Option<String>) -> Result<String>,
    ) -> Result<AuditResponse> {
        let prompt = self.prompt(
            self.rules.prompt(PromptKind::Audit),
            json!({
                "guide_to_audit": guide,
                "only_stop_ids": only.map(|ids| guide.stops().filter_map(|(_, stop)| ids.contains(&stop.id).then_some(&stop.id)).collect::<Vec<_>>()),
            }),
        );
        let label = if only.is_some() {
            "Auditing unchecked stops"
        } else {
            "Auditing every stop"
        };
        extract(&call(label, Some(prompt))?, "the stop audit")
    }

    fn repair_and_drop(
        &self,
        guide: &mut ReviewGuide,
        call: &mut dyn FnMut(&str, Option<String>) -> Result<String>,
    ) -> Result<Option<usize>> {
        let invalid_stops = guide
            .stops()
            .filter_map(|(_, stop)| {
                resolve::resolve_stop(stop, self.files)
                    .err()
                    .map(|error| (stop, error))
            })
            .collect::<Vec<_>>();
        if invalid_stops.is_empty() {
            return Ok(None);
        }
        let bad = invalid_stops
            .iter()
            .map(|(stop, error)| json!({"stop": stop, "resolver_error": error}))
            .collect::<Vec<_>>();
        let prompt = self.prompt(self.rules.extend(REPAIR), json!({"invalid_stops": bad}));
        let Repair { anchors } = extract(
            &call("Repairing stop anchors", Some(prompt))?,
            "the anchor repairs",
        )?;
        let mut anchors: HashMap<_, _> = anchors
            .into_iter()
            .map(|anchor| (anchor.id.clone(), anchor))
            .collect();
        let mut dropped = 0;
        for story in &mut guide.stories {
            story.stops.retain_mut(|stop| {
                if resolve::resolve_stop(stop, self.files).is_ok() {
                    return true;
                }
                if let Some(anchor) = anchors.remove(&stop.id) {
                    stop.path = anchor.path;
                    stop.side = anchor.side;
                    stop.start_line = anchor.start_line;
                    stop.end_line = anchor.end_line;
                    if resolve::resolve_stop(stop, self.files).is_ok() {
                        // A new block invalidates evidence that was tied to the old anchor.
                        stop.audit = Audit::Unchecked;
                        stop.followups.clear();
                        stop.test = None;
                        stop.untested = true;
                        return true;
                    }
                }
                dropped += 1;
                false
            });
        }
        guide.stories.retain(|story| !story.stops.is_empty());
        Ok(Some(dropped))
    }
}

/// Keep the first twelve ordinary stops in the writer's reading order. A
/// risky stop stays in its story; unrelated discoveries already trail them.
/// Demoted claims remain context for a named, non-dismissible group.
fn enforce_stop_budget(guide: &mut ReviewGuide, files: &[DiffFile]) -> bool {
    if guide.stop_count() <= MAX_STORY_STOPS {
        return false;
    }
    let mut remaining = MAX_STORY_STOPS;
    let mut changed = false;
    for story in &mut guide.stories {
        let mut hunks = Vec::new();
        let mut note = String::new();
        story.stops.retain(|stop| {
            if stop.risk.is_some() || matches!(stop.audit, Audit::Doubtful { .. }) {
                return true;
            }
            if remaining > 0 {
                remaining -= 1;
                return true;
            }
            changed = true;
            if let Ok(at) = resolve::resolve_stop(stop, files) {
                let file = &files[at.file];
                hunks.push(resolve::hunk_ref(file, &file.hunks[at.hunk]));
                if !note.is_empty() {
                    note.push('\n');
                }
                let _ = write!(note, "• {}", stop.claim.lines().next().unwrap_or_default());
            }
            false
        });
        if !hunks.is_empty() {
            guide.supporting.push(SupportingGroup {
                label: format!("More on {}", story.title),
                basis: Basis::Claimed,
                note,
                hunks,
            });
        }
    }
    if changed {
        guide.stories.retain(|story| !story.stops.is_empty());
    }
    changed
}

fn invalid(error: impl Into<String>) -> TuicrError {
    TuicrError::InvalidInput(error.into())
}

fn check_identity(guide: &ReviewGuide, diff: &str) -> Result<()> {
    if guide.version != crate::review_guide::GUIDE_VERSION {
        return Err(invalid(format!(
            "Expected a version {} guide, not version {}",
            crate::review_guide::GUIDE_VERSION,
            guide.version
        )));
    }
    if guide.diff != diff {
        return Err(invalid(format!(
            "Guide targets diff {}, but this review is {diff}; use `tuicr guide outline` for the current digest",
            guide.diff
        )));
    }
    Ok(())
}

pub(super) fn read_guide(path: &Path) -> Result<ReviewGuide> {
    let file = File::open(path)
        .map_err(|error| invalid(format!("Could not open {}: {error}", path.display())))?;
    if file.metadata()?.len() > MAX_GUIDE_BYTES {
        return Err(invalid(format!(
            "{} exceeds the 4 MiB guide limit",
            path.display()
        )));
    }
    serde_json::from_reader(BufReader::new(file.take(MAX_GUIDE_BYTES + 1)))
        .map_err(|error| invalid(format!("Could not read {}: {error}", path.display())))
}

pub(super) fn check(guide: &ReviewGuide, diff: &str, files: &[DiffFile]) -> Result<()> {
    let mut errors = Vec::new();
    if let Err(error) = check_identity(guide, diff) {
        errors.push(error.to_string());
    }
    if let Err(error) = guide.validate() {
        errors.push(error);
    }
    errors.extend(
        guide
            .stops()
            .filter_map(|(_, stop)| resolve::resolve_stop(stop, files).err()),
    );
    if errors.is_empty() {
        Ok(())
    } else {
        Err(invalid(errors.join("\n")))
    }
}

fn extract<T: DeserializeOwned>(output: &str, label: &str) -> Result<T> {
    // Skip coherent nonmatching objects whole; otherwise a nested object could
    // accidentally match the expected outer schema.
    let mut remaining = output;
    while let Some(start) = remaining.find('{') {
        remaining = &remaining[start..];
        let mut stream = serde_json::Deserializer::from_str(remaining).into_iter::<Value>();
        if let Some(Ok(value)) = stream.next() {
            if let Ok(result) = serde_json::from_value(value) {
                return Ok(result);
            }
            remaining = &remaining[stream.byte_offset()..];
        } else {
            remaining = &remaining[1..];
        }
    }
    Err(invalid(format!(
        "The reviewing agent did not return valid JSON for {label}; retry preparation or choose a different reviewer model"
    )))
}

#[derive(Deserialize)]
struct Combined {
    guide: ReviewGuide,
    audit: AuditResponse,
}

#[derive(Deserialize)]
struct AuditResponse {
    stops: Vec<AuditedStop>,
    #[serde(default)]
    missed: Vec<MissedStop>,
    #[serde(default)]
    estimate_minutes: Option<u32>,
}

#[derive(Deserialize)]
struct AuditedStop {
    id: String,
    audit: Audit,
    #[serde(default)]
    counterexample: Option<String>,
    #[serde(default)]
    test: Option<TestRef>,
    #[serde(default)]
    untested: Option<bool>,
    #[serde(default)]
    explanation_edits: Vec<ExplanationEdit>,
    #[serde(default)]
    risk: Option<Risk>,
    #[serde(default)]
    followups: Vec<Followup>,
}

/// A correction must target a whole sentence from the writer's explanation.
#[derive(Deserialize)]
struct ExplanationEdit {
    sentence: String,
    replacement: Option<String>,
}

fn apply_explanation_edits(explanation: &mut String, edits: &[ExplanationEdit]) {
    if edits.is_empty() {
        return;
    }
    let mut corrected = String::with_capacity(explanation.len());
    for sentence in explanation.split_sentence_bounds().map(str::trim) {
        let edit = edits.iter().find(|edit| edit.sentence.trim() == sentence);
        let sentence = match edit {
            Some(ExplanationEdit {
                replacement: None, ..
            }) => continue,
            Some(ExplanationEdit {
                replacement: Some(text),
                ..
            }) if text.trim().split_sentence_bounds().count() == 1 => text.trim(),
            _ => sentence,
        };
        if !corrected.is_empty() {
            corrected.push(' ');
        }
        corrected.push_str(sentence);
    }
    *explanation = corrected;
}

fn trim_prose(text: &mut String, max_sentences: usize, max_words: usize) {
    let mut end = 0;
    let mut words_left = max_words;
    for sentence in text.split_sentence_bounds().take(max_sentences) {
        let words = sentence.split_whitespace().count();
        if words > words_left {
            break;
        }
        words_left -= words;
        end += sentence.len();
    }
    text.truncate(text[..end].trim_end().len());
}

impl AuditedStop {
    fn verdict(&mut self) -> Audit {
        let mut verdict = match self
            .counterexample
            .take()
            .filter(|note| !note.trim().is_empty())
        {
            Some(note) => Audit::Doubtful { note },
            None => std::mem::take(&mut self.audit),
        };
        if let Audit::Doubtful { note } = &mut verdict {
            trim_prose(note, MAX_AUDIT_NOTE_SENTENCES, usize::MAX);
        }
        verdict
    }
}

#[derive(Deserialize)]
struct MissedStop {
    story_stop_id: Option<String>,
    stop: Stop,
}

#[derive(Deserialize)]
struct Repair {
    anchors: Vec<Anchor>,
}

#[derive(Deserialize)]
struct Anchor {
    id: String,
    path: String,
    side: Side,
    start_line: u32,
    end_line: u32,
}

fn merge_audit(guide: &mut ReviewGuide, response: AuditResponse, only: Option<&HashSet<String>>) {
    if let Some(estimate) = response.estimate_minutes.filter(|&minutes| minutes > 0) {
        guide.estimate_minutes = Some(estimate);
    }
    let mut outcomes = HashMap::new();
    // Each stop must have one independent result. Duplicates are not votes.
    for outcome in response.stops {
        outcomes
            .entry(outcome.id.clone())
            .and_modify(|entry| *entry = None)
            .or_insert(Some(outcome));
    }
    for stop in guide.stories.iter_mut().flat_map(|story| &mut story.stops) {
        if only.is_some_and(|ids| !ids.contains(&stop.id)) {
            continue;
        }
        let audited = outcomes.remove(&stop.id).flatten().and_then(|mut outcome| {
            let audit = outcome.verdict();
            (audit != Audit::Unchecked).then_some((outcome, audit))
        });
        stop.audit = match audited {
            Some((outcome, audit)) => {
                stop.test = outcome.test.filter(|_| {
                    outcome.untested != Some(true)
                        && (!matches!(audit, Audit::Doubtful { .. })
                            || outcome.untested == Some(false))
                });
                stop.untested = stop.test.is_none();
                apply_explanation_edits(&mut stop.explanation, &outcome.explanation_edits);
                if stop.risk.is_none() {
                    stop.risk = outcome.risk;
                }
                stop.followups = outcome.followups;
                audit
            }
            _ => {
                stop.followups.clear();
                stop.test = None;
                stop.untested = true;
                Audit::Unchecked
            }
        };
    }
    if only.is_some() {
        return;
    }
    let mut unexplained = Vec::new();
    for MissedStop {
        story_stop_id,
        mut stop,
    } in response.missed
    {
        stop.risk = Some(Risk {
            kind: RiskKind::Unexplained,
            reason: stop.risk.take().map_or_else(
                || "The audit found a change the description does not account for.".into(),
                |risk| risk.reason,
            ),
        });
        // A new claim discovered by this pass was not independently refuted.
        stop.audit = Audit::Unchecked;
        stop.followups.clear();
        stop.test = None;
        stop.untested = true;
        if let Some(story) = story_stop_id.as_ref().and_then(|id| {
            guide
                .stories
                .iter_mut()
                .find(|story| story.stops.iter().any(|s| &s.id == id))
        }) {
            story.stops.push(stop);
        } else {
            unexplained.push(stop);
        }
    }
    if !unexplained.is_empty() {
        guide.stories.push(Story {
            title: "Unexplained changes".into(),
            summary: "Material changes the audit could not connect to the described intent.".into(),
            stops: unexplained,
        });
    }
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
