use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::forge::traits::{ForgeKind, ForgeRepository};
use crate::vcs::traits::VcsType;

use super::{
    Build, LocalTarget, MAX_CATALOG_BYTES, MAX_FILE_BYTES, UNAVAILABLE, UNSAFE_PATH,
    is_full_object_id, is_object_id, paths,
};

const LOCAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_ARCHIVE_BYTES: u64 = 96 * 1024 * 1024;
const GIT_ARCHIVE_FAILURE: &str = "The pinned Git archive could not be read within the archive size or time limit; use the supplied diff.";
const REMOTE_ARCHIVE_FAILURE: &str = "The pinned GitHub archive was unavailable or exceeded its download or extraction limits; use the supplied diff.";
const HG_ARCHIVE_FAILURE: &str = "The pinned Mercurial archive was unavailable or exceeded the archive size or time limit; use the supplied diff.";
const UNREADABLE_FILES: &str = "Missing, unreadable, linked or non-regular files were omitted.";

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

fn git(root: &Path) -> Command {
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

fn output_id(command: &mut Command) -> Option<String> {
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

pub(super) fn worktree(
    root: &Path,
    single: Option<&Path>,
    vcs: VcsType,
    anchors: Option<&[PathBuf]>,
    build: &mut Build<'_>,
) -> Result<(), &'static str> {
    let dirs = directories::ProjectDirs::from("", "", "tuicr");
    let private_roots: Vec<_> = dirs
        .as_ref()
        .into_iter()
        .flat_map(|dirs| [dirs.data_dir(), dirs.config_dir(), dirs.cache_dir()])
        .filter_map(|dir| dir.canonicalize().ok())
        .filter(|dir| dir.starts_with(root))
        .collect();
    let copy = |build: &mut Build<'_>, rel: &Path| -> Result<(), &'static str> {
        if !paths::safe_relative(rel)
            || (!private_roots.is_empty()
                && private_roots.iter().any(|p| root.join(rel).starts_with(p)))
        {
            build.limit(UNSAFE_PATH);
            return Ok(());
        }
        let mut parent = root.to_path_buf();
        if let Some(relative_parent) = rel.parent() {
            for component in relative_parent.components() {
                parent.push(component);
                if !fs::symlink_metadata(&parent).is_ok_and(|metadata| metadata.is_dir()) {
                    build.limit(UNREADABLE_FILES);
                    return Ok(());
                }
                // An embedded checkout/submodule is a different review scope.
                if [".git", ".hg", ".jj"]
                    .iter()
                    .any(|name| fs::symlink_metadata(parent.join(name)).is_ok())
                {
                    build.limit("Nested working copies were omitted.");
                    return Ok(());
                }
            }
        }
        if !build.within_budget() {
            return Ok(());
        }
        let file = match paths::open_regular(root, rel) {
            Ok(file) => file,
            Err(_) => {
                build.limit(UNREADABLE_FILES);
                return Ok(());
            }
        };
        let size = file.metadata().map_err(|_| UNREADABLE_FILES)?.len();
        build
            .write(rel, size, file)
            .map_err(|_| "Could not copy working-tree files to the snapshot")
    };
    if let Some(single) = single {
        return copy(build, single);
    }
    if let Some(anchors) = anchors {
        for rel in anchors {
            if !build.within_budget() {
                break;
            }
            copy(build, rel)?;
        }
        build.limit("Git uses an alternate index. Only exact changed paths from the working tree were copied.");
        return Ok(());
    }
    let raw = match vcs {
        VcsType::Git | VcsType::Jujutsu => read_command(
            git(root).args([
                "ls-files",
                "-z",
                "--deduplicate",
                "--cached",
                "--others",
                "--exclude-standard",
            ]),
            MAX_CATALOG_BYTES,
            true,
        ),
        VcsType::Mercurial => read_command(
            hg(root).args(["status", "-n0", "-mauc"]),
            MAX_CATALOG_BYTES,
            true,
        ),
        VcsType::File => None,
    }
    .ok_or(UNAVAILABLE)?;
    let rules = worktree_ignore(root)?;
    for record in records(&raw) {
        if !build.within_budget() {
            break;
        }
        let Ok(path) = std::str::from_utf8(record) else {
            build.limit(UNSAFE_PATH);
            continue;
        };
        let rel = Path::new(path);
        if paths::safe_relative(rel) && rules.matched_path_or_any_parents(rel, false).is_ignore() {
            build.limit("Paths excluded by the review's ignore rules were omitted.");
            continue;
        }
        copy(build, rel)?;
    }
    if raw.len() >= MAX_CATALOG_BYTES {
        build.limit("The working-tree path catalog was truncated.");
    }
    Ok(())
}

fn worktree_ignore(root: &Path) -> Result<ignore::gitignore::Gitignore, &'static str> {
    const UNREADABLE_RULES: &str =
        "The working tree uses unreadable or oversized .tuicrignore rules; use the supplied diff.";
    const MAX_RULES_BYTES: u64 = 1024 * 1024;
    let path = Path::new(".tuicrignore");
    let mut rules = ignore::gitignore::GitignoreBuilder::new(root);
    if !matches!(fs::symlink_metadata(root.join(path)), Err(error) if error.kind() == io::ErrorKind::NotFound)
    {
        let mut content = String::new();
        paths::open_regular(root, path)
            .map_err(|_| UNREADABLE_RULES)?
            .take(MAX_RULES_BYTES + 1)
            .read_to_string(&mut content)
            .map_err(|_| UNREADABLE_RULES)?;
        if content.len() as u64 > MAX_RULES_BYTES {
            return Err(UNREADABLE_RULES);
        }
        for line in content.lines() {
            let _ = rules.add_line(None, line);
        }
    }
    rules.build().map_err(|_| UNREADABLE_RULES)
}

