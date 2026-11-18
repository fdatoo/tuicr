use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use super::sessions_tab::PreparedReview;
use super::{App, DiffSource, PullRequestDiffSource, TargetTab};
use crate::forge::traits::{ForgeKind, PullRequestSummary};
use crate::review_guide::{PrepState, PrepStatus, PrepTarget, ReviewKey, guides_dir};

const POLL_INTERVAL: Duration = Duration::from_millis(450);
const NOT_PREPARED: &str = "not prepared";

#[derive(Default)]
pub(super) struct PreparationUi {
    pub(super) repo_flag: Option<(&'static str, OsString)>,
    next_poll: Option<Instant>,
    last_view: Option<TargetTab>,
    pr_rows: HashMap<u64, String>,
    open_step: Option<String>,
    shown_failures: HashMap<ReviewKey, DateTime<Utc>>,
    notices: ReadyNotices,
    pending_output: Vec<String>,
}

impl App {
    pub fn configure_preparation_repo(&mut self, remote: Option<&str>, repo_url: Option<&str>) {
        self.preparation_ui.repo_flag = remote
            .map(|remote| ("--remote", remote.into()))
            .or_else(|| repo_url.map(|url| ("--repo-url", url.into())));
    }

    pub(super) fn refresh_preparation_ui(&mut self) {
        self.preparation_ui.next_poll = None;
    }

    pub(super) fn watch_preparation(&mut self, key: ReviewKey, since: DateTime<Utc>) {
        self.preparation_ui.notices.watch(key, since);
        self.refresh_preparation_ui();
    }

    pub(crate) fn pr_preparation_label(&self, number: u64) -> &str {
        self.preparation_ui
            .pr_rows
            .get(&number)
            .map_or(NOT_PREPARED, String::as_str)
    }

    pub(crate) fn open_preparation_step(&self) -> Option<&str> {
        self.preparation_ui.open_step.as_deref()
    }

    /// The TUI loop writes notifications between frames, through its own writer.
    pub fn write_ready_notifications(&mut self, writer: &mut impl Write) -> io::Result<()> {
        if self.preparation_ui.pending_output.is_empty() {
            return Ok(());
        }
        for sequence in self.preparation_ui.pending_output.drain(..) {
            writer.write_all(sequence.as_bytes())?;
        }
        writer.flush()
    }

