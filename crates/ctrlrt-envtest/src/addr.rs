//! Port of `controller-runtime/pkg/internal/testing/addr` (free port suggestion).
//!
//! Like Go, every suggested port is reserved through an exclusive `flock` on
//! `<user cache dir>/kubebuilder-envtest/port-<N>` (shared with Go envtest runs on the same
//! machine). The lock file handles are kept open for the life of the process (Go leaks the fd
//! on purpose too), so neither this process nor any other envtest process hands the same port
//! out twice; stale reservation files older than two minutes are garbage-collected.

use std::fs::{self, File, OpenOptions};
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

const PORT_RESERVE_TIME: Duration = Duration::from_secs(2 * 60);
const PORT_CONFLICT_RETRY: usize = 100;
const PORT_FILE_PREFIX: &str = "port-";

static CACHE_DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    let user_cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(|h| PathBuf::from(h).join(".cache"))
        });
    if let Some(base) = user_cache {
        let dir = base.join("kubebuilder-envtest");
        if fs::create_dir_all(&dir).is_ok() {
            return Some(dir);
        }
    }
    let dir = std::env::temp_dir().join("kubebuilder-envtest");
    fs::create_dir_all(&dir).ok().map(|_| dir)
});

/// Lock handles held for the whole process lifetime.
static HELD_LOCKS: Mutex<Vec<File>> = Mutex::new(Vec::new());

fn cache_add(port: u16) -> anyhow::Result<bool> {
    let Some(dir) = CACHE_DIR.as_ref() else {
        anyhow::bail!("unable to create the kubebuilder-envtest port cache directory");
    };
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(ft) = entry.file_type() else { continue };
        if !ft.is_file() || !name.starts_with(PORT_FILE_PREFIX) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if let Ok(modified) = meta.modified()
            && SystemTime::now()
                .duration_since(modified)
                .unwrap_or_default()
                > PORT_RESERVE_TIME
        {
            match fs::remove_file(entry.path()) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    let path = dir.join(format!("{PORT_FILE_PREFIX}{port}"));
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    match file.try_lock() {
        Ok(()) => {
            if let Ok(mut held) = HELD_LOCKS.lock() {
                held.push(file);
            }
            Ok(true)
        }
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// `addr.Suggest("")` — returns a free, reserved port and the host it was probed on.
pub fn suggest() -> anyhow::Result<(u16, String)> {
    // Keep the probing listeners open until we return (Go defers their Close), so the
    // kernel does not hand out the same ephemeral port again during the retries.
    let mut listeners = Vec::new();
    for _ in 0..PORT_CONFLICT_RETRY {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        listeners.push(listener);
        if cache_add(port)? {
            return Ok((port, Ipv4Addr::LOCALHOST.to_string()));
        }
    }
    anyhow::bail!("no free ports found after {} retries", PORT_CONFLICT_RETRY)
}