fn records(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes
        .split_inclusive(|&b| b == 0)
        .filter_map(|record| record.strip_suffix(&[0]))
        .filter(|record| !record.is_empty())
}

pub(super) fn git_index(root: &Path, build: &mut Build<'_>) -> Result<(), &'static str> {
    let raw = read_command(
        git(root).args(["ls-files", "--stage", "-z"]),
        MAX_CATALOG_BYTES,
        true,
    )
    .ok_or(UNAVAILABLE)?;
    let objects = GitObjects::new(root);
    for record in records(&raw) {
        if !build.within_budget() {
            break;
        }
        let Ok(record) = std::str::from_utf8(record) else {
            build.limit(UNSAFE_PATH);
            continue;
        };
        let (metadata, path) = record.split_once('\t').ok_or(UNAVAILABLE)?;
        let mut fields = metadata.split(' ');
        let (Some(mode), Some(sha), Some(stage)) = (fields.next(), fields.next(), fields.next())
        else {
            return Err(UNAVAILABLE);
        };
        if stage != "0" {
            build.limit("Unmerged paths in the Git index were omitted.");
            continue;
        }
        if !matches!(mode, "100644" | "100755") {
            build.limit(UNREADABLE_FILES);
            continue;
        }
        objects.copy(Path::new(path), sha, build)?;
    }
    if raw.len() >= MAX_CATALOG_BYTES {
        build.limit("The Git index path catalog was truncated.");
    }
    Ok(())
}

struct GitObjects<'a> {
    root: &'a Path,
    repo: Option<git2::Repository>,
}

impl<'a> GitObjects<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            repo: git2::Repository::open(root).ok(),
        }
    }

    fn copy(&self, path: &Path, sha: &str, build: &mut Build<'_>) -> Result<(), &'static str> {
        if !is_full_object_id(sha) || !paths::safe_relative(path) {
            build.limit(UNSAFE_PATH);
            return Ok(());
        }
        let object = self.repo.as_ref().zip(git2::Oid::from_str(sha).ok());
        let size = object
            .and_then(|(repo, oid)| {
                repo.odb()
                    .ok()?
                    .read_header(oid)
                    .ok()
                    .map(|(size, _)| size as u64)
            })
            .or_else(|| {
                let bytes = read_command(git(self.root).args(["cat-file", "-s", sha]), 64, false)?;
                std::str::from_utf8(&bytes).ok()?.trim().parse().ok()
            });
        let Some(size) = size else {
            build.limit(UNREADABLE_FILES);
            return Ok(());
        };
        if size > MAX_FILE_BYTES || size > super::MAX_TREE_BYTES - build.bytes {
            return build
                .write(path, size, io::empty())
                .map_err(|_| UNAVAILABLE);
        }
        if let Some((repo, oid)) = object
            && let Ok(blob) = repo.find_blob(oid)
        {
            return build
                .write(path, size, blob.content())
                .map_err(|_| UNAVAILABLE);
        }
        let Some(bytes) = read_command(
            git(self.root).args(["cat-file", "blob", sha]),
            size as usize,
            false,
        ) else {
            build.limit(UNREADABLE_FILES);
            return Ok(());
        };
        build
            .write(path, size, bytes.as_slice())
            .map_err(|_| UNAVAILABLE)
    }
}

