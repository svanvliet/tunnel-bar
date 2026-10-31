//! Windows-only platform glue: Job Objects for child process trees, TCP listener lookup,
//! power-resume notifications, taskbar theme detection and elevated-process handling.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::Notify;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::Power::{DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION, ResumeThread, THREAD_SUSPEND_RESUME,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND};

pub use windows_sys::Win32::System::Threading::{CREATE_NEW_CONSOLE, CREATE_NO_WINDOW, CREATE_SUSPENDED};

/// Job handles (stored as isize so the map is Send) keyed by the managed child's PID.
static JOBS: Lazy<Mutex<HashMap<u32, isize>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Owns the Job Object for one managed child. Every process the child spawns joins the job, and the job is
/// created with KILL_ON_JOB_CLOSE, so the whole tree dies when the guard drops or Tunnel Bar exits/crashes.
/// Attach while the child is still suspended (CREATE_SUSPENDED), then `resume_process`, so nothing it spawns
/// can start before it is in the job.
pub struct JobGuard(u32);

impl JobGuard {
    pub fn attach(pid: u32, process: HANDLE) -> Result<Self, String> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(format!("CreateJobObject failed: {}", std::io::Error::last_os_error()));
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0
                && AssignProcessToJobObject(job, process) != 0;
            if !ok {
                let e = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(format!("could not assign process to job: {e}"));
            }
            JOBS.lock().unwrap().insert(pid, job as isize);
        }
        Ok(Self(pid))
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        if let Some(job) = JOBS.lock().unwrap().remove(&self.0) {
            // Closing the last handle kills anything still in the job (e.g. grandchildren).
            unsafe { CloseHandle(job as HANDLE) };
        }
    }
}

/// Terminates every process in the child's job. Returns false if the PID has no job.
pub fn terminate_job(pid: u32) -> bool {
    let Some(job) = JOBS.lock().unwrap().remove(&pid) else { return false };
    unsafe {
        TerminateJobObject(job as HANDLE, 1);
        CloseHandle(job as HANDLE);
    }
    true
}

/// Kills a process and its descendants (for PIDs we don't hold a job for, e.g. orphans or external processes).
pub fn kill_tree(pid: u32) {
    let Ok(mut sys) = sysinfo::System::new() else { return };
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let parents: Vec<(u32, Option<u32>)> =
        sys.processes().iter().map(|(p, pr)| (p.as_u32(), pr.parent().map(|x| x.as_u32()))).collect();
    // Children first so they can't be reparented mid-kill.
    for p in crate::external::descendants(pid, &parents).into_iter().rev() {
        if let Some(pr) = sys.process(sysinfo::Pid::from_u32(p)) {
            pr.kill();
        }
    }
}

/// Resumes every thread of a process created with CREATE_SUSPENDED (std doesn't expose the main thread handle).
pub fn resume_process(pid: u32) -> Result<(), String> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snap.is_null() || snap == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return Err(format!("CreateToolhelp32Snapshot failed: {}", std::io::Error::last_os_error()));
        }
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0;
        let mut more = Thread32First(snap, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let t = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if !t.is_null() {
                    if ResumeThread(t) != u32::MAX {
                        resumed += 1;
                    }
                    CloseHandle(t);
                }
            }
            more = Thread32Next(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        if resumed == 0 { Err(format!("could not resume process {pid}")) } else { Ok(()) }
    }
}

/// Reads another process's command line with NtQueryInformationProcess(ProcessCommandLineInformation), which
/// needs only PROCESS_QUERY_LIMITED_INFORMATION (sysinfo reads the PEB, which elevated processes deny us).
pub fn process_cmdline(pid: u32) -> Option<Vec<String>> {
    use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
    use windows_sys::Win32::Foundation::UNICODE_STRING;
    use windows_sys::Win32::UI::Shell::CommandLineToArgvW;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        // u64 buffer keeps the UNICODE_STRING header aligned; the string data follows it.
        let mut buf = vec![0u64; 8192];
        let mut len = 0u32;
        let status = NtQueryInformationProcess(
            h,
            ProcessCommandLineInformation,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            &mut len,
        );
        CloseHandle(h);
        if status < 0 {
            return None;
        }
        let us = &*buf.as_ptr().cast::<UNICODE_STRING>();
        if us.Buffer.is_null() || us.Length == 0 {
            return None;
        }
        let mut wide: Vec<u16> = std::slice::from_raw_parts(us.Buffer, us.Length as usize / 2).to_vec();
        wide.push(0);
        let mut argc = 0i32;
        let argv = CommandLineToArgvW(wide.as_ptr(), &mut argc);
        if argv.is_null() {
            return None;
        }
        let args = (0..argc as usize)
            .map(|i| {
                let p = *argv.add(i);
                let n = (0..).take_while(|&j| *p.add(j) != 0).count();
                String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
            })
            .collect();
        windows_sys::Win32::Foundation::LocalFree(argv.cast());
        Some(args)
    }
}

