use std::ffi::OsString;
use std::io;
use std::process::{Child, Command, Stdio};

use chrono::Utc;

use crate::error::{Result, TuicrError};
use crate::review_guide::{
    GuideStore, PrepState, PrepStatus, PrepTarget, ReviewKey, STALL_AFTER_SECS,
};

use super::pipeline::Job;

const WORKER_ARG: &str = "--prepare-worker";
const PREFLIGHT_ARG: &str = "--prepare-preflight";

pub fn status(key: &ReviewKey) -> Option<PrepState> {
    GuideStore::open(key).ok()?.state().ok()?.map(effective)
}

fn effective(mut state: PrepState) -> PrepState {
    let status = state.effective_status(Utc::now());
    if state.status != status {
        state.status = status;
        state.error = Some(stall_reason(&state.step));
    }
    state
}

fn stall_reason(step: &str) -> String {
    format!(
        "No progress for {} minutes during '{step}'; retry preparation or check prepare.log",
        STALL_AFTER_SECS / 60
    )
}

pub(super) enum Start {
    Started(Child, JobIdentity),
    Preparing(JobIdentity),
    Ready,
    PreviousFailure,
}

#[derive(Clone, Copy)]
pub(super) struct JobIdentity {
    pid: Option<u32>,
    started_at: chrono::DateTime<Utc>,
}

impl From<&PrepState> for JobIdentity {
    fn from(state: &PrepState) -> Self {
        Self {
            pid: state.pid,
            started_at: state.started_at,
        }
    }
}

