use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{Snapshot, paths};

const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_LIMITS_BYTES: u64 = 4096;

pub(super) struct Cache {
    base: PathBuf,
}

impl Cache {
    pub(super) fn open(base: PathBuf) -> io::Result<Self> {
        // The OS or XDG cache path can include pre-existing aliases (e.g.
        // macOS /var). Resolve that parent, not the snapshot cache leaf.
        let parent = base
            .parent()
            .ok_or_else(|| io::Error::other("invalid cache path"))?;
        private_dir_builder().recursive(true).create(parent)?;
        let leaf = base
            .file_name()
            .ok_or_else(|| io::Error::other("invalid cache path"))?;
        let base = parent.canonicalize()?.join(leaf);
        create_private(&base)?;
        let cache = Self { base };
        cache.prune()?;
        Ok(cache)
    }

    pub(super) fn stage(&self) -> io::Result<Stage> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("build-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o700));
        }
        let temp = builder.tempdir_in(&self.base)?;
        let tree = temp.path().join("tree");
        create_private(&tree)?;
        Ok(Stage { temp, tree })
    }

    fn lock(&self) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let path = self.base.join(".lock");
        if fs::symlink_metadata(&path).is_ok_and(|metadata| !metadata.is_file()) {
            return Err(io::Error::other("unsafe snapshot cache lock"));
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || !owner_only(&metadata) {
            return Err(io::Error::other("unsafe snapshot cache lock"));
        }
        file.lock()?;
        Ok(file)
    }

    fn prune(&self) -> io::Result<()> {
        let _lock = self.lock()?;
        let now = SystemTime::now();
        for entry in fs::read_dir(&self.base)? {
            let entry = entry?;
            let Some(name) = entry.file_name().into_string().ok() else {
                continue;
            };
            if !is_key(&name) || !entry.file_type()?.is_dir() {
                continue;
            }
            let metadata = entry.metadata()?;
            if owner_only(&metadata)
                && metadata
                    .modified()
                    .is_ok_and(|mtime| now.duration_since(mtime).is_ok_and(|age| age > RETENTION))
            {
                // An active provider can keep an older snapshot pinned. Nobody
                // can obtain a new pin while the cache lock is held.
                let path = entry.path();
                if let Ok(lease) = open_lease(&path)
                    && lease.try_lock().is_ok()
                {
                    let _ = make_removable(&path).and_then(|()| fs::remove_dir_all(&path));
                }
            }
        }
        Ok(())
    }

    pub(super) fn get(&self, key: &str) -> io::Result<Option<Snapshot>> {
        let _lock = self.lock()?;
        self.get_locked(key)
    }

    fn get_locked(&self, key: &str) -> io::Result<Option<Snapshot>> {
        let dir = self.base.join(key);
        let metadata = match fs::symlink_metadata(&dir) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            result => result?,
        };
        if !metadata.is_dir() || !owner_only(&metadata) {
            return Err(io::Error::other("unsafe cached review snapshot"));
        }
        let tree = dir.join("tree");
        assert_read_only_tree(&tree)?;
        let lease = open_lease(&dir)?;
        lease.lock_shared()?;
        let mut limits = String::new();
        private_file(&dir.join("limits"))?
            .take(MAX_LIMITS_BYTES + 1)
            .read_to_string(&mut limits)?;
        if limits.len() as u64 > MAX_LIMITS_BYTES {
            return Err(io::Error::other("invalid snapshot limits"));
        }
        File::open(&dir)?.set_modified(SystemTime::now())?;
        Ok(Some(Snapshot {
            dir: tree,
            limits: (!limits.is_empty()).then_some(limits),
            _lease: lease,
        }))
    }

    pub(super) fn publish(
        &self,
        key: &str,
        mut stage: Stage,
        limits: &str,
    ) -> io::Result<Snapshot> {
        let _lock = self.lock()?;
        if let Some(snapshot) = self.get_locked(key)? {
            return Ok(snapshot);
        }
        write_private(&stage.temp.path().join("limits"), limits.as_bytes())?;
        let lease_path = stage.temp.path().join("lease");
        write_private(&lease_path, b"")?;
        freeze_dirs(&stage.tree)?;
        let path = self.base.join(key);
        if let Err(err) = fs::rename(stage.temp.path(), &path) {
            let _ = make_removable(&stage.tree);
            return Err(err);
        }
        // The old temporary path has been renamed and is now managed by prune.
        stage.tree = PathBuf::new();
        self.get_locked(key)?
            .ok_or_else(|| io::Error::other("published snapshot missing"))
    }
}

