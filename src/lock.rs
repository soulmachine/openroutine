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