/// Wait for this specific job, not a later preparation for the same review.
pub(super) fn wait_for_job(
    store: &GuideStore,
    identity: JobIdentity,
    diff: &str,
    mut child: Option<Child>,
) -> Result<PrepState> {
    loop {
        // Check exit before reading the state so an exit concurrent with the
        // read cannot hide the terminal state it already wrote.
        let exit = child.as_mut().map(Child::try_wait).transpose()?.flatten();
        if let Some(state) = observe_job(store, identity, diff)? {
            return Ok(state);
        }
        if let Some(exit) = exit {
            return Err(wait_error(
                store,
                format!("Worker exited with {exit} before reporting completion"),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn observe_job(store: &GuideStore, identity: JobIdentity, diff: &str) -> Result<Option<PrepState>> {
    let state = store
        .state()?
        .filter(|state| state.pid == identity.pid && state.started_at == identity.started_at)
        .map(effective)
        .ok_or_else(|| wait_error(store, "The preparation's state is missing or was replaced"))?;
    match state.status {
        PrepStatus::Preparing => Ok(None),
        PrepStatus::Ready if state.diff.as_deref() == Some(diff) => Ok(Some(state)),
        PrepStatus::Ready => Err(wait_error(
            store,
            "The running preparation was for a different diff; run `prepare` again for this one",
        )),
        PrepStatus::Failed => Err(wait_error(
            store,
            state
                .error
                .unwrap_or_else(|| "Review preparation failed".into()),
        )),
        PrepStatus::Cancelled => Err(wait_error(store, "Review preparation was cancelled")),
    }
}

fn wait_error(store: &GuideStore, message: impl std::fmt::Display) -> TuicrError {
    TuicrError::InvalidInput(format!("{message}. See {}", store.log_path().display()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartMode {
    Cli,
    Manual,
    Automatic,
}

/// Start a headless CLI preparation independently of the caller's terminal.
/// `prepare` loads the requested target and starts its private, logged worker.
pub fn spawn_detached(target_args: &[OsString]) -> io::Result<()> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("prepare")
        .args(target_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command, None)?;
    reap(command.spawn()?);
    Ok(())
}

pub(super) fn reap(mut child: Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

#[cfg(unix)]
fn detach(command: &mut Command, inherited_fd: Option<i32>) -> io::Result<()> {
    use std::os::unix::process::CommandExt;
    // Only async-signal-safe operations may run after fork, before exec.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if let Some(fd) = inherited_fd
                && libc::fcntl(fd, libc::F_SETFD, 0) == -1
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn detach(_command: &mut Command, _inherited_fd: Option<i32>) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Background review preparation requires Unix",
    ))
}

#[cfg(unix)]
pub(super) fn start_job(
    key: &ReviewKey,
    store: &GuideStore,
    job: &Job<'_>,
    target: PrepTarget,
    mode: StartMode,
) -> Result<Start> {
    use std::io::{Seek, Write};
    use std::os::fd::AsRawFd;

    let _lock = unix::lock(store)?;
    let previous = store.state()?;
    if let Some(running) = previous
        .as_ref()
        .filter(|state| state.effective_status(Utc::now()) == PrepStatus::Preparing)
    {
        return Ok(Start::Preparing(running.into()));
    }
    if mode == StartMode::Automatic
        && previous.as_ref().is_some_and(|state| {
            state.effective_status(Utc::now()) == PrepStatus::Failed
                && previous_diff_matches(state, &target, &job.diff)
        })
    {
        return Ok(Start::PreviousFailure);
    }
    if job.supplied.is_none()
        && let Some(guide) = store.guide()?
    {
        let rules = super::current_rules_digest()?;
        if super::guide_is_current(&guide, &job.diff, &rules) {
            return Ok(Start::Ready);
        }
    }
    let mut state = PrepState::started(
        "Starting review preparation",
        Some(job.diff.clone()),
        Some(job.title.clone()),
    );
    state.target = Some(target);
    launch(key, store, previous, state, |mut command| {
        // Anonymous, owner-only file: reviewed source never appears in argv or
        // survives as a named file, including on a crash or cancellation.
        let mut request = tempfile::tempfile_in(store.dir())?;
        {
            let mut output = io::BufWriter::new(&mut request);
            serde_json::to_writer(&mut output, job)?;
            output.flush()?;
        }
        request.rewind()?;
        let fd = request.as_raw_fd();
        command.args(["prepare", WORKER_ARG, key.as_str(), &fd.to_string()]);
        detach(&mut command, Some(fd))?;
        command.spawn().map_err(Into::into)
    })
}

#[cfg(unix)]
fn previous_diff_matches(state: &PrepState, target: &PrepTarget, diff: &str) -> bool {
    if let Some(previous) = &state.diff {
        return previous == diff;
    }
    if let (
        Some(PrepTarget::PullRequest {
            head_sha: before, ..
        }),
        PrepTarget::PullRequest {
            head_sha: current, ..
        },
    ) = (&state.target, target)
    {
        return head_matches(before, current);
    }
    true
}

#[cfg(unix)]
fn head_matches(expected: &str, actual: &str) -> bool {
    expected == actual
        || expected.is_empty()
        || (expected.bytes().all(|byte| byte.is_ascii_hexdigit())
            && actual
                .get(..expected.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(expected)))
}

/// Queue preparation without opening the review. The reserved state tracks
/// failures even before the diff has been loaded or the digest is known.
#[cfg(unix)]
pub fn spawn_tracked(
    target_args: &[OsString],
    key: &ReviewKey,
    target: &PrepTarget,
    title: &str,
) -> io::Result<()> {
    let store = GuideStore::open(key)?;
    let _lock = unix::lock(&store)?;
    let previous = store.state()?;
    if previous
        .as_ref()
        .is_some_and(|state| state.effective_status(Utc::now()) == PrepStatus::Preparing)
    {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "A review guide is already being prepared for this review",
        ));
    }
    let mut state = PrepState::started("Loading the selected review", None, Some(title.into()));
    state.target = Some(target.clone());
    let result = launch(key, &store, previous, state, |mut command| {
        command
            .args(["prepare", PREFLIGHT_ARG, key.as_str()])
            .args(target_args);
        detach(&mut command, None)?;
        command.spawn().map_err(Into::into)
    })
    .map_err(io::Error::other)?;
    if let Start::Started(child, _) = result {
        reap(child);
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn spawn_tracked(
    _target_args: &[OsString],
    _key: &ReviewKey,
    _target: &PrepTarget,
    _title: &str,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Background review preparation requires Unix",
    ))
}

#[cfg(unix)]
fn launch(
    key: &ReviewKey,
    store: &GuideStore,
    previous: Option<PrepState>,
    mut state: PrepState,
    spawn: impl FnOnce(Command) -> Result<Child>,
) -> Result<Start> {
    let started = (|| {
        if let Some(old) = previous.filter(|state| state.is_stalled(Utc::now())) {
            unix::stop_worker(key, old.pid)?;
        }
        let log = unix::open_private(&store.log_path())?;
        log.set_len(0)?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        spawn(command)
    })();
    match started {
        Ok(mut child) => {
            state.pid = Some(child.id());
            if let Err(error) = store.save_state(&state) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
            Ok(Start::Started(child, (&state).into()))
        }
        Err(error) => {
            state.pid = None;
            finish(&mut state, PrepStatus::Failed, Some(error.to_string()));
            store.save_state(&state)?;
            Err(error)
        }
    }
}

#[cfg(not(unix))]
pub(super) fn start_job(
    _key: &ReviewKey,
    _store: &GuideStore,
    _job: &Job<'_>,
    _target: PrepTarget,
    _mode: StartMode,
) -> Result<Start> {
    Err(TuicrError::UnsupportedOperation(
        "Background review preparation requires Unix".into(),
    ))
}

pub fn cancel(key: &ReviewKey) -> io::Result<()> {
    #[cfg(unix)]
    return unix::cancel(key);
    #[cfg(not(unix))]
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Cancelling review preparation requires Unix",
    ))
}

fn finish(state: &mut PrepState, status: PrepStatus, error: Option<String>) {
    let now = Utc::now();
    state.status = status;
    state.heartbeat_at = now;
    state.finished_at = Some(now);
    state.error = error;
}

pub fn run_worker_if_invoked() -> Option<Result<()>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("prepare"))
        || args.next().as_deref() != Some(std::ffi::OsStr::new(WORKER_ARG))
    {
        return None;
    }
    #[cfg(unix)]
    return Some(unix::run_worker(args));
    #[cfg(not(unix))]
    Some(Err(TuicrError::UnsupportedOperation(
        "Background review preparation requires Unix".into(),
    )))
}