    pub fn poll_preparation_ui(&mut self, notify_ready: bool, in_tmux: bool) -> bool {
        let view = self.showing_target_selector().then_some(self.target_tab);
        let now = Instant::now();
        let ui = &mut self.preparation_ui;
        if ui.last_view == view && ui.next_poll.is_some_and(|next| next > now) {
            return false;
        }
        ui.last_view = view;
        ui.next_poll = Some(now + POLL_INTERVAL);
        let now = Utc::now();
        ui.notices.begin_sample();
        let redraw = match view {
            Some(TargetTab::PullRequests) => {
                let mut rows = HashMap::new();
                for row in self.pr_tab.view().rows {
                    let summary = row.summary;
                    let key = ReviewKey::for_repository(&summary.repository, summary.number);
                    let state = crate::prepare::status(&key);
                    let stale = state.as_ref().is_some_and(|state| {
                        prepared_head_differs(
                            state,
                            summary.repository.kind,
                            summary.head_sha.as_deref(),
                        )
                    });
                    ui.observe(
                        key,
                        state.as_ref(),
                        &summary.title,
                        now,
                        notify_ready,
                        in_tmux,
                    );
                    if let Some(state) = state {
                        rows.insert(summary.number, preparation_label(Some(&state), stale));
                    }
                }
                let changed = rows != ui.pr_rows;
                ui.pr_rows = rows;
                changed
            }
            Some(TargetTab::Sessions) => {
                let root = self
                    .local_repo_root
                    .as_deref()
                    .unwrap_or(&self.vcs_info.root_path);
                let open = match &self.diff_source {
                    DiffSource::PullRequest(pr)
                        if self.pr_open_state.is_none()
                            && self.pr_reload_state.is_none()
                            && self.pr_range_reload_state.is_none() =>
                    {
                        self.cached_review_digest();
                        self.review_map
                            .digest
                            .get()
                            .map(|digest| (pr.as_ref(), digest.as_str()))
                    }
                    _ => None,
                };
                let ui = &mut self.preparation_ui;
                let rows = prepared_reviews(
                    root,
                    self.forge_repository.as_ref(),
                    self.pr_tab.loaded_rows(),
                    open,
                    |key, state, title| {
                        ui.observe(key.clone(), Some(state), title, now, notify_ready, in_tmux);
                    },
                );
                let changed = self.sessions_tab.update_guide_rows(rows);
                self.sessions_tab
                    .ensure_cursor_visible(self.sessions_list_viewport_height);
                changed
            }
            _ => false,
        };
        let mut open_step = None;
        let mut failure = None;
        if let Some(key) = self.review_key() {
            let state = crate::prepare::status(&key);
            let current = state.as_ref().filter(|state| {
                state.diff.as_deref().map_or_else(
                    || match &self.diff_source {
                        DiffSource::PullRequest(pr) => !prepared_head_differs(
                            state,
                            pr.key.repository.kind,
                            Some(&pr.key.head_sha),
                        ),
                        _ => true,
                    },
                    |diff| diff == self.cached_review_digest(),
                )
            });
            let ui = &mut self.preparation_ui;
            let title = self
                .pr_info
                .as_ref()
                .map_or("Local review", |info| info.details.title.as_str());
            ui.observe(
                key.clone(),
                state.as_ref(),
                title,
                now,
                notify_ready,
                in_tmux,
            );
            if let Some(state) = current {
                match state.status {
                    PrepStatus::Preparing => open_step = Some(step_label(state)),
                    PrepStatus::Failed
                        if ui.shown_failures.get(&key) != Some(&state.started_at) =>
                    {
                        ui.shown_failures.insert(key, state.started_at);
                        failure = Some(format!(
                            "Review preparation failed · :prepare to retry · {}",
                            failure_label(state)
                        ));
                    }
                    _ => {}
                }
            }
        }
        let ui = &mut self.preparation_ui;
        if notify_ready {
            ui.notices.poll_hidden(crate::prepare::status, |state| {
                queue_notification(&mut ui.pending_output, state, "Review guide", in_tmux);
            });
        }
        ui.notices.end_sample();
        let step_changed = open_step != ui.open_step;
        ui.open_step = open_step;
        if let Some(failure) = failure {
            self.set_error(failure);
            return true;
        }
        redraw || step_changed
    }
}

impl PreparationUi {
    fn observe(
        &mut self,
        key: ReviewKey,
        state: Option<&PrepState>,
        fallback_title: &str,
        now: DateTime<Utc>,
        notify_ready: bool,
        in_tmux: bool,
    ) {
        if notify_ready
            && self.notices.observe(key, state, now)
            && let Some(state) = state
        {
            queue_notification(&mut self.pending_output, state, fallback_title, in_tmux);
        }
    }
}

fn queue_notification(output: &mut Vec<String>, state: &PrepState, title: &str, in_tmux: bool) {
    output.push(notification_sequence(
        state
            .title
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or(title),
        &ready_size(state, ", "),
        in_tmux,
    ));
}

fn prepared_reviews(
    root: &Path,
    forge: Option<&crate::forge::traits::ForgeRepository>,
    loaded_prs: &[PullRequestSummary],
    open_pr: Option<(&PullRequestDiffSource, &str)>,
    mut observe: impl FnMut(&ReviewKey, &PrepState, &str),
) -> Vec<PreparedReview> {
    let Ok(dir) = guides_dir().and_then(std::fs::read_dir) else {
        return Vec::new();
    };
    let mut rows: Vec<_> = dir
        .filter_map(|entry| {
            let key = ReviewKey::from_digest(entry.ok()?.file_name().into_string().ok()?)?;
            let state = crate::prepare::status(&key)?;
            if !matches!(state.status, PrepStatus::Preparing | PrepStatus::Ready) {
                return None;
            }
            let belongs = match state.target.as_ref()? {
                PrepTarget::Local { repo_root, .. } => repo_root == root,
                PrepTarget::PullRequest {
                    repository, number, ..
                } => forge.is_some_and(|forge| {
                    key == ReviewKey::for_repository(forge, *number)
                        && key == ReviewKey::for_repository(repository, *number)
                }),
            };
            if !belongs {
                return None;
            }
            let label = session_preparation_label(&state, loaded_prs, open_pr);
            let title = prepared_review_title(state.target.as_ref()?, state.title.as_deref());
            observe(&key, &state, &title);
            Some(PreparedReview {
                key,
                target: state.target?,
                title,
                label,
                started_at: state.started_at,
            })
        })
        .collect();
    rows.sort_unstable_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| a.key.as_str().cmp(b.key.as_str()))
    });
    rows
}

