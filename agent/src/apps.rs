//! Works out which folder an app "lives in", so approving or denying an app
//! covers everything in that folder (helpers, updaters, launchers) and the
//! app keeps working properly.
//!
//! Paths are handled as plain text in a normalised form (lowercase, `/`
//! separators) so the rules behave the same everywhere and can be tested.

/// Lowercase with forward slashes and no trailing slash.
pub fn norm(path: &str) -> String {
    let mut p = path.trim().replace('\\', "/").to_lowercase();
    while p.len() > 1 && p.ends_with('/') { p.pop(); }
    p
}

/// True if `path` is `folder` or anything inside it.
pub fn inside(path: &str, folder: &str) -> bool {
    path == folder || (path.starts_with(folder) && path.as_bytes().get(folder.len()) == Some(&b'/'))
}

fn components(p: &str) -> Vec<&str> { p.split('/').filter(|c| !c.is_empty()).collect() }

/// "c:/users/sam" for any path inside a user's folder, else "".
fn home_of(p: &str) -> String {
    let c = components(p);
    if c.len() >= 3 && c[1] == "users" { format!("{}/{}/{}", c[0], c[1], c[2]) } else { String::new() }
}

/// Folders where each sub-folder is a separate app.
fn install_roots(user_home: &str) -> Vec<String> {
    let mut v: Vec<String> = [
        "c:/program files", "c:/program files (x86)", "c:/programdata",
        "c:/xboxgames", "c:/program files/windowsapps",
    ].iter().map(|s| s.to_string()).collect();
    if !user_home.is_empty() {
        for sub in ["appdata/local/programs", "appdata/local", "appdata/roaming"] {
            v.push(format!("{user_home}/{sub}"));
        }
    }
    if let Ok(t) = std::env::var("GUARDIAN_TEST_APPS") { v.push(norm(&t)); }
    // Most specific first, so appdata/local/programs wins over appdata/local.
    v.sort_by_key(|r| std::cmp::Reverse(r.len()));
    v
}

/// Launcher folders that hold many separate games: each game gets its own folder.
const LIBRARIES: &[&str] = &[
    "steamapps/common", "epic games", "riot games", "ubisoft/ubisoft game launcher/games",
    "ea games", "gog galaxy/games", "origin games", "battle.net", "games",
    // Vendor folders that contain several unrelated apps:
    "microsoft", "google", "common files", "packages",
];

/// Folders far too broad to approve as a whole.
pub fn too_broad(folder: &str, user_home: &str) -> bool {
    let folder = norm(folder);
    let folder = folder.as_str();
    let derived = if user_home.is_empty() { home_of(folder) } else { norm(user_home) };
    let user_home = derived.as_str();
    let c = components(folder);
    if c.len() <= 1 { return true; } // a whole drive
    if folder.starts_with("c:/windows") { return true; }
    if install_roots(user_home).iter().any(|r| r == folder) { return true; }
    if LIBRARIES.iter().any(|l| folder.ends_with(&format!("/{l}"))) { return true; }
    if folder == "c:/users" || folder == user_home { return true; }
    if !user_home.is_empty() {
        for sub in ["desktop", "downloads", "documents", "appdata", "onedrive", "pictures", "videos", "music"] {
            if folder == format!("{user_home}/{sub}") { return true; }
        }
    }
    false
}

/// The folder an app belongs to. Falls back to the exe itself when its
/// folder is too broad (e.g. a game exe sitting in Downloads).
pub fn app_home(exe: &str, user_home: &str) -> String {
    let exe = norm(exe);
    let user_home = if user_home.is_empty() { home_of(&exe) } else { norm(user_home) };

    // Inside a game library: the game's own folder.
    for lib in LIBRARIES {
        let marker = format!("/{lib}/");
        if let Some(i) = exe.find(&marker) {
            let rest = &exe[i + marker.len()..];
            if let Some(game) = rest.split('/').next() {
                if rest.contains('/') { return exe[..i + marker.len() + game.len()].to_string(); }
            }
        }
    }
    // Inside an install root: the first folder below it.
    for root in install_roots(&user_home) {
        if inside(&exe, &root) && exe != root {
            let rest = &exe[root.len() + 1..];
            let first = rest.split('/').next().unwrap_or("");
            if rest.contains('/') { return format!("{root}/{first}"); }
            return exe; // exe sitting directly in the root
        }
    }
    // Anywhere else: the exe's own folder, unless that's too broad.
    match exe.rsplit_once('/') {
        Some((dir, _)) if !too_broad(dir, &user_home) => dir.to_string(),
        _ => exe,
    }
}