pub struct TrackedPreparation {
    key: ReviewKey,
    store: GuideStore,
}

impl TrackedPreparation {
    pub fn report_failure(&self, error: String) -> io::Result<()> {
        #[cfg(unix)]
        return unix::report_preflight_failure(&self.store, error);
        #[cfg(not(unix))]
        {
            drop(error);
            Ok(())
        }
    }

    pub(super) fn prepare(&self, key: &ReviewKey, target: PrepTarget, job: Job<'_>) -> Result<()> {
        if self.key != *key {
            return Err(TuicrError::InvalidInput(
                "The selected review changed before preparation; open the current PR and try again"
                    .into(),
            ));
        }
        #[cfg(unix)]
        return unix::run_tracked(&self.store, target, job);
        #[cfg(not(unix))]
        Err(TuicrError::UnsupportedOperation(
            "Background review preparation requires Unix".into(),
        ))
    }
}

pub fn tracked_preparation() -> Result<Option<(TrackedPreparation, Vec<OsString>)>> {
    let mut args = std::env::args_os();
    let exe = args.next().unwrap_or_else(|| "tuicr".into());
    if args.next().as_deref() != Some(std::ffi::OsStr::new("prepare"))
        || args.next().as_deref() != Some(std::ffi::OsStr::new(PREFLIGHT_ARG))
    {
        return Ok(None);
    }
    let key = args
        .next()
        .and_then(|arg| arg.into_string().ok())
        .and_then(ReviewKey::from_digest)
        .ok_or_else(|| TuicrError::InvalidInput("Invalid preparation review key".into()))?;
    let argv = [exe, "prepare".into()].into_iter().chain(args).collect();
    let store = GuideStore::open(&key)?;
    Ok(Some((TrackedPreparation { key, store }, argv)))
}

#[cfg(unix)]
mod unix {
    use std::fs::{self, File, OpenOptions};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::review_agent::Cancel;
    use crate::review_guide::ReviewGuide;

    static STOP: AtomicBool = AtomicBool::new(false);

    extern "C" fn request_stop(_: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }

