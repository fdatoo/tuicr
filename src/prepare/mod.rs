//! Headless review preparation and its shared prompt, process and status APIs.

mod evidence;
mod pipeline;
mod prompts;
mod worker;

use std::borrow::Cow;
use std::path::Path;

use serde_json::{Value, json};

use crate::app::{App, DiffSource};
use crate::cli::GuideCommand;
use crate::error::{Result, TuicrError};
use crate::model::{DiffFile, LineSide};
use crate::review_guide::{GuideStore, ReviewGuide, ReviewKey, outline};

use pipeline::Job;
pub use prompts::{PromptKind, effective_prompt};
use worker::Start;
pub(crate) use worker::StartMode;
pub use worker::{
    TrackedPreparation, cancel, run_worker_if_invoked, spawn_detached, spawn_tracked, status,
    tracked_preparation,
};

pub fn read_guide(path: &Path) -> Result<ReviewGuide> {
    pipeline::read_guide(path)
}

pub fn print_prompts() -> Result<()> {
    let rules = prompts::Rules::load()?;
    for (label, kind) in [
        ("Write", PromptKind::Write),
        ("Audit", PromptKind::Audit),
        ("Ask", PromptKind::Ask),
    ] {
        println!("# {label} prompt\n\n{}", rules.prompt(kind));
    }
    Ok(())
}

pub fn run_cli(
    app: &mut App,
    command: GuideCommand,
    tracked: Option<&TrackedPreparation>,
) -> Result<()> {
    app.wait_for_preparation_range()?;
    let files = review_files(app);
    let diff = app.review_digest();
    match command {
        GuideCommand::Outline => {
            print!("{}", outline::outline(files, &diff));
        }
        GuideCommand::Check { path } => {
            let guide = read_guide(&path)?;
            pipeline::check(&guide, &diff, files)
                .map_err(|error| invalid(format!("{}:\n{error}", path.display())))?;
            println!(
                "Review guide matches diff {diff} ({} stops)",
                guide.stop_count()
            );
        }
        GuideCommand::Prepare { guide, wait, .. } => {
            let key = app
                .review_key()
                .ok_or_else(|| invalid("Open a loaded diff before preparing a review guide"))?;
            let job = review_job(app, files, diff, guide);
            let target = app.preparation_target();
            if let Some(tracked) = tracked {
                return tracked.prepare(&key, target, job);
            }
            let store = GuideStore::open(&key)?;
            let (identity, child) =
                match worker::start_job(&key, &store, &job, target, StartMode::Cli)? {
                    Start::Started(child, identity) => {
                        if !wait {
                            println!(
                                "Preparing review {}. Watch with: tail -f -- {}",
                                key.as_str(),
                                shell_path(&store.log_path())
                            );
                            return Ok(());
                        }
                        (identity, Some(child))
                    }
                    Start::Preparing(identity) => {
                        if !wait {
                            println!(
                                "Review {} is already preparing. Watch {}",
                                key.as_str(),
                                store.log_path().display()
                            );
                            return Ok(());
                        }
                        println!(
                            "Review {} is already preparing; waiting for it to finish",
                            key.as_str()
                        );
                        (identity, None)
                    }
                    Start::Ready => {
                        println!("Review guide already ready for {}", key.as_str());
                        return Ok(());
                    }
                    Start::PreviousFailure => {
                        unreachable!("explicit CLI preparation may retry a failed job")
                    }
                };
            let state = worker::wait_for_job(&store, identity, &job.diff, child)?;
            let estimate = state
                .estimate_minutes
                .map_or_else(String::new, |mins| format!(", about {mins} min"));
            println!(
                "Review guide ready for {} ({} stops{estimate})",
                key.as_str(),
                state.stops.unwrap_or(0)
            );
        }
    }
    Ok(())
}

fn shell_path(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

fn invalid(message: impl Into<String>) -> TuicrError {
    TuicrError::InvalidInput(message.into())
}

fn review_job<'a>(
    app: &App,
    files: &'a [DiffFile],
    diff: String,
    supplied: Option<std::path::PathBuf>,
) -> Job<'a> {
    let (title, context) = review_context(app);
    Job {
        files: Cow::Borrowed(files),
        source: app.source_request(LineSide::New),
        diff,
        title,
        context,
        supplied,
    }
}

fn review_context(app: &App) -> (String, Value) {
    if let Some(info) = &app.pr_info {
        let checks: Vec<_> = info
            .checks
            .iter()
            .map(|check| {
                json!({
                    "name": check.name, "status": check.status, "conclusion": check.conclusion,
                })
            })
            .collect();
        let context = json!({
            "title": info.details.title, "description": info.details.body,
            "checks_reported_by_forge": checks,
        });
        return (info.details.title.clone(), context);
    }
    let commits = if let Some((start, end)) = app.commit_selection_range {
        app.review_commits
            .get(start..=end)
            .unwrap_or(&app.review_commits)
    } else {
        &app.review_commits
    };
    let commits: Vec<_> = commits
        .iter()
        .map(|commit| {
            json!({
                "id": commit.id, "summary": commit.summary, "description": commit.body,
            })
        })
        .collect();
    let title = match &app.diff_source {
        DiffSource::Staged => "Staged changes",
        DiffSource::Unstaged => "Unstaged changes",
        DiffSource::CommitRange(_) => "Local commit range",
        DiffSource::StagedUnstagedAndCommits(_) => "Commit range and working tree changes",
        _ => "Working tree changes",
    };
    (
        title.into(),
        json!({"title": title, "commits": commits, "path_filter": app.path_filter}),
    )
}

fn review_files(app: &App) -> &[DiffFile] {
    &app.diff_files[app
        .diff_files
        .partition_point(|file| file.is_commit_message)..]
}

pub(crate) fn current_rules_digest() -> Result<String> {
    Ok(prompts::Rules::load()?.digest)
}

fn guide_is_current(guide: &ReviewGuide, diff: &str, current_rules: &str) -> bool {
    guide.diff == diff && guide.provenance.rules_digest.as_deref() == Some(current_rules)
}

pub(crate) fn start_open_review(
    app: &App,
    key: &ReviewKey,
    mode: StartMode,
) -> Result<&'static str> {
    let store = GuideStore::open(key)?;
    let diff = app.review_digest();
    let job = review_job(app, review_files(app), diff, None);
    match worker::start_job(key, &store, &job, app.preparation_target(), mode)? {
        Start::Started(child, _) => {
            worker::reap(child);
            Ok("Preparing the review guide; you can keep reviewing")
        }
        Start::Preparing(_) => Ok("A review guide is already being prepared for this review"),
        Start::Ready => Ok("The review guide for this diff is already ready; use :guide"),
        Start::PreviousFailure => Ok("The previous preparation failed; use :prepare to retry"),
    }
}