pub(super) fn git_archive(
    root: &Path,
    sha: &str,
    single: Option<&Path>,
    build: &mut Build<'_>,
) -> Result<(), &'static str> {
    // A tree object, rather than a commit, also prevents export-subst from
    // replacing source bytes with an expanded commit message or hash.
    let tree = output_id(git(root).args([
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{sha}^{{tree}}"),
    ]))
    .ok_or(GIT_ARCHIVE_FAILURE)?;
    let archive = capture_archive(
        git(root).args(["archive", "--format=tar", &tree]),
        build.root,
    )
    .ok_or(GIT_ARCHIVE_FAILURE)?;
    let archived =
        expand_archive(archive, false, single, build).map_err(|_| GIT_ARCHIVE_FAILURE)?;
    if !build.within_budget() {
        return Ok(());
    }
    // Git archive honors export-ignore. Compare with the pinned tree catalog
    // and recover those regular files from their exact blobs, never the worktree.
    let raw = read_command(
        git(root).args(["ls-tree", "-r", "-z", &tree]),
        MAX_CATALOG_BYTES,
        true,
    )
    .ok_or(GIT_ARCHIVE_FAILURE)?;
    let objects = GitObjects::new(root);
    for record in records(&raw) {
        if !build.within_budget() {
            break;
        }
        let Ok(record) = std::str::from_utf8(record) else {
            build.limit(UNSAFE_PATH);
            continue;
        };
        let (metadata, path) = record.split_once('\t').ok_or(GIT_ARCHIVE_FAILURE)?;
        let rel = Path::new(path);
        if archived.contains(rel) || single.is_some_and(|single| rel != single) {
            continue;
        }
        let Some((type_and_mode, oid)) = metadata.rsplit_once(' ') else {
            return Err(GIT_ARCHIVE_FAILURE);
        };
        if matches!(type_and_mode, "100644 blob" | "100755 blob") {
            objects.copy(rel, oid, build)?;
        } else {
            build.limit(UNREADABLE_FILES);
        }
    }
    if raw.len() >= MAX_CATALOG_BYTES {
        build.limit("The Git tree path catalog was truncated.");
    }
    Ok(())
}

pub(super) fn hg_archive(
    root: &Path,
    sha: &str,
    single: Option<&Path>,
    build: &mut Build<'_>,
) -> Result<(), &'static str> {
    let archive = capture_archive(
        hg(root).args([
            "--config",
            "ui.archivemeta=false",
            "archive",
            "-t",
            "tar",
            "-p",
            "snapshot",
            "-r",
            sha,
            "-",
        ]),
        build.root,
    )
    .ok_or(HG_ARCHIVE_FAILURE)?;
    expand_archive(archive, true, single, build).map_err(|_| HG_ARCHIVE_FAILURE)?;
    Ok(())
}

pub(super) fn github_archive(
    repository: &ForgeRepository,
    sha: &str,
    build: &mut Build<'_>,
) -> Result<(), &'static str> {
    fn slug_component(value: &str) -> bool {
        !matches!(value, "" | "." | "..")
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    }
    if !slug_component(&repository.owner)
        || !slug_component(&repository.name)
        || !slug_component(&repository.host)
        || !is_full_object_id(sha)
    {
        return Err(REMOTE_ARCHIVE_FAILURE);
    }
    let endpoint = format!(
        "repos/{}/{}/tarball/{sha}",
        repository.owner, repository.name
    );
    let mut command = Command::new("gh");
    command
        .current_dir(build.root)
        .env("GH_PROMPT_DISABLED", "1")
        .args(["api", "--hostname", &repository.host, &endpoint]);
    let file = capture_archive(&mut command, build.root).ok_or(REMOTE_ARCHIVE_FAILURE)?;
    extract_github_archive(file, build)
}