/// Whether this (unelevated) process may terminate `pid`; false for processes started elevated.
pub fn can_terminate(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::PROCESS_TERMINATE;
    unsafe {
        let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if h.is_null() {
            return false;
        }
        CloseHandle(h);
        true
    }
}

/// Runs `taskkill /PID <pid> /T /F` elevated via a UAC prompt and waits for it.
/// Err(true) when the user declined the prompt, Err(false) on any other failure.
pub fn kill_tree_elevated(pid: u32) -> Result<(), bool> {
    use windows_sys::Win32::Foundation::{ERROR_CANCELLED, GetLastError, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    use windows_sys::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (verb, file, params) = (wide("runas"), wide("taskkill.exe"), wide(&format!("/PID {pid} /T /F")));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = params.as_ptr();
        info.nShow = SW_HIDE;
        if ShellExecuteExW(&mut info) == 0 {
            return Err(GetLastError() == ERROR_CANCELLED);
        }
        if info.hProcess.is_null() {
            return Ok(());
        }
        let waited = WaitForSingleObject(info.hProcess, 15_000);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        if waited == WAIT_OBJECT_0 && code == 0 { Ok(()) } else { Err(false) }
    }
}

/// Whether the taskbar uses the light theme (Settings > Personalization > Colors > "Choose your mode").
/// Defaults to dark, the Windows 11 default, if the value is missing.
pub fn taskbar_is_light() -> bool {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("SystemUsesLightTheme");
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    rc == 0 && value != 0
}

pub fn alive(pid: u32) -> bool {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        ok && code == STILL_ACTIVE as u32
    }
}

/// All local TCP listeners as (port, owning pid), IPv4 and IPv6.
pub fn tcp_listeners() -> Vec<(u16, u32)> {
    let mut out = Vec::new();
    for af in [AF_INET, AF_INET6] {
        let mut size = 0u32;
        unsafe {
            GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, af as u32, TCP_TABLE_OWNER_PID_LISTENER, 0);
        }
        if size == 0 {
            continue;
        }
        // u32 buffer keeps the table header (a u32 count) and rows aligned.
        let mut buf = vec![0u32; (size as usize).div_ceil(4) + 16];
        size = (buf.len() * 4) as u32;
        let rc = unsafe {
            GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut size, 0, af as u32, TCP_TABLE_OWNER_PID_LISTENER, 0)
        };
        if rc != 0 {
            continue;
        }
        let n = buf[0] as usize;
        let rows = unsafe { buf.as_ptr().add(1).cast::<u8>() };
        let port = |p: u32| u16::from_be((p & 0xFFFF) as u16);
        for i in 0..n {
            unsafe {
                if af == AF_INET {
                    let r = &*rows.cast::<MIB_TCPROW_OWNER_PID>().add(i);
                    out.push((port(r.dwLocalPort), r.dwOwningPid));
                } else {
                    let r = &*rows.cast::<MIB_TCP6ROW_OWNER_PID>().add(i);
                    out.push((port(r.dwLocalPort), r.dwOwningPid));
                }
            }
        }
    }
    out
}

static RESUMED: Lazy<Notify> = Lazy::new(Notify::new);

/// Fires after the machine resumes from sleep/hibernate.
pub fn resumed() -> &'static Notify {
    &RESUMED
}

unsafe extern "system" fn power_cb(_ctx: *const core::ffi::c_void, kind: u32, _setting: *const core::ffi::c_void) -> u32 {
    if kind == PBT_APMRESUMEAUTOMATIC || kind == PBT_APMRESUMESUSPEND {
        RESUMED.notify_one();
    }
    0
}

/// Subscribes to suspend/resume notifications (no window required). Registration lives for the process.
pub fn register_power_notifications() -> Result<(), String> {
    // Must outlive the registration, which lasts for the life of the process.
    let params: &'static DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS = Box::leak(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
        Callback: Some(power_cb),
        Context: std::ptr::null_mut(),
    }));
    let mut handle: *mut core::ffi::c_void = std::ptr::null_mut();
    let rc = unsafe {
        PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, params as *const _ as HANDLE, &mut handle)
    };
    if rc == 0 { Ok(()) } else { Err(format!("PowerRegisterSuspendResumeNotification failed ({rc})")) }
}
