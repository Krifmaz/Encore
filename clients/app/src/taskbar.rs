//! Windows only: the thumbnail toolbar under the taskbar preview of the Encore window (previous, play / pause,
//! next). The buttons are added once the taskbar button exists ("TaskbarButtonCreated", which comes again
//! after Explorer restarts); their clicks arrive as WM_COMMAND / THBN_CLICKED on the window, which we see
//! through a window subclass. Everything here runs on the main (UI) thread. The taskbar progress itself is
//! set through Tauri (`set_progress_bar`, see desktop.rs).

use crate::icons::{rgba, Glyph};
use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{
    DefSubclassProc, ITaskbarList3, SetWindowSubclass, TaskbarList, THBF_ENABLED, THBN_CLICKED, THB_FLAGS, THB_ICON,
    THB_TOOLTIP, THUMBBUTTON,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, RegisterWindowMessageW, HICON, ICONINFO, WM_COMMAND};

const ID_PREV: u32 = 1;
const ID_TOGGLE: u32 = 2;
const ID_NEXT: u32 = 3;
const SUBCLASS_ID: usize = 0x456e_636f; // "Enco"

/// What a click does: the desktop module passes `bridge::control` for the window's app.
static ACTION: OnceLock<Box<dyn Fn(&'static str) + Send + Sync>> = OnceLock::new();

struct Bar {
    hwnd: HWND,
    list: Option<ITaskbarList3>,
    added: bool,
    playing: bool,
    play: HICON,
    pause: HICON,
    prev: HICON,
    next: HICON,
}

/// The "TaskbarButtonCreated" message id, read by `subclass` for every message without touching `BAR`.
static CREATED_MSG: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static BAR: RefCell<Option<Bar>> = const { RefCell::new(None) };
}

/// A 32-bit icon with alpha from straight RGBA pixels.
fn icon(g: Glyph) -> HICON {
    const SIZE: u32 = 16;
    let px = rgba(g, SIZE);
    // GDI wants BGRA, premultiplication isn't needed for icons
    let bgra: Vec<u8> = px.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    let mask = vec![0u8; (SIZE * SIZE / 8) as usize];
    unsafe {
        let color = CreateBitmap(SIZE as i32, SIZE as i32, 1, 32, Some(bgra.as_ptr() as *const c_void));
        let mono = CreateBitmap(SIZE as i32, SIZE as i32, 1, 1, Some(mask.as_ptr() as *const c_void));
        let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mono, hbmColor: color };
        let h = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mono.into());
        h
    }
}

fn tip(text: &str) -> [u16; 260] {
    let mut t = [0u16; 260];
    for (i, c) in text.encode_utf16().take(259).enumerate() {
        t[i] = c;
    }
    t
}

fn button(id: u32, icon: HICON, text: &str) -> THUMBBUTTON {
    THUMBBUTTON {
        dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
        iId: id,
        iBitmap: 0,
        hIcon: icon,
        szTip: tip(text),
        dwFlags: THBF_ENABLED,
    }
}

fn buttons(b: &Bar) -> [THUMBBUTTON; 3] {
    [
        button(ID_PREV, b.prev, "Previous"),
        if b.playing { button(ID_TOGGLE, b.pause, "Pause") } else { button(ID_TOGGLE, b.play, "Play") },
        button(ID_NEXT, b.next, "Next"),
    ]
}

/// Change the bar's state with `update` (which returns whether the buttons need sending), then add or
/// update the buttons (the taskbar button has to exist; when it doesn't yet, TaskbarButtonCreated retries).
/// The COM calls go to Explorer and pump messages while they wait, so `subclass` can run meanwhile: they
/// are made with the RefCell released.
fn sync(update: impl FnOnce(&mut Bar) -> bool) {
    let Some((list, hwnd, btns, added)) = BAR.with(|c| {
        let mut g = c.try_borrow_mut().ok()?;
        let b = g.as_mut()?;
        if !update(b) {
            return None;
        }
        Some((b.list.clone(), b.hwnd, buttons(b), b.added))
    }) else { return };
    let list = match list {
        Some(l) => l,
        None => {
            let Ok(l) = (unsafe { CoCreateInstance::<_, ITaskbarList3>(&TaskbarList, None, CLSCTX_INPROC_SERVER) }) else { return };
            if unsafe { l.HrInit() }.is_err() {
                return;
            }
            BAR.with(|c| if let Ok(mut g) = c.try_borrow_mut() { if let Some(b) = g.as_mut() { b.list = Some(l.clone()); } });
            l
        }
    };
    let ok = if added { unsafe { list.ThumbBarUpdateButtons(hwnd, &btns) }.is_ok() } else { unsafe { list.ThumbBarAddButtons(hwnd, &btns) }.is_ok() };
    BAR.with(|c| if let Ok(mut g) = c.try_borrow_mut() { if let Some(b) = g.as_mut() { b.added = ok; } });
}

unsafe extern "system" fn subclass(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    let created = CREATED_MSG.load(Ordering::Relaxed);
    if created != 0 && msg == created {
        // a new taskbar button (first show, or Explorer restarted): the old buttons are gone with it
        sync(|b| { b.added = false; b.list = None; true });
    } else if msg == WM_COMMAND && ((wparam.0 >> 16) & 0xffff) as u32 == THBN_CLICKED {
        let action = match (wparam.0 & 0xffff) as u32 {
            ID_PREV => Some("prev"),
            ID_TOGGLE => Some("toggle"),
            ID_NEXT => Some("next"),
            _ => None,
        };
        if let (Some(a), Some(f)) = (action, ACTION.get()) {
            f(a);
            return LRESULT(0);
        }
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Hook the window (its raw HWND) and add the buttons. `on_click` gets "prev" / "toggle" / "next".
pub fn install(hwnd: *mut c_void, on_click: Box<dyn Fn(&'static str) + Send + Sync>) {
    let _ = ACTION.set(on_click);
    let hwnd = HWND(hwnd);
    let created_msg = unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) };
    CREATED_MSG.store(created_msg, Ordering::Relaxed);
    let bar = Bar {
        hwnd, list: None, added: false, playing: false,
        play: icon(Glyph::Play), pause: icon(Glyph::Pause), prev: icon(Glyph::Prev), next: icon(Glyph::Next),
    };
    BAR.with(|b| *b.borrow_mut() = Some(bar));
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(subclass), SUBCLASS_ID, 0);
    }
    sync(|_| true);
}

/// Swap the middle button between play and pause.
pub fn set_playing(playing: bool) {
    sync(|b| {
        let send = b.playing != playing || !b.added;
        b.playing = playing;
        send
    });
}