pub(super) struct Stage {
    temp: tempfile::TempDir,
    pub(super) tree: PathBuf,
}

impl Drop for Stage {
    fn drop(&mut self) {
        if !self.tree.as_os_str().is_empty() {
            let _ = make_removable(&self.tree);
        }
    }
}

fn is_key(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn private_dir_builder() -> fs::DirBuilder {
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder
}

fn create_private(path: &Path) -> io::Result<()> {
    match private_dir_builder().create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || !owner_only(&metadata) {
        return Err(io::Error::other(
            "snapshot cache is not a private directory owned by the current user",
        ));
    }
    Ok(())
}

fn owner_only(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions.
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
    }
}

fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(
        &mut options,
        libc::O_NOFOLLOW | libc::O_NONBLOCK,
    );
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(io::Error::other("unsafe snapshot metadata"));
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || !owner_only(&metadata) {
        return Err(io::Error::other("unsafe snapshot metadata"));
    }
    Ok(file)
}

fn open_lease(dir: &Path) -> io::Result<File> {
    // Exclusive locks on a shared read/write file are needed for pruning, even
    // on platforms that do not allow an exclusive lock on read-only handles.
    let path = dir.join("lease");
    private_file(&path)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(
        &mut options,
        libc::O_NOFOLLOW | libc::O_NONBLOCK,
    );
    options.open(path)
}

fn write_private(path: &Path, value: &[u8]) -> io::Result<()> {
    use std::io::Write as _;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(value)
}

fn assert_read_only_tree(root: &Path) -> io::Result<()> {
    let top = fs::symlink_metadata(root)?;
    if !top.is_dir() || !owner_only(&top) || !top.permissions().readonly() {
        return Err(io::Error::other(
            "snapshot tree is not private and read-only",
        ));
    }
    for entry in ignore::WalkBuilder::new(root)
        .standard_filters(false)
        .follow_links(false)
        .build()
        .skip(1)
    {
        let entry = entry.map_err(io::Error::other)?;
        let relative = entry.path().strip_prefix(root).map_err(io::Error::other)?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if !paths::safe_relative(relative)
            || !(metadata.is_file() || metadata.is_dir())
            || !owner_only(&metadata)
            || !metadata.permissions().readonly()
        {
            return Err(io::Error::other("unsafe file in review snapshot"));
        }
    }
    Ok(())
}

fn freeze_dirs(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            freeze_dirs(&entry.path())?;
        }
    }
    set_writable(dir, false)
}

pub(super) fn make_removable(dir: &Path) -> io::Result<()> {
    set_writable(dir, true)?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            make_removable(&entry.path())?;
        } else if cfg!(windows) && ty.is_file() {
            set_writable(&entry.path(), true)?;
        }
    }
    Ok(())
}

fn set_writable(path: &Path, writable: bool) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() && !meta.is_file() {
        return Err(io::Error::other(
            "snapshot path is not a regular file or directory",
        ));
    }
    let mut perms = meta.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(if writable {
            (perms.mode() | 0o200) & 0o700
        } else {
            perms.mode() & 0o500
        });
    }
    #[cfg(not(unix))]
    perms.set_readonly(!writable);
    fs::set_permissions(path, perms)
}
