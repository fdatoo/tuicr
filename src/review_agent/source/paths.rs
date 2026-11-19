use std::fs::{self, File};
use std::io;
use std::path::{Component, Path};

fn windows_special_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or_default();
    ["CON", "PRN", "AUX", "NUL"]
        .iter()
        .any(|name| stem.eq_ignore_ascii_case(name))
        || (stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem
                .get(..3)
                .is_some_and(|s| s.eq_ignore_ascii_case("COM") || s.eq_ignore_ascii_case("LPT")))
}

/// Local-only reads never follow VCS metadata directories or unsafe names.
pub(super) fn local_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|component| {
            let Component::Normal(part) = component else {
                return false;
            };
            part.to_str().is_some_and(|part| {
                !part.contains([':', '\\'])
                    && !windows_special_name(part)
                    && ![".git", ".hg", ".jj"]
                        .iter()
                        .any(|name| part.eq_ignore_ascii_case(name))
            })
        })
}

pub(super) fn open_local_regular(root: &Path, rel: &Path) -> io::Result<File> {
    #[cfg(unix)]
    let file = {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        let mut parent = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(root)?;
        let mut parts = rel.iter().peekable();
        while let Some(part) = parts.next() {
            let name = CString::new(part.as_bytes())?;
            let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
            if parts.peek().is_some() {
                flags |= libc::O_DIRECTORY;
            }
            // SAFETY: descriptor refers to an open directory and name lives through the call.
            let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: openat returned a fresh descriptor; File now owns it.
            parent = unsafe { File::from_raw_fd(fd) };
        }
        parent
    };
    #[cfg(not(unix))]
    let file = {
        let mut path = root.to_path_buf();
        for part in rel {
            path.push(part);
            if fs::symlink_metadata(&path)?.file_type().is_symlink() {
                return Err(io::Error::other("links are not reviewed source"));
            }
        }
        if !fs::symlink_metadata(&path)?.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        File::open(path)?
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    Ok(file)
}
