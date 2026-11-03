//! Protected Windows service.
//!
//! `guardian-agent install`   (run as admin) copies Guardian to Program Files,
//!                            locks down its settings folder and registers the
//!                            "GuardianService" service to start with Windows.
//! `guardian-agent uninstall` (run as admin) removes it and restores settings.
//!
//! The service runs as SYSTEM and starts the agent in whoever is signed in,
//! as SYSTEM too, so his standard account gets "Access denied" if he tries
//! to end it in Task Manager. If it ever stops, the service starts it again.

#![cfg(windows)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;
use windows_service::{
    define_windows_service,
    service::{ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType},
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};

pub const SERVICE_NAME: &str = "GuardianService";

pub fn install_dir() -> PathBuf {
    PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into())).join("Guardian")
}
pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into())).join("Guardian")
}

fn cmd(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program).args(args).status().map(|s| s.success()).unwrap_or(false)
}

fn is_admin() -> bool { cmd("net.exe", &["session"]) }

pub fn install() -> Result<(), String> {
    if !is_admin() { return Err("Please run this as administrator (right-click > Run as administrator).".into()); }
    let dir = install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join("guardian-agent.exe");
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    if me != target {
        let _ = cmd("sc.exe", &["stop", SERVICE_NAME]);
        std::thread::sleep(Duration::from_secs(2));
        std::fs::copy(&me, &target).map_err(|e| format!("copy failed: {e}"))?;
    }

    // Settings folder: only SYSTEM and admins can read it (it holds the pairing code).
    let data = data_dir();
    std::fs::create_dir_all(&data).map_err(|e| e.to_string())?;
    let d = data.to_string_lossy().to_string();
    cmd("icacls.exe", &[&d, "/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "/T", "/Q"]);
    // Program folder: everyone can run it, only admins can change it (that's the default; enforce it).
    let p = dir.to_string_lossy().to_string();
    cmd("icacls.exe", &[&p, "/inheritance:r", "/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "*S-1-5-32-545:(OI)(CI)RX", "/T", "/Q"]);

    // Let home-mode discovery messages through Windows Firewall (Guardian only).
    let prog = format!("program={}", target.display());
    cmd("netsh.exe", &["advfirewall", "firewall", "delete", "rule", "name=Guardian discovery"]);
    cmd("netsh.exe", &["advfirewall", "firewall", "add", "rule", "name=Guardian discovery", "dir=in", "action=allow",
        "protocol=UDP", "localport=47800", &prog, "profile=any"]);

    let bin = format!("\"{}\" service", target.display());
    if !cmd("sc.exe", &["query", SERVICE_NAME]) {
        if !cmd("sc.exe", &["create", SERVICE_NAME, "binPath=", &bin, "start=", "auto", "DisplayName=", "Guardian"]) {
            return Err("couldn't create the service".into());
        }
    }
    cmd("sc.exe", &["description", SERVICE_NAME, "Guardian family safety"]);
    // Restart automatically if it ever stops.
    cmd("sc.exe", &["failure", SERVICE_NAME, "reset=", "0", "actions=", "restart/2000/restart/2000/restart/2000"]);
    cmd("sc.exe", &["failureflag", SERVICE_NAME, "1"]);
    cmd("sc.exe", &["start", SERVICE_NAME]);
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    if !is_admin() { return Err("Please run this as administrator.".into()); }
    cmd("sc.exe", &["stop", SERVICE_NAME]);
    std::thread::sleep(Duration::from_secs(3));
    cmd("taskkill.exe", &["/F", "/IM", "guardian-agent.exe", "/FI", &format!("PID ne {}", std::process::id())]);
    cmd("sc.exe", &["delete", SERVICE_NAME]);
    cmd("netsh.exe", &["advfirewall", "firewall", "delete", "rule", "name=Guardian discovery"]);
    crate::winsetup::restore_dns();
    crate::winsetup::remove_browser_policies();
    Ok(())
}

// ------------------------------------------------------------ the service

define_windows_service!(ffi_service_main, service_main);

pub fn run_service() -> Result<(), String> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main).map_err(|e| e.to_string())
}

fn service_main(_args: Vec<OsString>) {
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let handler = move |c| match c {
        ServiceControl::Stop | ServiceControl::Shutdown => { let _ = stop_tx.send(()); ServiceControlHandlerResult::NoError }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let Ok(status) = service_control_handler::register(SERVICE_NAME, handler) else { return };
    let set = |state, accept| {
        let _ = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS, current_state: state, controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0), checkpoint: 0, wait_hint: Duration::from_secs(5), process_id: None,
        });
    };
    set(ServiceState::Running, ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN);

    let mut child: Option<session::Child> = None;
    loop {
        let active = session::active_session();
        let needs_start = match &child {
            None => active.is_some(),
            Some(c) => !c.running() || Some(c.session) != active,
        };
        if needs_start {
            if let Some(c) = child.take() { c.kill(); }
            if let Some(s) = active { child = session::spawn_agent(s); }
        }
        if stop_rx.recv_timeout(Duration::from_secs(2)).is_ok() { break; }
    }
    if let Some(c) = child.take() { c.kill(); }
    set(ServiceState::Stopped, ServiceControlAccept::empty());
}

/// Starting the agent inside the signed-in user's session, running as SYSTEM.
mod session {
    use std::{ffi::c_void, ptr::null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_TIMEOUT};
    use windows_sys::Win32::Security::{DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenPrimary, TokenSessionId, TOKEN_ALL_ACCESS};
    use windows_sys::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
    use windows_sys::Win32::System::Threading::{
        CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, TerminateProcess, WaitForSingleObject,
        CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTUPINFOW,
    };

    pub struct Child { process: HANDLE, pub session: u32 }
    unsafe impl Send for Child {}

    impl Child {
        pub fn running(&self) -> bool { unsafe { WaitForSingleObject(self.process, 0) == WAIT_TIMEOUT } }
        pub fn kill(self) { unsafe { TerminateProcess(self.process, 0); CloseHandle(self.process); } }
    }

    pub fn active_session() -> Option<u32> {
        let s = unsafe { WTSGetActiveConsoleSessionId() };
        (s != u32::MAX && s != 0).then_some(s)
    }

    fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

    pub fn spawn_agent(session: u32) -> Option<Child> {
        unsafe {
            let mut own: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own) == 0 { return None; }
            let mut tok: HANDLE = null_mut();
            let ok = DuplicateTokenEx(own, TOKEN_ALL_ACCESS, null_mut(), SecurityImpersonation, TokenPrimary, &mut tok);
            CloseHandle(own);
            if ok == 0 { return None; }
            let mut sid = session;
            if SetTokenInformation(tok, TokenSessionId, &mut sid as *mut u32 as *mut c_void, 4) == 0 { CloseHandle(tok); return None; }

            let exe = std::env::current_exe().ok()?.to_string_lossy().to_string();
            let mut cmdline = wide(&format!("\"{exe}\" run"));
            let mut desktop = wide("winsta0\\default");
            let mut si: STARTUPINFOW = std::mem::zeroed();
            si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
            si.lpDesktop = desktop.as_mut_ptr();
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessAsUserW(tok, null_mut(), cmdline.as_mut_ptr(), null_mut(), null_mut(), 0,
                CREATE_NO_WINDOW, null_mut(), null_mut(), &si, &mut pi);
            CloseHandle(tok);
            if ok == 0 { return None; }
            CloseHandle(pi.hThread);
            Some(Child { process: pi.hProcess, session })
        }
    }
}
