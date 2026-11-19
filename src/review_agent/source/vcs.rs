use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::forge::traits::{ForgeKind, ForgeRepository};
use crate::vcs::traits::VcsType;

use super::{LocalTarget, MAX_FILE_BYTES, is_full_object_id, is_object_id};

const LOCAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn is_git_checkout(root: &Path) -> bool {
    fs::symlink_metadata(root.join(".git")).is_ok_and(|m| m.is_file() || m.is_dir())
}

pub(super) fn is_colocated_jj(root: &Path) -> bool {
    if !is_git_checkout(root) {
        return false;
    }
    let git_dir = |command: &mut Command| {
        let raw = read_command(command, 4096, false)?;
        let text = std::str::from_utf8(&raw)
            .ok()?
            .trim_end_matches(['\r', '\n']);
        root.join(text).canonicalize().ok()
    };
    git_dir(jj(root).args(["git", "root"])).is_some_and(|jj_dir| {
        git_dir(git(root).args(["rev-parse", "--absolute-git-dir"])).as_deref()
            == Some(jj_dir.as_path())
    })
}

fn jj(root: &Path) -> Command {
    let mut command = Command::new("jj");
    command.current_dir(root).args([
        "--ignore-working-copy",
        "--at-op=@",
        "--no-pager",
        "--color=never",
    ]);
    command
}

pub(super) fn git(root: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .current_dir(root)
        .args([
            "--no-pager",
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
        ])
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    for key in [
        "GIT_DIR",
        "GIT_COMMON_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_ATTR_SOURCE",
        "GIT_CONFIG_PARAMETERS",
        "GIT_CONFIG_COUNT",
    ] {
        command.env_remove(key);
    }
    command
}

/// A Git command that may use any transport: a local clone, or a fetch from
/// the remote the user's own checkout already trusts.
pub(super) fn git_network(root: &Path) -> Command {
    let mut command = git(root);
    command
        .env_remove("GIT_ALLOW_PROTOCOL")
        .env("GIT_TERMINAL_PROMPT", "0");
    command
}

/// Like `git_network`, but a partial clone may fetch missing blobs on demand.
pub(super) fn git_lazy(root: &Path) -> Command {
    let mut command = git_network(root);
    command.env_remove("GIT_NO_LAZY_FETCH");
    command
}

fn hg(root: &Path) -> Command {
    let mut command = Command::new("hg");
    command
        .current_dir(root)
        .env("HGPLAIN", "1")
        .env("HGRCPATH", "");
    command
}

fn read_command(command: &mut Command, cap: usize, allow_prefix: bool) -> Option<Vec<u8>> {
    crate::process::read_bounded_command(command, cap, LOCAL_COMMAND_TIMEOUT, allow_prefix)
}

pub(super) fn git_text(root: &Path, revision: &str, path: &Path) -> Option<Vec<u8>> {
    let spec = format!("{revision}:{}", path.to_str()?);
    let mut command = git(root);
    if revision.is_empty()
        && let Some(index) = std::env::var_os("GIT_INDEX_FILE")
    {
        command.env("GIT_INDEX_FILE", index);
    }
    read_command(
        command.args(["cat-file", "blob", &spec]),
        MAX_FILE_BYTES as usize,
        false,
    )
}

pub(super) fn hg_text(root: &Path, revision: &str, path: &Path) -> Option<Vec<u8>> {
    read_command(
        hg(root).args(["cat", "-r", revision, "--"]).arg(path),
        MAX_FILE_BYTES as usize,
        false,
    )
}

pub(super) fn output_id(command: &mut Command) -> Option<String> {
    let output = read_command(command, 128, false)?;
    let id = std::str::from_utf8(&output).ok()?.trim();
    is_full_object_id(id).then(|| id.to_owned())
}

pub(super) fn has_commit(root: &Path, sha: &str) -> bool {
    read_command(
        git(root).args([
            "cat-file",
            "-e",
            "--end-of-options",
            &format!("{sha}^{{commit}}"),
        ]),
        0,
        false,
    )
    .is_some()
}

pub(super) fn git_revision(root: &Path, vcs: VcsType, target: &LocalTarget) -> Option<String> {
    let token = match target {
        LocalTarget::Revision(id) if is_object_id(id) => id.clone(),
        LocalTarget::ParentOf(id) if is_object_id(id) => {
            if vcs == VcsType::Jujutsu {
                // jj diffs merge commits against merged parents, not Git's first parent.
                let output = read_command(
                    git(root).args(["rev-list", "--parents", "-n1", id]),
                    512,
                    false,
                )?;
                if std::str::from_utf8(&output)
                    .ok()?
                    .split_ascii_whitespace()
                    .count()
                    != 2
                {
                    return None;
                }
            }
            format!("{id}^1")
        }
        LocalTarget::RecordedHead(id) if is_object_id(id) => {
            let head = output_id(git(root).args(["rev-parse", "--verify", "HEAD^{commit}"]))?;
            return head.starts_with(id).then_some(head);
        }
        LocalTarget::JjRecordedParent(change) => return jj_parent(root, change),
        _ => return None,
    };
    output_id(git(root).args([
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{token}^{{commit}}"),
    ]))
}

