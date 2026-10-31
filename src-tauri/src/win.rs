//! Windows-only platform glue: Job Objects for child process trees, TCP listener lookup,
//! power-resume notifications and taskbar theme detection.

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
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC, PBT_APMRESUMESUSPEND};

pub use windows_sys::Win32::System::Threading::{CREATE_NEW_CONSOLE, CREATE_NO_WINDOW};

/// Job handles (stored as isize so the map is Send) keyed by the managed child's PID.
static JOBS: Lazy<Mutex<HashMap<u32, isize>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Owns the Job Object for one managed child. Every process the child spawns joins the job, and the job is
/// created with KILL_ON_JOB_CLOSE, so the whole tree dies when the guard drops or Tunnel Bar exits/crashes.
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
