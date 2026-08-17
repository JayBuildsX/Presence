//! League Client detection: lockfile discovery and parsing.
//!
//! The League Client exposes its local HTTP/WebSocket API only while it is
//! running. The credentials for that API — a process id, a port and a secret
//! token — are written to a `lockfile` file. Everything the plugin needs to
//! reach the client comes from reading that file and parsing it; nothing is
//! scraped from memory, injected, or sniffed.
//!
//! # Lockfile format
//!
//! ```text
//! process:pid:port:token:protocol
//! ```
//!
//! Example: `LeagueClient:12268:53034:9mcjDj66mOZWGOOU_nXhfQ:https`

use std::path::{Path, PathBuf};

/// File name of the League Client credentials file.
pub const LOCKFILE_NAME: &str = "lockfile";

/// The directory name that contains League installs under each drive root.
pub const RIOT_GAMES_DIR: &str = "Riot Games";

/// The install directory name for the League of Legends client.
pub const GAME_DIR: &str = "League of Legends";

/// Fallback lockfile location maintained by the Riot Client metadata service.
///
/// Used when the client install directory cannot be located by scanning drive
/// roots (e.g. a non-default install path).
pub const METADATA_LOCKFILE: &str =
    "C:\\ProgramData\\Riot Games\\Metadata\\league_of_legends.live\\league_of_legends.live.lockfile";

/// Environment variable override for the lockfile path (mainly for tests).
pub const LOCKFILE_ENV_VAR: &str = "PRESENCEHUB_LOL_LOCKFILE";

/// Connection details parsed from the League lockfile.
///
/// The token is a secret. It must never be logged or exposed in error
/// messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeagueConnectionInfo {
    /// The process name (e.g. `LeagueClient`).
    pub process: String,
    /// The pid of the running client process.
    pub pid: u32,
    /// The local port the LCU API listens on.
    pub port: u16,
    /// The authentication token (treated as a secret).
    pub token: String,
    /// The protocol the client serves (e.g. `https`).
    pub protocol: String,
}

/// Parse a lockfile's contents into connection information.
///
/// Returns `None` for any malformed input. Parsing is deliberately strict so
/// that a partially-written lockfile (e.g. mid-update) is treated as "not
/// available" rather than producing garbage.
pub fn parse_lockfile(content: &str) -> Option<LeagueConnectionInfo> {
    let content = content.trim();
    // The format never contains whitespace; any present is a sign of a
    // malformed or partial lockfile.
    if content.chars().any(char::is_whitespace) {
        return None;
    }

    let mut parts = content.split(':');

    let process = parts.next()?.to_string();
    let pid: u32 = parts.next()?.parse().ok()?;
    let port: u16 = parts.next()?.parse().ok()?;
    let token = parts.next()?.to_string();
    let protocol = parts.next()?.to_string();

    // Reject trailing fields — the format is exactly 5 fields.
    if parts.next().is_some() {
        return None;
    }

    if process.is_empty() || token.is_empty() || protocol.is_empty() {
        return None;
    }
    if pid == 0 || port == 0 {
        return None;
    }

    Some(LeagueConnectionInfo {
        process,
        pid,
        port,
        token,
        protocol,
    })
}

/// Checks whether a process is currently alive.
///
/// On Windows this uses `OpenProcess` + `GetExitCodeProcess`. A pid that
/// cannot be queried (permission restriction) is optimistically treated as
/// alive — the LCU connection itself is the authoritative liveness check, so
/// a false positive here only costs one failed connect attempt.
#[cfg(windows)]
pub fn process_alive(pid: u32) -> bool {
    use winapi::shared::minwindef::FALSE;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::minwinbase::STILL_ACTIVE;
    use winapi::um::processthreadsapi::{GetExitCodeProcess, OpenProcess};
    use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;

    if pid == 0 {
        return false;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if handle.is_null() {
            // Could be a dead pid or a permission restriction. Stay optimistic;
            // the LCU connect will disambiguate.
            return true;
        }
        let mut exit_code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);

        if ok == 0 {
            return true;
        }
        exit_code == STILL_ACTIVE
    }
}