fn session_preparation_label(
    state: &PrepState,
    loaded_prs: &[PullRequestSummary],
    open_pr: Option<(&PullRequestDiffSource, &str)>,
) -> String {
    let Some(PrepTarget::PullRequest {
        repository,
        number,
        head_sha,
    }) = state
        .target
        .as_ref()
        .filter(|_| state.status == PrepStatus::Ready)
    else {
        return preparation_label(Some(state), false);
    };
    let key = ReviewKey::for_repository(repository, *number);
    let listed_head = loaded_prs
        .iter()
        .find(|row| {
            row.number == *number && ReviewKey::for_repository(&row.repository, row.number) == key
        })
        .and_then(|row| row.head_sha.as_deref())
        .filter(|head| !head.is_empty());
    if prepared_head_differs(state, repository.kind, listed_head) {
        return "out of date".into();
    }
    if let Some((pr, digest)) = open_pr
        && pr.key.number == *number
        && ReviewKey::for_repository(&pr.key.repository, pr.key.number) == key
    {
        return preparation_label(Some(state), state.diff.as_deref() != Some(digest));
    }
    if listed_head.is_some() && !head_sha.is_empty() {
        return preparation_label(Some(state), false);
    }
    if head_sha.is_empty() {
        "ready (head unknown)".into()
    } else {
        let short_head: String = single_line(head_sha).chars().take(8).collect();
        format!("ready (prepared for {short_head})")
    }
}

fn prepared_review_title(target: &PrepTarget, stored_title: Option<&str>) -> String {
    let title = single_line(stored_title.unwrap_or("Review guide"));
    match target {
        PrepTarget::PullRequest { number, .. } => format!("#{number} {title}"),
        PrepTarget::Local {
            commits,
            path_filter,
            ..
        } => {
            let mut title = title;
            if let Some(head) = commits.last() {
                title.push_str(" · ");
                title.extend(head.chars().take(8));
            }
            if let Some(path) = path_filter {
                title.push_str(" · ");
                title.push_str(&single_line(path));
            }
            title
        }
    }
}

fn prepared_head_differs(state: &PrepState, forge: ForgeKind, current: Option<&str>) -> bool {
    let Some(current) = current.filter(|head| !head.is_empty()) else {
        return false;
    };
    let Some(PrepTarget::PullRequest { head_sha, .. }) = state.target.as_ref() else {
        return false;
    };
    !head_sha.is_empty() && current != head_sha
        // Bitbucket Cloud's list endpoint can return abbreviated source hashes.
        && !(forge == ForgeKind::Bitbucket && (head_sha.starts_with(current) || current.starts_with(head_sha)))
}

fn preparation_label(state: Option<&PrepState>, stale: bool) -> String {
    match state.map(|state| (state.status, state)) {
        Some((PrepStatus::Ready, _)) if stale => "out of date".into(),
        Some((PrepStatus::Ready, state)) => format!("ready · {}", ready_size(state, " · ")),
        Some((PrepStatus::Preparing, state)) => format!("preparing · {}", step_label(state)),
        Some((PrepStatus::Failed, state)) => format!("failed · {}", failure_label(state)),
        Some((PrepStatus::Cancelled, _)) => "cancelled".into(),
        None => NOT_PREPARED.into(),
    }
}

fn step_label(state: &PrepState) -> String {
    if state.step.is_empty() {
        "starting".into()
    } else {
        single_line(&state.step)
    }
}

fn failure_label(state: &PrepState) -> String {
    let Some(error) = state.error.as_deref() else {
        return "see preparation log".into();
    };
    let error = error.strip_prefix("Invalid input: ").unwrap_or(error);
    let reason = error
        .strip_prefix(state.step.as_str())
        .and_then(|reason| reason.strip_prefix(": "))
        .unwrap_or(error);
    single_line(reason)
}

