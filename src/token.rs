//! The bearer token the local API is guarded by.
//!
//! Every endpoint needs it, reads included: a task list carries the prompts
//! you run, which is not less sensitive than firing them.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const TOKEN_FILE: &str = "token";
/// 32 bytes of entropy, hex-encoded.
const BYTES: usize = 32;

pub fn path_in(state_dir: &Path) -> PathBuf {
    state_dir.join(TOKEN_FILE)
}

/// Reads the token, creating one the first time.
pub fn load_or_create(state_dir: &Path) -> Result<String> {
    match read(state_dir)? {
        Some(token) => Ok(token),
        None => create(state_dir),
    }
}

pub fn read(state_dir: &Path) -> Result<Option<String>> {
    let path = path_in(state_dir);
    match std::fs::read_to_string(&path) {
        Ok(token) => Ok(Some(token.trim().to_string()).filter(|token| !token.is_empty())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// Writes a fresh token, replacing any existing one.
pub fn create(state_dir: &Path) -> Result<String> {
    std::fs::create_dir_all(state_dir)
        .with_context(|| format!("creating {}", state_dir.display()))?;

    let mut bytes = [0u8; BYTES];
    std::io::Read::read_exact(
        &mut std::fs::File::open("/dev/urandom").context("opening /dev/urandom")?,
        &mut bytes,
    )
    .context("reading random bytes")?;
    let token: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();

    let path = path_in(state_dir);
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, format!("{token}\n"))
        .with_context(|| format!("writing {}", temporary.display()))?;
    // Readable only by its owner, before it takes its final name.
    set_owner_only(&temporary)?;
    std::fs::rename(&temporary, &path).with_context(|| format!("replacing {}", path.display()))?;

    Ok(token)
}

fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {}", path.display()))
}

/// Compares in constant time, so a wrong token tells the caller nothing
/// about how much of it was right.
pub fn matches(expected: &str, offered: &str) -> bool {
    if expected.len() != offered.len() {
        return false;
    }
    expected
        .bytes()
        .zip(offered.bytes())
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}
