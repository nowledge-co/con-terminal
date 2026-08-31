use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Parser;

const CACHE_FILE: &str = "ssh_cache";
const LOCK_FILE: &str = "ssh_cache.lock";
const TERMINFO_NAME: &str = "xterm-ghostty";
const STALE_LOCK_AFTER: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub host: String,
    pub timestamp: i64,
}

#[derive(Debug, Default)]
pub struct Cache {
    path: PathBuf,
}

impl Cache {
    pub fn user() -> Self {
        Self {
            path: con_paths::app_cache_dir().join(CACHE_FILE),
        }
    }

    #[cfg(test)]
    fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn clear(&self) -> io::Result<()> {
        self.with_lock(|path| match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        })
    }

    pub fn add(&self, host: &str) -> io::Result<bool> {
        validate_host(host)?;
        self.with_lock(|path| {
            let mut entries = read_entries(path)?;
            let now = unix_timestamp();
            let existed = if let Some(entry) = entries.iter_mut().find(|entry| entry.host == host) {
                entry.timestamp = now;
                true
            } else {
                entries.push(CacheEntry {
                    host: host.to_string(),
                    timestamp: now,
                });
                false
            };
            write_entries(path, &entries)?;
            Ok(!existed)
        })
    }

    pub fn remove(&self, host: &str) -> io::Result<()> {
        validate_host(host)?;
        self.with_lock(|path| {
            let mut entries = read_entries(path)?;
            entries.retain(|entry| entry.host != host);
            if entries.is_empty() {
                match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            } else {
                write_entries(path, &entries)?;
            }
            Ok(())
        })
    }

    pub fn contains(&self, host: &str, expire_days: Option<u32>) -> io::Result<bool> {
        validate_host(host)?;
        Ok(read_entries(&self.path)?
            .iter()
            .filter(|entry| !entry.is_expired(expire_days))
            .any(|entry| entry.host == host))
    }

    pub fn list(&self, expire_days: Option<u32>) -> io::Result<Vec<CacheEntry>> {
        let mut entries: Vec<_> = read_entries(&self.path)?
            .into_iter()
            .filter(|entry| !entry.is_expired(expire_days))
            .collect();
        entries.sort_by(|left, right| left.host.cmp(&right.host));
        Ok(entries)
    }

    fn with_lock<T>(&self, operation: impl FnOnce(&Path) -> io::Result<T>) -> io::Result<T> {
        let parent = self.path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "cache path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        let lock_path = parent.join(LOCK_FILE);
        let _lock = CacheLock::acquire(&lock_path)?;
        operation(&self.path)
    }
}

impl CacheEntry {
    fn is_expired(&self, expire_days: Option<u32>) -> bool {
        let Some(expire_days) = expire_days else {
            return false;
        };
        let max_age = i64::from(expire_days) * 24 * 60 * 60;
        unix_timestamp().saturating_sub(self.timestamp) > max_age
    }
}

struct CacheLock {
    path: PathBuf,
}