fn ready_size(state: &PrepState, separator: &str) -> String {
    match state.estimate_minutes {
        Some(minutes) => format!(
            "{} stops{separator}about {minutes} min",
            state.stops.unwrap_or(0)
        ),
        None => format!("{} stops", state.stops.unwrap_or(0)),
    }
}

fn single_line(value: &str) -> String {
    value
        .chars()
        .filter_map(|ch| {
            if matches!(ch, '\t' | '\r' | '\n') {
                Some(' ')
            } else if ch.is_control() {
                None
            } else {
                Some(ch)
            }
        })
        .collect()
}

#[derive(Default)]
struct ReadyNotices {
    previous: HashMap<ReviewKey, DateTime<Utc>>,
    visible: HashMap<ReviewKey, DateTime<Utc>>,
    // Active work outlives the view that first showed it.
    watching: HashMap<ReviewKey, DateTime<Utc>>,
    notified: HashSet<(ReviewKey, DateTime<Utc>)>,
}

impl ReadyNotices {
    fn begin_sample(&mut self) {
        self.visible.clear();
    }

    fn end_sample(&mut self) {
        std::mem::swap(&mut self.visible, &mut self.previous);
    }

    fn watch(&mut self, key: ReviewKey, since: DateTime<Utc>) {
        self.watching.entry(key).or_insert(since);
    }

    fn observe(&mut self, key: ReviewKey, state: Option<&PrepState>, now: DateTime<Utc>) -> bool {
        let visible_since = self.previous.get(&key).copied().unwrap_or(now);
        let watched_since = self.watching.get(&key).copied();
        let notify = state.is_some_and(|state| {
            if state.status == PrepStatus::Preparing {
                self.watch(key.clone(), now);
                return false;
            }
            self.watching.remove(&key);
            mark_notified(
                &mut self.notified,
                &key,
                state,
                watched_since.unwrap_or(visible_since),
            )
        });
        self.visible.insert(key, visible_since);
        notify
    }

    fn poll_hidden(
        &mut self,
        mut status: impl FnMut(&ReviewKey) -> Option<PrepState>,
        mut notify: impl FnMut(&PrepState),
    ) {
        self.watching.retain(|key, since| {
            if self.visible.contains_key(key) {
                return true;
            }
            let Some(state) = status(key) else {
                return false;
            };
            if state.status == PrepStatus::Preparing {
                return true;
            }
            if mark_notified(&mut self.notified, key, &state, *since) {
                notify(&state);
            }
            false
        });
    }
}

fn mark_notified(
    notified: &mut HashSet<(ReviewKey, DateTime<Utc>)>,
    key: &ReviewKey,
    state: &PrepState,
    since: DateTime<Utc>,
) -> bool {
    // A job can finish between polls. The persisted completion time rules
    // out guides that were already ready when the job or view was first seen.
    state.status == PrepStatus::Ready
        && state.finished_at.is_some_and(|finished| finished > since)
        && notified.insert((key.clone(), state.started_at))
}