pub(super) fn extract_github_archive(
    file: impl Read,
    build: &mut Build<'_>,
) -> Result<(), &'static str> {
    let decoder = flate2::read::GzDecoder::new(file);
    expand_archive(decoder.take(MAX_ARCHIVE_BYTES), true, None, build)
        .map_err(|_| REMOTE_ARCHIVE_FAILURE)?;
    // GitHub generates archives with Git's export attributes, unlike the local
    // object path which verifies and repairs the output against the tree.
    build.limit(
        "GitHub archive contents may reflect repository export-ignore or export-subst attributes.",
    );
    Ok(())
}

fn expand_archive(
    input: impl Read,
    strip_prefix: bool,
    single: Option<&Path>,
    build: &mut Build<'_>,
) -> io::Result<HashSet<PathBuf>> {
    let mut archive = tar::Archive::new(input);
    let mut paths = HashSet::new();
    let mut prefix = None;
    for (i, item) in archive.entries()?.enumerate() {
        if !build.within_budget() {
            break;
        }
        if i >= super::MAX_FILES * 3 {
            build.limit("The archive entry catalog was truncated.");
            break;
        }
        let mut item = item?;
        let bytes = item.path_bytes();
        let Ok(raw) = std::str::from_utf8(&bytes) else {
            build.limit(UNSAFE_PATH);
            continue;
        };
        let path = Path::new(raw);
        let rel = if strip_prefix {
            let mut components = path.components();
            let Some(Component::Normal(top)) = components.next() else {
                build.limit(UNSAFE_PATH);
                continue;
            };
            if prefix.get_or_insert_with(|| top.to_owned()) != top {
                return Err(io::Error::other("archive contains multiple roots"));
            }
            components.as_path()
        } else {
            path
        };
        if item.header().entry_type().is_dir() || single.is_some_and(|single| single != rel) {
            continue;
        }
        if !item.header().entry_type().is_file() {
            build.limit(UNREADABLE_FILES);
            continue;
        }
        let rel = rel.to_path_buf();
        if !paths::safe_relative(&rel) {
            build.limit(UNSAFE_PATH);
            continue;
        }
        let size = item.size();
        build.write(&rel, size, &mut item)?;
        paths.insert(rel);
    }
    Ok(paths)
}

fn capture_archive(command: &mut Command, private_dir: &Path) -> Option<File> {
    let mut spool = tempfile::tempfile_in(private_dir).ok()?;
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(command, 0);
    let deadline = Instant::now() + ARCHIVE_TIMEOUT;
    let mut child = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let result = (|| {
        let stdout = child.stdout.take()?;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .spawn(move || {
                let mut stream = stdout.take(MAX_ARCHIVE_BYTES + 1);
                let result = io::copy(&mut stream, &mut spool).and_then(|count| {
                    if count > MAX_ARCHIVE_BYTES {
                        return Err(io::Error::other("archive too large"));
                    }
                    spool.rewind()?;
                    Ok(spool)
                });
                let _ = tx.send(result);
            })
            .ok()?;
        let spool = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()?
            .ok()?;
        loop {
            if let Some(status) = child.try_wait().ok()? {
                return Some((status, spool));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10).min(remaining));
        }
    })();
    match result {
        Some((status, spool)) => status.success().then_some(spool),
        None => {
            #[cfg(unix)]
            // SAFETY: the unreaped child is the leader of its own process group.
            unsafe {
                libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

/// Match only explicit local clone URLs, never the current worktree or a Git
/// checkout merely containing a matching object ID.
pub(super) fn matching_git_checkout(root: &Path, repository: &ForgeRepository) -> Option<PathBuf> {
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
    let matched = std::str::from_utf8(&output)
        .ok()?
        .split_terminator('\0')
        .filter_map(|entry| entry.split_once('\n').map(|(_, url)| url))
        .any(|url| checkout_url_matches(url, repository));
    matched.then_some(root)
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
