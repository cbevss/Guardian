//! "Hide his screen": covers every monitor on his PC with the Guardian logo.
//!
//! The cover is left out of screen capture (Windows 10 2004+), so parents still
//! see and can control his real desktop while he only sees the logo. While a
//! parent has control, the cover lets their clicks and typing through to the
//! apps underneath; otherwise it takes focus so his own typing goes nowhere.

use std::sync::atomic::{AtomicBool, Ordering};

static WANT: AtomicBool = AtomicBool::new(false);
static PASS: AtomicBool = AtomicBool::new(false);

/// `hidden`: show the cover. `parent_in_control`: let remote input through it.
pub fn update(hidden: bool, parent_in_control: bool) {
    PASS.store(parent_in_control, Ordering::SeqCst);
    let was = WANT.swap(hidden, Ordering::SeqCst);
    if hidden != was { println!("[screen] {}", if hidden { "hidden from him" } else { "shown again" }); }
    #[cfg(windows)]
    win::ensure_thread();
}

#[cfg(windows)]
mod win {
    use super::{PASS, WANT};
    use std::sync::{atomic::Ordering, Once};
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const BG: u32 = 0x00120d0b;      // #0b0d12
    const SHIELD: u32 = 0x00ff7c4f;  // #4f7cff
    const SHIELD2: u32 = 0x00e85323; // #2353e8
    const TEXT: u32 = 0x00f4e8e4;    // #e4e8f4
    const MUTED: u32 = 0x00a68981;   // #8189a6
    const WHITE: u32 = 0x00ffffff;

    static START: Once = Once::new();
    pub fn ensure_thread() { START.call_once(|| { std::thread::spawn(run); }); }

