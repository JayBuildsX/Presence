//! Process start-time lookup.
//!
//! Lets plugins stamp their activities with the tracked application's real
//! process start time, so the displayed elapsed timer counts since the
//! application was launched — not since PresenceHub started tracking it.
//!
//! When the start time cannot be determined (no matching process, missing
//! permissions, non-Windows platform), the helpers return `None` and callers
//! must fall back to their session timer.

use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the start time of the earliest running process whose base name
/// matches one of `exe_names` (case-insensitive), as Unix seconds.
///
/// Returns `None` when no matching process is running or the start time
/// cannot be read. Callers must fall back to their session timer.
pub fn process_start_unix(exe_names: &[&str]) -> Option<i64> {
    process_start_time(exe_names).and_then(|time| {
        time.duration_since(UNIX_EPOCH)
            .ok()
            .map(|duration| duration.as_secs() as i64)
    })
}

/// Returns the start time of the earliest running process whose base name
/// matches one of `exe_names` (case-insensitive).
///
/// When several instances match, the earliest start wins, so the timer
/// reflects how long the application has been on. Returns `None` when no
/// matching process is running or the start time cannot be read.
pub fn process_start_time(exe_names: &[&str]) -> Option<SystemTime> {
    #[cfg(windows)]
    {
        windows::earliest_start(exe_names)
    }
    #[cfg(not(windows))]
    {
        let _ = exe_names;
        None
    }
}

#[cfg(windows)]
mod windows {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use winapi::shared::minwindef::{FALSE, FILETIME};
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::processthreadsapi::{GetProcessTimes, OpenProcess};
    use winapi::um::tlhelp32::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;

    /// 100-nanosecond intervals between the Windows and Unix epochs
    /// (11,644,473,600 seconds).
    const WINDOWS_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;

    pub fn earliest_start(exe_names: &[&str]) -> Option<SystemTime> {
        let mut earliest: Option<SystemTime> = None;
        for pid in matching_pids(exe_names) {
            if let Some(started) = creation_time(pid) {
                earliest = Some(earliest.map_or(started, |best| best.min(started)));
            }
        }
        earliest
    }

    /// PIDs of running processes whose base name matches one of `exe_names`.
    fn matching_pids(exe_names: &[&str]) -> Vec<u32> {
        let mut pids = Vec::new();
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot.is_null() {
                return pids;
            }

            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snapshot, &mut entry) != FALSE {
                loop {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                    if exe_names
                        .iter()
                        .any(|wanted| name.eq_ignore_ascii_case(wanted))
                    {
                        pids.push(entry.th32ProcessID);
                    }
                    if Process32NextW(snapshot, &mut entry) == FALSE {
                        break;
                    }
                }
            }

            CloseHandle(snapshot);
        }
        pids
    }

    /// Creation time of a process, via `GetProcessTimes`.
    fn creation_time(pid: u32) -> Option<SystemTime> {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
            if handle.is_null() {
                return None;
            }
            let mut created: FILETIME = std::mem::zeroed();
            let mut _exited: FILETIME = std::mem::zeroed();
            let mut _kernel: FILETIME = std::mem::zeroed();
            let mut _user: FILETIME = std::mem::zeroed();
            let ok = GetProcessTimes(handle, &mut created, &mut _exited, &mut _kernel, &mut _user);
            CloseHandle(handle);
            if ok == FALSE {
                return None;
            }
            filetime_to_system_time(created)
        }
    }

    /// Convert a `FILETIME` (100ns since 1601-01-01) to `SystemTime`.
    pub(super) fn filetime_to_system_time(filetime: FILETIME) -> Option<SystemTime> {
        let intervals = ((filetime.dwHighDateTime as u64) << 32) | (filetime.dwLowDateTime as u64);
        let unix_intervals = intervals.checked_sub(WINDOWS_TO_UNIX_EPOCH_100NS)?;
        UNIX_EPOCH.checked_add(Duration::from_nanos(unix_intervals * 100))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_process_has_no_start_time() {
        assert!(process_start_unix(&["definitely-not-a-real-process-xyz.exe"]).is_none());
        assert!(process_start_time(&["definitely-not-a-real-process-xyz.exe"]).is_none());
    }

    #[test]
    fn empty_wanted_list_has_no_start_time() {
        assert!(process_start_unix(&[]).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn filetime_conversion_matches_known_answer() {
        use winapi::shared::minwindef::FILETIME;

        // 2026-09-03T12:00:00Z == 1,788,436,800 Unix seconds.
        let unix_secs = 1_788_436_800u64;
        let raw = unix_secs * 10_000_000 + 116_444_736_000_000_000;
        let filetime = FILETIME {
            dwLowDateTime: (raw & 0xFFFF_FFFF) as u32,
            dwHighDateTime: (raw >> 32) as u32,
        };
        let converted =
            windows::filetime_to_system_time(filetime).expect("in-range FILETIME must convert");
        let round_tripped = converted
            .duration_since(std::time::UNIX_EPOCH)
            .expect("must be after the Unix epoch")
            .as_secs();
        assert_eq!(round_tripped, unix_secs);
    }

    #[test]
    fn start_time_is_not_in_the_future() {
        // The test runner itself is a process; its start time must be known
        // and must not lie in the future.
        let exe: Option<String> = std::env::current_exe()
            .ok()
            .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()));
        let Some(exe) = exe else {
            return;
        };
        let started = process_start_time(std::slice::from_ref(&exe.as_str()));
        #[cfg(windows)]
        {
            let started = started.expect("own test process should have a start time");
            assert!(
                started <= SystemTime::now(),
                "process start must not be in the future"
            );
            let unix = process_start_unix(std::slice::from_ref(&exe.as_str()))
                .expect("unix conversion should succeed");
            assert!(unix > 0);
        }
        #[cfg(not(windows))]
        {
            assert!(started.is_none());
        }
    }
}
