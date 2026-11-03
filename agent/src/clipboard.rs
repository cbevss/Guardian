//! Clipboard sync (text). Real clipboard on Windows; an in-memory stand-in elsewhere.
#[cfg(windows)]
pub fn get() -> Option<String> { arboard::Clipboard::new().ok()?.get_text().ok() }
#[cfg(windows)]
pub fn set(t: &str) -> bool { arboard::Clipboard::new().and_then(|mut c| c.set_text(t.to_owned())).is_ok() }

#[cfg(not(windows))]
static CLIP: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
#[cfg(not(windows))]
pub fn get() -> Option<String> {
    // Test hook: a file can stand in for "the user copied something".
    if let Ok(t) = std::env::var("GUARDIAN_TEST_CLIP").and_then(|p| std::fs::read_to_string(p).map_err(|_| std::env::VarError::NotPresent)) {
        *CLIP.lock().unwrap() = t;
    }
    Some(CLIP.lock().unwrap().clone())
}
#[cfg(not(windows))]
pub fn set(t: &str) -> bool { *CLIP.lock().unwrap() = t.to_owned(); println!("[clipboard] set {} chars", t.len()); true }