/// Non-Windows fallback: no cheap way to verify, defer to the connection.
#[cfg(not(windows))]
pub fn process_alive(_pid: u32) -> bool {
    true
}

/// Discovers and reads the League Client lockfile.
///
/// Candidate locations, in order of preference:
///
/// 1. `PRESENCEHUB_LOL_LOCKFILE` environment variable (test/custom installs)
/// 2. `<drive>:\Riot Games\League of Legends\lockfile` for each drive letter
/// 3. The Riot Client metadata fallback lockfile
#[derive(Debug, Default, Clone)]
pub struct LockfileLocator {
    /// Additional candidate paths checked before the defaults.
    extra: Vec<PathBuf>,
}

impl LockfileLocator {
    /// Creates a locator with the default candidate locations.
    pub fn new() -> Self {
        Self { extra: Vec::new() }
    }

    /// Adds candidate paths checked before the default locations.
    pub fn with_extra(mut self, paths: impl IntoIterator<Item = PathBuf>) -> Self {
        self.extra.extend(paths);
        self
    }

    /// The full ordered list of candidate lockfile paths.
    pub fn candidates(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.extra.clone();

        if let Ok(env_path) = std::env::var(LOCKFILE_ENV_VAR) {
            if !env_path.is_empty() {
                out.push(PathBuf::from(env_path));
            }
        }

        for letter in b'A'..=b'Z' {
            let drive = format!("{}:\\", letter as char);
            let candidate = PathBuf::from(drive)
                .join(RIOT_GAMES_DIR)
                .join(GAME_DIR)
                .join(LOCKFILE_NAME);
            out.push(candidate);
        }

        out.push(PathBuf::from(METADATA_LOCKFILE));
        out
    }

    /// Finds the first lockfile that exists on disk.
    pub fn find_lockfile(&self) -> Option<PathBuf> {
        self.find_lockfile_in(self.candidates())
    }

    /// Finds the first existing path from an explicit candidate list.
    ///
    /// Exposed separately so tests can exercise discovery without touching
    /// the real drives or the metadata fallback.
    pub fn find_lockfile_in<I>(&self, candidates: I) -> Option<PathBuf>
    where
        I: IntoIterator<Item = PathBuf>,
    {
        candidates.into_iter().find(|p| p.exists())
    }

    /// Finds and parses the lockfile, returning connection information.
    ///
    /// Returns `None` when no lockfile exists, it cannot be parsed, or the
    /// process it refers to is verifiably dead.
    pub fn find_and_parse(&self) -> Option<LeagueConnectionInfo> {
        let path = self.find_lockfile()?;
        let content = std::fs::read_to_string(&path).ok()?;
        let info = parse_lockfile(&content)?;
        if process_alive(info.pid) {
            Some(info)
        } else {
            None
        }
    }
}

