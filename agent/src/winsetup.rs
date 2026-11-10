//! Windows settings Guardian keeps in place (needs to run as SYSTEM/admin,
//! which the protected service does). His standard account can't change these.

#[cfg(windows)]
fn run(program: &str, args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(program).args(args).creation_flags(CREATE_NO_WINDOW)
        .status().map(|s| s.success()).unwrap_or(false)
}

#[cfg(windows)]
fn powershell(script: &str) -> bool {
    run("powershell.exe", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
}

/// Points every network adapter at Guardian's website filter.
#[cfg(windows)]
pub fn apply_dns() {
    powershell("Get-NetAdapter | Where-Object Status -eq 'Up' | ForEach-Object { \
        $cur = (Get-DnsClientServerAddress -InterfaceIndex $_.ifIndex).ServerAddresses; \
        if (-not ($cur -contains '127.0.0.1')) { \
          Set-DnsClientServerAddress -InterfaceIndex $_.ifIndex -ServerAddresses ('127.0.0.1','::1'); \
          Clear-DnsClientCache } }");
}

/// Puts adapters back to automatic DNS (used when uninstalling).
#[cfg(windows)]
pub fn restore_dns() {
    powershell("Get-NetAdapter | ForEach-Object { Set-DnsClientServerAddress -InterfaceIndex $_.ifIndex -ResetServerAddresses }; Clear-DnsClientCache");
}

/// Stops browsers using their own "secure DNS", which would skip the filter.
#[cfg(windows)]
pub fn apply_browser_policies() {
    for key in [r"HKLM\SOFTWARE\Policies\Google\Chrome", r"HKLM\SOFTWARE\Policies\Microsoft\Edge", r"HKLM\SOFTWARE\Policies\BraveSoftware\Brave"] {
        run("reg.exe", &["add", key, "/v", "DnsOverHttpsMode", "/t", "REG_SZ", "/d", "off", "/f"]);
    }
    let ff = r"HKLM\SOFTWARE\Policies\Mozilla\Firefox\DNSOverHTTPS";
    run("reg.exe", &["add", ff, "/v", "Enabled", "/t", "REG_DWORD", "/d", "0", "/f"]);
    run("reg.exe", &["add", ff, "/v", "Locked", "/t", "REG_DWORD", "/d", "1", "/f"]);
}

#[cfg(windows)]
pub fn remove_browser_policies() {
    for key in [r"HKLM\SOFTWARE\Policies\Google\Chrome", r"HKLM\SOFTWARE\Policies\Microsoft\Edge", r"HKLM\SOFTWARE\Policies\BraveSoftware\Brave"] {
        run("reg.exe", &["delete", key, "/v", "DnsOverHttpsMode", "/f"]);
    }
    run("reg.exe", &["delete", r"HKLM\SOFTWARE\Policies\Mozilla\Firefox\DNSOverHTTPS", "/f"]);
}

/// Locks the screen (used outside allowed hours).
#[cfg(windows)]
pub fn lock_screen() { run("rundll32.exe", &["user32.dll,LockWorkStation"]); }

// Elsewhere these only log, so the rest of Guardian can be tested anywhere.
#[cfg(not(windows))] pub fn apply_dns() {}
#[cfg(not(windows))] pub fn apply_browser_policies() {}
#[cfg(not(windows))] pub fn lock_screen() { println!("[schedule] outside allowed hours: screen locked"); }

/// True when someone is at the desktop (Windows' lock screen isn't showing).
fn at_desktop() -> bool {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    let names: Vec<String> = sys.processes().values().map(|p| p.name().to_string_lossy().to_lowercase()).collect();
    names.iter().any(|n| n == "explorer.exe" || cfg!(not(windows))) && !names.iter().any(|n| n == "logonui.exe")
}

/// Keeps the settings in place (e.g. a new Wi-Fi adapter appears).
pub async fn keep_applied() {
    loop {
        let _ = tokio::task::spawn_blocking(|| { apply_dns(); apply_browser_policies(); }).await;
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

/// Locks the PC whenever the schedule says it shouldn't be in use.
pub async fn schedule_enforcer(shared: crate::policy::Shared) {
    let mut was_allowed = true;
    loop {
        // Count time used while he's signed in and at the desktop (not the lock screen).
        let in_use = tokio::task::spawn_blocking(at_desktop).await.unwrap_or(false);
        if in_use && !shared.overridden() { crate::activity::add_seconds(15); }
        let within_hours = shared.schedule_allows_now();
        let time_left = shared.time_left();
        let allowed = within_hours && time_left;
        if !allowed {
            if was_allowed {
                println!("[schedule] PC locked");
                crate::activity::log("time", if !time_left { "Daily time limit reached, PC locked" } else { "Outside allowed hours, PC locked" });
            }
            let _ = tokio::task::spawn_blocking(lock_screen).await;
        } else if !was_allowed {
            println!("[schedule] allowed hours started");
        }
        was_allowed = allowed;
        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
    }
}