/// A friendly name for a request popup, e.g. "Steam" or "Minecraft.exe".
pub fn display_name(home: &str) -> String {
    let last = home.rsplit('/').next().unwrap_or(home);
    let mut c = last.chars();
    match c.next() { Some(f) => f.to_uppercase().chain(c).collect(), None => last.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    const H: &str = "C:\\Users\\Sam";

    #[test] fn program_files_app_covers_whole_folder() {
        assert_eq!(app_home(r"C:\Program Files (x86)\Steam\steam.exe", H), "c:/program files (x86)/steam");
        assert_eq!(app_home(r"C:\Program Files (x86)\Steam\bin\cef\steamwebhelper.exe", H), "c:/program files (x86)/steam");
    }
    #[test] fn each_steam_game_is_separate_from_steam() {
        assert_eq!(app_home(r"C:\Program Files (x86)\Steam\steamapps\common\Rocket League\Binaries\Win64\RocketLeague.exe", H),
                   "c:/program files (x86)/steam/steamapps/common/rocket league");
        assert_eq!(app_home(r"D:\SteamLibrary\steamapps\common\Portal 2\portal2.exe", H), "d:/steamlibrary/steamapps/common/portal 2");
    }
    #[test] fn epic_and_riot_games_are_separate() {
        assert_eq!(app_home(r"C:\Program Files\Epic Games\Fortnite\FortniteGame\Binaries\Win64\FortniteClient.exe", H),
                   "c:/program files/epic games/fortnite");
        assert_eq!(app_home(r"C:\Riot Games\VALORANT\live\VALORANT.exe", H), "c:/riot games/valorant");
    }
    #[test] fn appdata_apps() {
        assert_eq!(app_home(r"C:\Users\Sam\AppData\Local\Discord\app-1.0.9\Discord.exe", H), "c:/users/sam/appdata/local/discord");
        assert_eq!(app_home(r"C:\Users\Sam\AppData\Local\Programs\Roblox\Versions\v1\RobloxPlayerBeta.exe", H),
                   "c:/users/sam/appdata/local/programs/roblox");
    }
    #[test] fn loose_exe_in_downloads_is_just_that_exe() {
        assert_eq!(app_home(r"C:\Users\Sam\Downloads\cheat.exe", H), "c:/users/sam/downloads/cheat.exe");
        assert_eq!(app_home(r"C:\Users\Sam\Desktop\game.exe", H), "c:/users/sam/desktop/game.exe");
    }
    #[test] fn other_drive_uses_exe_folder() {
        assert_eq!(app_home(r"D:\Games\Minecraft\Minecraft.exe", H), "d:/games/minecraft");
    }
    #[test] fn broad_folders_rejected() {
        let h = norm(H);
        for f in ["c:", "c:/windows/system32", "c:/program files", "c:/users/sam", "c:/users/sam/downloads",
                  "c:/program files (x86)/steam/steamapps/common", "c:/program files/epic games"] {
            assert!(too_broad(f, &h), "{f}");
        }
        assert!(!too_broad("c:/program files (x86)/steam", &h));
    }
    #[test] fn vendor_folders_split_by_app() {
        assert_eq!(app_home(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe", ""), "c:/program files (x86)/microsoft/edge");
        assert_eq!(app_home(r"C:\Users\Sam\AppData\Local\Microsoft\OneDrive\OneDrive.exe", ""), "c:/users/sam/appdata/local/microsoft/onedrive");
        assert_eq!(app_home(r"C:\Program Files\Google\Chrome\Application\chrome.exe", ""), "c:/program files/google/chrome");
    }
    #[test] fn works_without_knowing_user_home() {
        assert_eq!(app_home(r"C:\Users\Sam\Downloads\cheat.exe", ""), "c:/users/sam/downloads/cheat.exe");
        assert!(too_broad("C:\\Users\\Sam\\Desktop", ""));
    }
    #[test] fn inside_is_exact() {
        assert!(inside("c:/games/steam/x.exe", "c:/games/steam"));
        assert!(!inside("c:/games/steamy/x.exe", "c:/games/steam"));
    }
}