fn jj_parent(root: &Path, change: &str) -> Option<String> {
    let bytes = read_command(
        jj(root).args([
            "log",
            "-r",
            "@",
            "--no-graph",
            "-T",
            r#"change_id.short() ++ "\n" ++ parents.map(|p| p.commit_id()).join("\n")"#,
        ]),
        512,
        false,
    )?;
    let (working_change, parent) = std::str::from_utf8(&bytes).ok()?.split_once('\n')?;
    (working_change == change && is_full_object_id(parent) && has_commit(root, parent))
        .then(|| parent.to_owned())
}

pub(super) fn hg_revision(root: &Path, target: &LocalTarget) -> Option<String> {
    let token = match target {
        LocalTarget::Revision(id) if is_object_id(id) => id.clone(),
        LocalTarget::ParentOf(id) if is_object_id(id) => format!("p1({id})"),
        LocalTarget::RecordedHead(id) if is_object_id(id) => {
            let head = output_id(hg(root).args(["log", "-r", ".", "--template", "{node}"]))?;
            return head.starts_with(id).then_some(head);
        }
        _ => return None,
    };
    output_id(hg(root).args(["log", "-r", &token, "--template", "{node}"]))
}

/// Match only explicit local clone URLs, never the current worktree or a Git
/// checkout merely containing a matching object ID. Returns the canonical
/// root and the name of the first remote whose URL matched.
pub(super) fn matching_git_checkout(
    root: &Path,
    repository: &ForgeRepository,
) -> Option<(PathBuf, String)> {
    let root = root.canonicalize().ok()?;
    if !is_git_checkout(&root) {
        return None;
    }
    let output = read_command(
        git(&root).args([
            "config",
            "--local",
            "--no-includes",
            "--null",
            "--get-regexp",
            r"^remote\..*\.url$",
        ]),
        32 * 1024,
        false,
    )?;
    let remote = std::str::from_utf8(&output)
        .ok()?
        .split_terminator('\0')
        .filter_map(|entry| entry.split_once('\n'))
        .find(|(_, url)| checkout_url_matches(url, repository))
        .and_then(|(key, _)| key.strip_prefix("remote.")?.strip_suffix(".url"))?
        .to_owned();
    Some((root, remote))
}

fn checkout_url_matches(url: &str, repository: &ForgeRepository) -> bool {
    use crate::forge::remote_url::{
        parse_scp_like_remote, strip_port, strip_scheme, trim_url_suffix,
    };
    match repository.kind {
        ForgeKind::AzureDevOps => {
            return crate::forge::azure::az::parse_azure_remote_url(url).as_ref()
                == Some(repository);
        }
        ForgeKind::Gerrit => {
            return crate::forge::gerrit::api::parse_gerrit_remote_url(url).as_ref()
                == Some(repository);
        }
        _ => {}
    }
    let url = trim_url_suffix(url.trim());
    let Some((host, path)) = parse_scp_like_remote(url).or_else(|| {
        let (authority, path) = strip_scheme(url)?.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        Some((strip_port(host), path))
    }) else {
        return false;
    };
    let host = match repository.kind {
        ForgeKind::GitHub if host.eq_ignore_ascii_case("ssh.github.com") => "github.com",
        ForgeKind::Bitbucket
            if [
                "ssh.bitbucket.org",
                "altssh.bitbucket.org",
                "api.bitbucket.org",
            ]
            .iter()
            .any(|alias| host.eq_ignore_ascii_case(alias)) =>
        {
            "bitbucket.org"
        }
        _ => host,
    };
    let path = path.strip_suffix(".git").unwrap_or(path);
    host.eq_ignore_ascii_case(&repository.host)
        && path
            .rsplit_once('/')
            .is_some_and(|(owner, name)| owner == repository.owner && name == repository.name)
}

/// The Git common directory (shared by linked worktrees), canonicalized.
pub(super) fn git_common_dir(root: &Path) -> Option<PathBuf> {
    let raw = read_command(
        git(root).args(["rev-parse", "--git-common-dir"]),
        4096,
        false,
    )?;
    let text = std::str::from_utf8(&raw)
        .ok()?
        .trim_end_matches(['\r', '\n']);
    root.join(text).canonicalize().ok()
}

/// The commit `HEAD` of a checkout resolves to, if its objects are readable.
pub(super) fn head_commit(dir: &Path) -> Option<String> {
    output_id(git(dir).args(["rev-parse", "--verify", "HEAD^{commit}"]))
}