fn notification_sequence(title: &str, size: &str, tmux: bool) -> String {
    // Unlike OSC 9, OSC 777 uses semicolons to separate title from body.
    let title = single_line(title).replace(';', ",");
    let sequence = format!("\x1b]9;{title} — {size}\x1b\\\x1b]777;notify;{title};{size}\x1b\\");
    // The pane's bell is unwrapped so the multiplexer can report it as well.
    if tmux {
        format!(
            "\x07\x1bPtmux;{}\x1b\\",
            sequence.replace('\x1b', "\x1b\x1b")
        )
    } else {
        format!("\x07{sequence}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forge::traits::ForgeRepository;

    #[test]
    fn preparation_row_labels_include_stalled_and_out_of_date_jobs() {
        assert_eq!(preparation_label(None, false), "not prepared");
        let mut state = PrepState::started("Auditing stops", None, None);
        assert_eq!(
            preparation_label(Some(&state), false),
            "preparing · Auditing stops"
        );
        let dir = tempfile::tempdir().unwrap();
        crate::persistence::storage::set_test_reviews_dir(Some(dir.path().into()));
        let key = ReviewKey::for_local(dir.path(), "worktree");
        state.heartbeat_at -= chrono::Duration::minutes(6);
        crate::review_guide::GuideStore::open(&key)
            .unwrap()
            .save_state(&state)
            .unwrap();
        let stalled = crate::prepare::status(&key).unwrap();
        assert!(
            preparation_label(Some(&stalled), false)
                .starts_with("failed · No progress for 5 minutes")
        );
        crate::persistence::storage::set_test_reviews_dir(None);
        state.status = PrepStatus::Failed;
        state.error = Some("Reviewer is not logged in".into());
        assert_eq!(
            preparation_label(Some(&state), false),
            "failed · Reviewer is not logged in"
        );
        state.status = PrepStatus::Cancelled;
        assert_eq!(preparation_label(Some(&state), false), "cancelled");
        state.status = PrepStatus::Ready;
        state.stops = Some(11);
        state.estimate_minutes = Some(12);
        assert_eq!(
            preparation_label(Some(&state), false),
            "ready · 11 stops · about 12 min"
        );
        let repository = ForgeRepository::github("github.com", "octo", "demo");
        state.target = Some(PrepTarget::PullRequest {
            repository,
            number: 1,
            head_sha: "old".into(),
        });
        let stale = prepared_head_differs(&state, ForgeKind::GitHub, Some("new"));
        assert_eq!(preparation_label(Some(&state), stale), "out of date");
    }

    #[test]
    fn ready_notice_fires_once_per_preparation_and_not_for_preexisting_guides() {
        let key = ReviewKey::for_pull_request("gh", "github.com", "octo", "demo", 1);
        let mut notices = ReadyNotices::default();
        let mut state = PrepState::started("Auditing", None, None);
        let seconds = chrono::Duration::seconds;
        let start = state.started_at;
        let mut sample = |state: &PrepState, at| {
            notices.begin_sample();
            let notify = notices.observe(key.clone(), Some(state), at);
            notices.end_sample();
            notify
        };
        state.status = PrepStatus::Ready;
        state.finished_at = Some(start + seconds(1));
        assert!(!sample(&state, start + seconds(2)));
        state.status = PrepStatus::Preparing;
        state.started_at = start + seconds(3);
        assert!(!sample(&state, start + seconds(4)));
        state.status = PrepStatus::Ready;
        state.finished_at = Some(start + seconds(5));
        assert!(sample(&state, start + seconds(6)));
        assert!(!sample(&state, start + seconds(7)));
        state.started_at = start + seconds(8);
        state.finished_at = Some(start + seconds(9));
        assert!(
            sample(&state, start + seconds(10)),
            "a new job may finish between polls"
        );
    }

    #[test]
    fn ready_notice_follows_preparation_after_its_tab_is_hidden() {
        let key = ReviewKey::for_pull_request("gh", "github.com", "octo", "demo", 1);
        let mut notices = ReadyNotices::default();
        let mut state = PrepState::started("Auditing", None, None);
        let seconds = chrono::Duration::seconds;
        let start = state.started_at;

        notices.begin_sample();
        assert!(!notices.observe(key.clone(), Some(&state), start + seconds(1)));
        notices.end_sample();

        // The next view does not show this PR. The background poll must
        // still find its completion in the store.
        notices.begin_sample();
        state.status = PrepStatus::Ready;
        state.finished_at = Some(start + seconds(2));
        let mut notifications = 0;
        notices.poll_hidden(
            |watched| {
                assert_eq!(watched, &key);
                Some(state.clone())
            },
            |_| notifications += 1,
        );
        notices.end_sample();
        assert_eq!(notifications, 1);

        notices.begin_sample();
        assert!(!notices.observe(key, Some(&state), start + seconds(3)));
        notices.poll_hidden(|_| panic!("no job left to poll"), |_| unreachable!());
        notices.end_sample();
    }

    #[test]
    fn ready_notice_encodes_osc_and_tmux_without_title_controls() {
        let size = "11 stops, about 12 min";
        let output = notification_sequence("Review\x07\n\x1b;星\u{009c}", size, false);
        assert_eq!(
            output,
            format!("\x07\x1b]9;Review ,星 — {size}\x1b\\\x1b]777;notify;Review ,星;{size}\x1b\\")
        );
        assert_eq!(
            notification_sequence("Review\x07\n\x1b;星\u{009c}", size, true),
            format!(
                "\x07\x1bPtmux;{}\x1b\\",
                output[1..].replace('\x1b', "\x1b\x1b")
            )
        );
    }
}