impl CacheLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        for _ in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(mut file) => {
                    writeln!(file, "pid={}", std::process::id())?;
                    return Ok(Self {
                        path: path.to_path_buf(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(path)
                        .and_then(|metadata| metadata.modified())
                        .ok()
                        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                        .is_some_and(|age| age > STALE_LOCK_AFTER);
                    if stale {
                        let _ = fs::remove_file(path);
                        continue;
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::WouldBlock,
                        "SSH cache is busy; try again",
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "SSH cache is busy; try again",
        ))
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn read_entries(path: &Path) -> io::Result<Vec<CacheEntry>> {
    let mut content = String::new();
    match File::open(path) {
        Ok(mut file) => {
            file.read_to_string(&mut content)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }

    Ok(content.lines().filter_map(parse_entry).collect())
}

fn parse_entry(line: &str) -> Option<CacheEntry> {
    let mut fields = line.trim().split('|');
    let host = fields.next()?;
    let timestamp = fields.next()?.parse().ok()?;
    if fields.next().is_none() || !valid_cache_key(host) {
        return None;
    }
    Some(CacheEntry {
        host: host.to_string(),
        timestamp,
    })
}

fn write_entries(path: &Path, entries: &[CacheEntry]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "cache path has no parent"))?;
    let temp_path = parent.join(format!(".{CACHE_FILE}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)?;
        for entry in entries {
            writeln!(file, "{}|{}|{}", entry.host, entry.timestamp, TERMINFO_NAME)?;
        }
        file.sync_all()?;
        fs::rename(&temp_path, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn validate_host(host: &str) -> io::Result<()> {
    if valid_cache_key(host) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected hostname, IP address, or user@hostname without whitespace or '|'",
        ))
    }
}

fn valid_cache_key(host: &str) -> bool {
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    !host.is_empty()
        && host.len() <= 255
        && !host.chars().any(|character| {
            character.is_whitespace() || character.is_control() || character == '|'
        })
        && host.split('@').count() <= 2
        && host.split('@').all(|part| !part.is_empty())
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub fn run(args: &[std::ffi::OsString]) -> i32 {
    let parsed = match SshCacheArgs::try_parse_from(
        std::iter::once(std::ffi::OsString::from("con-cli +ssh-cache")).chain(args.iter().cloned()),
    ) {
        Ok(args) => args,
        Err(error) => {
            let code = if matches!(error.kind(), clap::error::ErrorKind::DisplayHelp) {
                0
            } else {
                2
            };
            eprint!("{error}");
            return code;
        }
    };
    let cache = Cache::user();
    let result = if parsed.clear {
        cache.clear().map(|_| {
            println!("Cache cleared.");
        })
    } else if let Some(host) = parsed.add {
        cache.add(&host).map(|added| {
            if added {
                println!("Added '{host}' to cache.");
            } else {
                println!("Updated '{host}' cache entry.");
            }
        })
    } else if let Some(host) = parsed.remove {
        cache.remove(&host).map(|_| {
            println!("Removed '{host}' from cache.");
        })
    } else if let Some(host) = parsed.host {
        match cache.contains(&host, parsed.expire_days) {
            Ok(true) => {
                println!("'{host}' has Con terminfo installed.");
                return 0;
            }
            Ok(false) => {
                println!("'{host}' does not have Con terminfo installed.");
                return 1;
            }
            Err(error) => Err(error),
        }
    } else {
        cache.list(parsed.expire_days).map(|entries| {
            if entries.is_empty() {
                println!("No hosts in cache.");
            } else {
                println!("Cached hosts ({}):", entries.len());
                for entry in entries {
                    println!("  {}", entry.host);
                }
            }
        })
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("con-cli +ssh-cache: {error}");
            1
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "con-cli +ssh-cache", about = "Manage Con's SSH terminfo cache")]
struct SshCacheArgs {
    #[arg(long, conflicts_with_all = ["add", "remove", "host"])]
    clear: bool,
    #[arg(long, value_name = "HOST", conflicts_with_all = ["clear", "remove", "host"])]
    add: Option<String>,
    #[arg(long, value_name = "HOST", conflicts_with_all = ["clear", "add", "host"])]
    remove: Option<String>,
    #[arg(long, value_name = "HOST", conflicts_with_all = ["clear", "add", "remove"])]
    host: Option<String>,
    #[arg(long, value_name = "DAYS")]
    expire_days: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(name: &str) -> (Cache, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("con-ssh-cache-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        (Cache::at(root.join("nested/ssh_cache")), root)
    }

    #[test]
    fn cache_round_trip_and_update() {
        let (cache, root) = cache("round-trip");
        assert!(cache.add("user@example.com").unwrap());
        assert!(cache.contains("user@example.com", None).unwrap());
        assert!(!cache.add("user@example.com").unwrap());
        assert_eq!(cache.list(None).unwrap().len(), 1);
        cache.remove("user@example.com").unwrap();
        assert!(!cache.contains("user@example.com", None).unwrap());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_entries_are_ignored() {
        let (cache, root) = cache("malformed");
        fs::create_dir_all(cache.path.parent().unwrap()).unwrap();
        fs::write(
            &cache.path,
            "good.example|123|xterm-ghostty\nmalformed\n|2|xterm-ghostty\n",
        )
        .unwrap();
        assert_eq!(
            cache.list(None).unwrap(),
            vec![CacheEntry {
                host: "good.example".to_string(),
                timestamp: 123
            }]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn operations_are_mutually_exclusive() {
        let args = vec![
            std::ffi::OsString::from("--clear"),
            std::ffi::OsString::from("--host=x"),
        ];
        assert!(
            SshCacheArgs::try_parse_from(
                std::iter::once(std::ffi::OsString::from("con-cli +ssh-cache"))
                    .chain(args.into_iter())
            )
            .is_err()
        );
    }
}
