//! One Daemon at a time.
//!
//! Two schedulers over one state directory would race over the same file and
//! fire the same Tasks twice, so the second one must not start at all.

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const LOCK_FILE: &str = "daemon.lock";

/// Held for as long as the Daemon runs. Dropping it — or the process exiting
/// for any reason, including a kill — releases the lock to the kernel.
pub struct DaemonLock {
    _file: std::fs::File,
    path: PathBuf,
}

impl DaemonLock {
    pub fn acquire(state_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("creating state dir {}", state_dir.display()))?;
        let path = state_dir.join(LOCK_FILE);

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;

        // Safety: `flock` on a descriptor we own. LOCK_NB means we are told
        // rather than made to wait.
        let taken = unsafe {
            libc::flock(
                std::os::unix::io::AsRawFd::as_raw_fd(&file),
                libc::LOCK_EX | libc::LOCK_NB,
            )
        };
        if taken != 0 {
            let holder = std::fs::read_to_string(&path).unwrap_or_default();
            let holder = holder.trim();
            anyhow::bail!(
                "another openroutine is already running{}; its lock is {}",
                if holder.is_empty() {
                    String::new()
                } else {
                    format!(" (pid {holder})")
                },
                path.display()
            );
        }

        // Leave the pid behind so the next person to try knows who to look for.
        file.set_len(0).ok();
        write!(file, "{}", std::process::id()).ok();
        file.flush().ok();

        Ok(Self { _file: file, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Whether a Daemon is holding the lock right now.
///
/// Asks without taking it: a shared lock cannot be had while someone holds
/// the exclusive one, and the probe leaves the file untouched either way.
pub fn is_held(state_dir: &Path) -> bool {
    let path = state_dir.join(LOCK_FILE);
    let Ok(file) = std::fs::File::open(&path) else {
        return false;
    };
    let fd = std::os::unix::io::AsRawFd::as_raw_fd(&file);
    // Safety: `flock` on a descriptor we own, non-blocking.
    let shared = unsafe { libc::flock(fd, libc::LOCK_SH | libc::LOCK_NB) };
    if shared == 0 {
        unsafe { libc::flock(fd, libc::LOCK_UN) };
        return false;
    }
    std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock
}

/// The pid recorded by whoever holds the lock.
pub fn holder(state_dir: &Path) -> Option<String> {
    let recorded = std::fs::read_to_string(state_dir.join(LOCK_FILE)).ok()?;
    let recorded = recorded.trim().to_string();
    (!recorded.is_empty()).then_some(recorded)
}
