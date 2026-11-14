//! Unix socket setup, peer framing, and the daemon singleton lock.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const MAX_FRAME_SIZE: usize = 64 * 1024 * 1024;

pub(super) fn existing_socket_path() -> Option<PathBuf> {
    let path = daemon_dir().ok()?.join("daemon.sock");
    let metadata = fs::symlink_metadata(&path).ok()?;
    metadata.file_type().is_socket().then_some(path)
}

pub(super) fn daemon_socket_path() -> Result<PathBuf> {
    let directory = daemon_dir()?;
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create daemon directory {}", directory.display()))?;
    let metadata = fs::symlink_metadata(&directory)
        .with_context(|| format!("failed to inspect daemon directory {}", directory.display()))?;
    // SAFETY: getuid has no preconditions and only reads the current process uid.
    let current_uid = unsafe { libc::getuid() };
    if !metadata.is_dir() || metadata.uid() != current_uid {
        bail!(
            "daemon directory {} must be a directory owned by the current user",
            directory.display()
        );
    }
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to secure daemon directory {}", directory.display()))?;

    let path = directory.join("daemon.sock");
    if path.as_os_str().as_encoded_bytes().len() >= 100 {
        bail!(
            "daemon socket path is too long: {}. Set XDG_RUNTIME_DIR to a shorter private directory.",
            path.display()
        );
    }
    Ok(path)
}

pub(super) fn acquire_daemon_lock(socket_path: &Path) -> Result<File> {
    let lock_path = socket_path.with_extension("lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .with_context(|| format!("failed to open daemon lock {}", lock_path.display()))?;
    // SAFETY: `lock` owns a valid file descriptor for the lock file and the
    // nonblocking exclusive lock is released automatically when it is dropped.
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == -1 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            bail!("Vera daemon is already running");
        }
        return Err(error).context("failed to acquire daemon lock");
    }
    Ok(lock)
}

pub(super) fn remove_stale_socket(socket_path: &Path) -> Result<()> {
    match fs::symlink_metadata(socket_path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(socket_path).with_context(|| {
                format!(
                    "failed to remove stale daemon socket {}",
                    socket_path.display()
                )
            })?;
        }
        Ok(_) => bail!(
            "refusing to replace non-socket daemon path {}",
            socket_path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("failed to inspect daemon socket {}", socket_path.display())
            });
        }
    }
    Ok(())
}

pub(super) struct SocketGuard(pub(super) PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_file(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.0.display(), %error, "failed to remove daemon socket");
        }
    }
}

pub(super) fn write_frame(stream: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    let payload = serde_json::to_vec(value).context("failed to encode daemon request")?;
    if payload.len() > MAX_FRAME_SIZE {
        bail!("daemon request exceeds the {MAX_FRAME_SIZE}-byte limit");
    }
    let length = u32::try_from(payload.len()).context("daemon request is too large")?;
    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}

pub(super) fn read_frame<T: for<'de> Deserialize<'de>>(stream: &mut UnixStream) -> Result<T> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME_SIZE {
        bail!("daemon response exceeds the {MAX_FRAME_SIZE}-byte limit");
    }
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload)?;
    serde_json::from_slice(&payload).context("failed to decode daemon response")
}

pub(super) async fn read_async_frame<T: for<'de> Deserialize<'de>>(
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<T> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME_SIZE {
        bail!("daemon request exceeds the {MAX_FRAME_SIZE}-byte limit");
    }
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload).context("failed to decode daemon request")
}

pub(super) async fn write_async_frame(
    stream: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
) -> Result<()> {
    let payload = serde_json::to_vec(value).context("failed to encode daemon response")?;
    if payload.len() > MAX_FRAME_SIZE {
        bail!("daemon response exceeds the {MAX_FRAME_SIZE}-byte limit");
    }
    let length = u32::try_from(payload.len()).context("daemon response is too large")?;
    stream.write_all(&length.to_be_bytes()).await?;
    stream.write_all(&payload).await?;
    stream.flush().await?;
    Ok(())
}

fn daemon_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::cache_dir)
        .context("could not determine a local runtime directory")?;
    Ok(base.join("vera"))
}