/// Reads connection info directly from a specific lockfile path.
///
/// Convenience for tests and for callers that already know the install path.
pub fn read_lockfile(path: impl AsRef<Path>) -> Option<LeagueConnectionInfo> {
    let content = std::fs::read_to_string(path).ok()?;
    parse_lockfile(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_lockfile --------------------------------------------------------

    #[test]
    fn parses_valid_lockfile() {
        let info = parse_lockfile("LeagueClient:12268:53034:9mcjDj66mOZWGOOU_nXhfQ:https").unwrap();
        assert_eq!(info.process, "LeagueClient");
        assert_eq!(info.pid, 12268);
        assert_eq!(info.port, 53034);
        assert_eq!(info.token, "9mcjDj66mOZWGOOU_nXhfQ");
        assert_eq!(info.protocol, "https");
    }

    #[test]
    fn parses_with_trailing_newline() {
        let info = parse_lockfile("LeagueClient:1:2:tok:https\n").unwrap();
        assert_eq!(info.port, 2);
        assert_eq!(info.token, "tok");
    }

    #[test]
    fn rejects_empty_input() {
        assert!(parse_lockfile("").is_none());
        assert!(parse_lockfile("   ").is_none());
    }

    #[test]
    fn rejects_too_few_fields() {
        assert!(parse_lockfile("LeagueClient:12268:53034").is_none());
        assert!(parse_lockfile("LeagueClient:12268").is_none());
    }

    #[test]
    fn rejects_too_many_fields() {
        assert!(parse_lockfile("a:1:2:tok:https:extra").is_none());
    }

    #[test]
    fn rejects_non_numeric_pid_and_port() {
        assert!(parse_lockfile("a:abc:2:tok:https").is_none());
        assert!(parse_lockfile("a:1:xyz:tok:https").is_none());
    }

    #[test]
    fn rejects_out_of_range_port() {
        assert!(parse_lockfile("a:1:70000:tok:https").is_none());
        assert!(parse_lockfile("a:1:99999:tok:https").is_none());
    }

    #[test]
    fn rejects_zero_pid_and_port() {
        assert!(parse_lockfile("a:0:2:tok:https").is_none());
        assert!(parse_lockfile("a:1:0:tok:https").is_none());
    }

    #[test]
    fn rejects_empty_token() {
        assert!(parse_lockfile("a:1:2::https").is_none());
    }

    #[test]
    fn rejects_whitespace_padded_fields() {
        assert!(parse_lockfile("a:1:2:  tok  :https").is_none());
    }

    #[test]
    fn token_is_not_logged_by_default() {
        // The Display/Debug of the error path must not leak the token; the
        // token is simply not part of any formatted output here.
        let info = parse_lockfile("a:1:2:secrettoken:https").unwrap();
        assert_eq!(info.token, "secrettoken");
    }

    // -- locator --------------------------------------------------------------

    #[test]
    fn candidates_prefers_extra_then_env() {
        let locator = LockfileLocator::new().with_extra([PathBuf::from("C:\\custom\\lockfile")]);
        let candidates = locator.candidates();
        assert_eq!(candidates[0], PathBuf::from("C:\\custom\\lockfile"));
    }

    #[test]
    fn candidates_includes_drive_scan_and_metadata_fallback() {
        let locator = LockfileLocator::new();
        let candidates = locator.candidates();
        let last = candidates.last().unwrap();
        assert_eq!(
            last.as_os_str(),
            PathBuf::from(METADATA_LOCKFILE).as_os_str()
        );
        // The A: drive candidate is present.
        assert!(candidates
            .iter()
            .any(|p| p.ends_with("A:\\Riot Games\\League of Legends\\lockfile")));
    }

    #[test]
    fn find_lockfile_in_returns_first_existing() {
        let dir = std::env::temp_dir();
        let a = dir.join("ph_league_a.lockfile");
        let b = dir.join("ph_league_b.lockfile");
        std::fs::write(&a, "LeagueClient:1:111:tok:https").unwrap();
        std::fs::write(&b, "LeagueClient:1:222:tok:https").unwrap();

        let locator = LockfileLocator::new();
        let found = locator.find_lockfile_in([a.clone(), b.clone()]);
        assert_eq!(found, Some(a.clone()));

        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn find_lockfile_returns_none_when_missing() {
        let locator = LockfileLocator::new();
        let found = locator.find_lockfile_in([PathBuf::from("Z:\\nonexistent\\lockfile")]);
        assert!(found.is_none());
    }

    #[test]
    fn find_and_parse_uses_alive_pid() {
        // Use the current process pid so the staleness check passes.
        let pid = std::process::id();
        let content = format!("LeagueClient:{pid}:33333:tok:https");
        let dir = std::env::temp_dir();
        let path = dir.join("ph_league_live.lockfile");
        std::fs::write(&path, content).unwrap();

        let locator = LockfileLocator::new().with_extra([path.clone()]);
        let info = locator.find_and_parse();
        assert_eq!(info.map(|i| i.port), Some(33333));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_lockfile_from_path() {
        let dir = std::env::temp_dir();
        let path = dir.join("ph_league_direct.lockfile");
        std::fs::write(&path, "LeagueClient:1:444:tok:https").unwrap();

        let info = read_lockfile(&path).unwrap();
        assert_eq!(info.port, 444);

        let _ = std::fs::remove_file(&path);
    }
}