    fn wide(s: &str) -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() }

    unsafe extern "system" fn collect(_m: HMONITOR, _dc: HDC, r: *mut RECT, data: LPARAM) -> BOOL {
        (*(data as *mut Vec<RECT>)).push(*r);
        1
    }
    fn monitors() -> Vec<RECT> {
        let mut v: Vec<RECT> = Vec::new();
        unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect), &mut v as *mut _ as LPARAM); }
        v
    }

    fn run() {
        unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let hinst = GetModuleHandleW(std::ptr::null());
            let class = wide("GuardianCover");
            let wc = WNDCLASSW {
                style: 0, lpfnWndProc: Some(proc_), cbClsExtra: 0, cbWndExtra: 0, hInstance: hinst,
                hIcon: std::ptr::null_mut(), hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(), lpszMenuName: std::ptr::null(), lpszClassName: class.as_ptr(),
            };
            RegisterClassW(&wc);

            let mut wins: Vec<HWND> = Vec::new();
            let mut shown_rects: Vec<(i32, i32, i32, i32)> = Vec::new();
            let mut pass_now = false;
            loop {
                let want = WANT.load(Ordering::SeqCst);
                let pass = PASS.load(Ordering::SeqCst);
                let rects: Vec<(i32, i32, i32, i32)> = if want { monitors().iter().map(|r| (r.left, r.top, r.right - r.left, r.bottom - r.top)).collect() } else { vec![] };

                // (Re)build when switched on/off, monitors change, or pass-through changes.
                if rects != shown_rects || (want && pass != pass_now) {
                    for w in wins.drain(..) { DestroyWindow(w); }
                    for &(x, y, w, h) in &rects {
                        let mut ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
                        if pass { ex |= WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE; }
                        let hwnd = CreateWindowExW(ex, class.as_ptr(), wide("Guardian").as_ptr(), WS_POPUP,
                            x, y, w, h, std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null());
                        if hwnd.is_null() { continue; }
                        if pass { SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA); }
                        // Parents' screen view skips this window and shows the real desktop.
                        SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
                        ShowWindow(hwnd, if pass { SW_SHOWNOACTIVATE } else { SW_SHOW });
                        wins.push(hwnd);
                    }
                    shown_rects = rects;
                    pass_now = pass;
                }
                // Stay on top, and keep his keyboard on the cover (not the apps under it).
                for &w in &wins {
                    SetWindowPos(w, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
                if !pass_now { if let Some(&w) = wins.first() { if GetForegroundWindow() != w && !wins.contains(&GetForegroundWindow()) { SetForegroundWindow(w); } } }

                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                std::thread::sleep(std::time::Duration::from_millis(if wins.is_empty() { 400 } else { 150 }));
            }
        }
    }

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_PAINT => { paint(hwnd); 0 }
            WM_ERASEBKGND => 1,
            WM_CLOSE => 0,                    // Alt+F4 does nothing
            WM_SYSCOMMAND if (wp & 0xFFF0) == SC_CLOSE as usize => 0,
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    /// Points along a cubic curve, for the shield's rounded bottom.
    fn bezier(out: &mut Vec<POINT>, p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), f: &dyn Fn(f32, f32) -> POINT) {
        for i in 1..=16 {
            let t = i as f32 / 16.0; let u = 1.0 - t;
            let x = u*u*u*p0.0 + 3.0*u*u*t*p1.0 + 3.0*u*t*t*p2.0 + t*t*t*p3.0;
            let y = u*u*u*p0.1 + 3.0*u*u*t*p1.1 + 3.0*u*t*t*p2.1 + t*t*t*p3.1;
            out.push(f(x, y));
        }
    }

    unsafe fn paint(hwnd: HWND) {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(hwnd, &mut ps);
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut rc);
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);

        let bg = CreateSolidBrush(BG);
        FillRect(dc, &rc, bg);
        DeleteObject(bg as HGDIOBJ);

        // Shield, drawn from the same shape as the app icon (a 32x32 design).
        let size = (w.min(h) as f32 * 0.20).max(80.0);
        let k = size / 32.0;
        let (ox, oy) = (w as f32 / 2.0 - 16.0 * k, h as f32 / 2.0 - size * 0.85);
        let at = move |x: f32, y: f32| POINT { x: (ox + x * k) as i32, y: (oy + y * k) as i32 };
        let mut pts = vec![at(16.0, 2.0), at(28.0, 7.0), at(28.0, 16.0)];
        bezier(&mut pts, (28.0, 16.0), (28.0, 22.5), (22.5, 27.5), (16.0, 30.0), &at);
        bezier(&mut pts, (16.0, 30.0), (9.5, 27.5), (4.0, 22.5), (4.0, 16.0), &at);
        pts.push(at(4.0, 7.0));
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        let shield = CreateSolidBrush(SHIELD);
        let old_brush = SelectObject(dc, shield as HGDIOBJ);
        Polygon(dc, pts.as_ptr(), pts.len() as i32);

        // Padlock: white body, shackle, keyhole.
        let white = CreateSolidBrush(WHITE);
        SelectObject(dc, white as HGDIOBJ);
        let a = at(11.5, 16.0); let b = at(20.5, 24.0);
        RoundRect(dc, a.x, a.y, b.x, b.y, (3.0 * k) as i32, (3.0 * k) as i32);
        let pen = CreatePen(PS_SOLID, (1.7 * k).max(2.0) as i32, WHITE);
        SelectObject(dc, pen as HGDIOBJ);
        let mut arc = vec![at(13.0, 16.0), at(13.0, 13.0)];
        for i in 0..=12 { let t = std::f32::consts::PI * (i as f32 / 12.0); arc.push(at(16.0 - 3.0 * t.cos(), 13.0 - 2.4 * t.sin())); }
        arc.push(at(19.0, 16.0));
        Polyline(dc, arc.as_ptr(), arc.len() as i32);
        SelectObject(dc, GetStockObject(NULL_PEN));
        let hole = CreateSolidBrush(SHIELD2);
        SelectObject(dc, hole as HGDIOBJ);
        let c = at(16.0, 19.5); let r = (1.3 * k) as i32;
        Ellipse(dc, c.x - r, c.y - r, c.x + r, c.y + r);
        let s0 = at(15.1, 19.5); let s1 = at(16.9, 22.2);
        RoundRect(dc, s0.x, s0.y, s1.x, s1.y, r, r);

        SelectObject(dc, old_brush); SelectObject(dc, old_pen);
        for o in [shield, white, hole] { DeleteObject(o as HGDIOBJ); }
        DeleteObject(pen as HGDIOBJ);

        // "Guardian" and a line underneath.
        SetBkMode(dc, TRANSPARENT as i32);
        let text = |s: &str, height: i32, weight: u32, color: u32, top: i32| {
            let face = wide("Segoe UI Variable Display");
            let font = CreateFontW(-height, 0, 0, 0, weight as i32, 0, 0, 0, DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32,
                CLIP_DEFAULT_PRECIS as u32, CLEARTYPE_QUALITY as u32, DEFAULT_PITCH as u32, face.as_ptr());
            let old = SelectObject(dc, font as HGDIOBJ);
            SetTextColor(dc, color);
            let mut r = RECT { left: 0, top, right: w, bottom: top + height * 2 };
            let t: Vec<u16> = s.encode_utf16().collect();
            DrawTextW(dc, t.as_ptr(), t.len() as i32, &mut r, DT_CENTER | DT_SINGLELINE | DT_VCENTER);
            SelectObject(dc, old); DeleteObject(font as HGDIOBJ);
        };
        let title_h = (size * 0.40) as i32;
        let top = (oy + 32.0 * k + size * 0.12) as i32;
        text("Guardian", title_h, FW_BOLD, TEXT, top);
        text("Your screen is paused by a parent", (size * 0.13).max(16.0) as i32, FW_NORMAL, MUTED, top + title_h * 2);

        EndPaint(hwnd, &ps);
    }
}