    pub(super) fn lock(store: &GuideStore) -> io::Result<File> {
        let base = store
            .dir()
            .parent()
            .ok_or_else(|| io::Error::other("Invalid review store path"))?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(base)?;
        ensure_private_dir(base)?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(store.dir())?;
        ensure_private_dir(store.dir())?;
        let file = open_private(&store.dir().join("prepare.lock"))?;
        loop {
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                return Ok(file);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    fn ensure_private_dir(path: &Path) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir()
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} must be an owner-only directory", path.display()),
            ));
        }
        Ok(())
    }

    pub(super) fn open_private(path: &Path) -> io::Result<File> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.mode() & 0o077 != 0 || meta.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} must be an owner-only regular file", path.display()),
            ));
        }
        Ok(file)
    }

    pub(super) fn cancel(key: &ReviewKey) -> io::Result<()> {
        let store = GuideStore::open(key)?;
        let _lock = lock(&store)?;
        let mut state = store
            .state()?
            .filter(|state| state.status == PrepStatus::Preparing)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "No review preparation is running")
            })?;
        stop_worker(key, state.pid)?;
        finish(&mut state, PrepStatus::Cancelled, None);
        state.step = "Preparation cancelled".into();
        store.save_state(&state)
    }

    pub(super) fn stop_worker(key: &ReviewKey, pid: Option<u32>) -> io::Result<()> {
        let Some(pid) = pid
            .and_then(|pid| i32::try_from(pid).ok())
            .filter(|pid| *pid > 1)
        else {
            return Ok(());
        };
        let current_exe = std::env::current_exe()?;
        let output = Command::new("/bin/ps")
            .args(["-ww", "-p", &pid.to_string(), "-o", "command="])
            .output()?;
        if !output.status.success() {
            return Ok(());
        }
        let expected = format!(
            "{} prepare {WORKER_ARG} {} ",
            current_exe.display(),
            key.as_str()
        );
        let command = String::from_utf8_lossy(&output.stdout);
        let command = command.trim();
        let preflight = format!(
            "{} prepare {PREFLIGHT_ARG} {} ",
            current_exe.display(),
            key.as_str()
        );
        let matches_worker = command
            .strip_prefix(&expected)
            .is_some_and(|fd| fd.parse::<i32>().is_ok())
            || command.starts_with(&preflight);
        // Detached workers are both the process group and session leader.
        if !matches_worker
            || unsafe { libc::getpgid(pid) } != pid
            || unsafe { libc::getsid(pid) } != pid
        {
            return Ok(());
        }
        if unsafe { libc::kill(-pid, libc::SIGTERM) } == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }

    pub(super) fn run_worker(mut args: impl Iterator<Item = OsString>) -> Result<()> {
        let mut next = || args.next().and_then(|arg| arg.into_string().ok());
        let key = next()
            .and_then(ReviewKey::from_digest)
            .ok_or_else(|| TuicrError::InvalidInput("Invalid preparation review key".into()))?;
        let fd = next()
            .and_then(|fd| fd.parse::<i32>().ok())
            .filter(|fd| *fd > 2)
            .ok_or_else(|| {
                TuicrError::InvalidInput("Missing private preparation request".into())
            })?;
        let store = GuideStore::open(&key)?;
        let Some(state) = current_state(&store)? else {
            return Ok(());
        };
        execute(&store, state, read_handoff(fd))
    }

    pub(super) fn report_preflight_failure(store: &GuideStore, error: String) -> io::Result<()> {
        let _lock = lock(store)?;
        if let Some(mut state) = read_owned_state(store)? {
            finish(&mut state, PrepStatus::Failed, Some(error));
            store.save_state(&state)?;
        }
        Ok(())
    }

    pub(super) fn run_tracked(store: &GuideStore, target: PrepTarget, job: Job<'_>) -> Result<()> {
        let state = {
            let _lock = lock(store)?;
            let Some(mut state) = read_owned_state(store)? else {
                return Ok(());
            };
            if state.is_stalled(Utc::now()) {
                return Err(
                    io::Error::new(io::ErrorKind::TimedOut, stall_reason(&state.step)).into(),
                );
            }
            if let (
                Some(PrepTarget::PullRequest {
                    head_sha: expected, ..
                }),
                PrepTarget::PullRequest {
                    head_sha: actual, ..
                },
            ) = (&state.target, &target)
                && !head_matches(expected, actual)
            {
                return Err(TuicrError::InvalidInput(
                    "The PR head changed before preparation; reload the PR list and try again"
                        .into(),
                ));
            }
            state.target = Some(target);
            state.title = Some(job.title.clone());
            state.diff = Some(job.diff.clone());
            state.heartbeat_at = Utc::now();
            store.save_state(&state)?;
            state
        };
        execute(store, state, Ok(job))
    }

    fn execute(store: &GuideStore, mut state: PrepState, job: Result<Job<'_>>) -> Result<()> {
        let result = job.and_then(|job| run_pipeline(store, job, &mut state));
        let _lock = lock(store)?;
        if read_owned_state(store)?.is_none() {
            return Ok(());
        }
        let result = result.and_then(|guide| {
            store.save_guide(&guide)?;
            state.stops = u32::try_from(guide.stop_count()).ok();
            state.estimate_minutes = guide.estimate_minutes;
            state.step = "Review guide ready".into();
            finish(&mut state, PrepStatus::Ready, None);
            Ok(())
        });
        if let Err(error) = &result {
            eprintln!("Review preparation failed: {error}");
            finish(&mut state, PrepStatus::Failed, Some(error.to_string()));
        }
        store.save_state(&state)?;
        if state.status == PrepStatus::Ready {
            eprintln!(
                "Review guide ready ({} stops, about {} min)",
                state.stops.unwrap_or_default(),
                state.estimate_minutes.unwrap_or_default()
            );
        }
        result
    }

    fn current_state(store: &GuideStore) -> io::Result<Option<PrepState>> {
        let _lock = lock(store)?;
        read_owned_state(store)
    }

    fn read_owned_state(store: &GuideStore) -> io::Result<Option<PrepState>> {
        Ok(store.state()?.filter(|state| {
            state.status == PrepStatus::Preparing && state.pid == Some(std::process::id())
        }))
    }

    fn read_handoff(fd: i32) -> Result<Job<'static>> {
        // Reset close-on-exec before invoking any children from this process.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error().into());
        }
        let request = unsafe { File::from_raw_fd(fd) };
        Ok(serde_json::from_reader(io::BufReader::new(request))?)
    }

    fn run_pipeline(
        store: &GuideStore,
        job: Job<'_>,
        state: &mut PrepState,
    ) -> Result<ReviewGuide> {
        let signal = unsafe {
            libc::signal(
                libc::SIGTERM,
                request_stop as *const () as libc::sighandler_t,
            )
        };
        if signal == libc::SIG_ERR {
            return Err(io::Error::last_os_error().into());
        }
        let cancel = Cancel::default();
        std::thread::scope(|scope| {
            let (done, pending) = std::sync::mpsc::channel::<()>();
            let watch_cancel = cancel.clone();
            scope.spawn(move || {
                while pending.recv_timeout(Duration::from_millis(100))
                    == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                {
                    if STOP.load(Ordering::Relaxed) {
                        watch_cancel.cancel();
                        break;
                    }
                }
            });
            let result = super::super::pipeline::prepare(job, store, state, &cancel, |state| {
                let _lock = lock(store)?;
                let old = read_owned_state(store)?
                    .filter(|_| !STOP.load(Ordering::Relaxed))
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::Interrupted,
                            "Review preparation was cancelled or replaced",
                        )
                    })?;
                if old.is_stalled(Utc::now()) {
                    return Err(
                        io::Error::new(io::ErrorKind::TimedOut, stall_reason(&old.step)).into(),
                    );
                }
                if old.step != state.step {
                    eprintln!("{}", state.step);
                }
                store.save_state(state)?;
                Ok(())
            });
            drop(done);
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_worker_key_accepts_only_store_digests() {
        let key = ReviewKey::for_local(std::path::Path::new("repo"), "staged");
        assert_eq!(ReviewKey::from_digest(key.as_str().into()), Some(key));
        assert!(ReviewKey::from_digest("../wrong-store".into()).is_none());
        assert!(ReviewKey::from_digest("F".repeat(64)).is_none());
    }

    #[test]
    fn preparation_state_stall_allows_a_new_start() {
        let dir = tempfile::tempdir().unwrap();
        let store = GuideStore::in_dir(dir.path(), &ReviewKey::for_local(dir.path(), "worktree"));
        let mut state = PrepState::started("Writing", Some("digest".into()), None);
        store.save_state(&state).unwrap();
        assert_eq!(
            effective(store.state().unwrap().unwrap()).status,
            PrepStatus::Preparing
        );
        state.heartbeat_at -= chrono::Duration::seconds(STALL_AFTER_SECS + 1);
        store.save_state(&state).unwrap();
        let stalled = effective(store.state().unwrap().unwrap());
        assert_eq!(stalled.status, PrepStatus::Failed);
        assert!(stalled.error.unwrap().contains("No progress"));
        assert_eq!(
            store.state().unwrap().unwrap().status,
            PrepStatus::Preparing
        );
    }

    #[cfg(unix)]
    #[test]
    fn preparation_preflight_failure_persists_actionable_state() {
        let dir = tempfile::tempdir().unwrap();
        let key = ReviewKey::for_local(dir.path(), "worktree");
        let store = GuideStore::in_dir(&dir.path().join("guides"), &key);
        let state = PrepState::started(
            "Loading the selected review",
            None,
            Some("Review title".into()),
        );
        store.save_state(&state).unwrap();
        unix::report_preflight_failure(&store, "Selected range missing; reload this PR".into())
            .unwrap();
        let state = store.state().unwrap().unwrap();
        assert_eq!(state.status, PrepStatus::Failed);
        assert!(state.finished_at.is_some());
        assert_eq!(state.title.as_deref(), Some("Review title"));
        assert_eq!(
            state.error.as_deref(),
            Some("Selected range missing; reload this PR")
        );
        assert!(store.guide().unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn preparation_duplicate_start_refuses_existing_owner() {
        let dir = tempfile::tempdir().unwrap();
        let key = ReviewKey::for_local(dir.path(), "worktree");
        let store = GuideStore::in_dir(&dir.path().join("guides"), &key);
        let previous = PrepState::started("Auditing", Some("different diff".into()), None);
        store.save_state(&previous).unwrap();
        let job = Job {
            files: std::borrow::Cow::Borrowed(&[]),
            source: crate::review_agent::SourceRequest::local(
                dir.path().into(),
                crate::vcs::traits::VcsType::Git,
                crate::review_agent::LocalTarget::Worktree,
                false,
            ),
            diff: "current diff".into(),
            title: "Test review".into(),
            context: serde_json::Value::Null,
            supplied: None,
        };
        let target = PrepTarget::Local {
            repo_root: dir.path().into(),
            source: crate::model::SessionDiffSource::WorkingTree,
            commits: Vec::new(),
            path_filter: None,
            revision: None,
        };
        assert!(matches!(
            start_job(&key, &store, &job, target.clone(), StartMode::Cli).unwrap(),
            Start::Preparing(_)
        ));
        assert_eq!(store.state().unwrap().as_ref(), Some(&previous));
        assert!(!store.log_path().exists());

        let mut failed = previous;
        failed.diff = None;
        finish(
            &mut failed,
            PrepStatus::Failed,
            Some("could not load".into()),
        );
        store.save_state(&failed).unwrap();
        assert!(matches!(
            start_job(&key, &store, &job, target, StartMode::Automatic).unwrap(),
            Start::PreviousFailure
        ));
        assert_eq!(store.state().unwrap(), Some(failed));
    }

    #[test]
    fn preparation_wait_observes_only_the_joined_job_and_requested_diff() {
        let dir = tempfile::tempdir().unwrap();
        let key = ReviewKey::for_local(dir.path(), "worktree");
        let store = GuideStore::in_dir(dir.path(), &key);
        let mut state = PrepState::started("Writing", Some("expected".into()), None);
        let identity = (&state).into();
        store.save_state(&state).unwrap();
        assert!(observe_job(&store, identity, "expected").unwrap().is_none());

        state.heartbeat_at -= chrono::Duration::seconds(STALL_AFTER_SECS + 1);
        store.save_state(&state).unwrap();
        assert!(
            observe_job(&store, identity, "expected")
                .unwrap_err()
                .to_string()
                .contains("No progress")
        );

        for (status, error, expected) in [
            (
                PrepStatus::Failed,
                Some("claude unavailable".into()),
                "claude unavailable",
            ),
            (PrepStatus::Cancelled, None, "cancelled"),
        ] {
            finish(&mut state, status, error);
            store.save_state(&state).unwrap();
            assert!(
                observe_job(&store, identity, "expected")
                    .unwrap_err()
                    .to_string()
                    .contains(expected)
            );
        }

        finish(&mut state, PrepStatus::Ready, None);
        store.save_state(&state).unwrap();
        assert_eq!(
            observe_job(&store, identity, "expected").unwrap(),
            Some(state.clone())
        );
        assert!(
            observe_job(&store, identity, "different")
                .unwrap_err()
                .to_string()
                .contains("different diff")
        );

        state.started_at += chrono::Duration::seconds(1);
        store.save_state(&state).unwrap();
        assert!(
            observe_job(&store, identity, "expected")
                .unwrap_err()
                .to_string()
                .contains("replaced")
        );
    }
}
