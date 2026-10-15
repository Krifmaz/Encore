//! macOS only: the dock menu (right-click or long-press on the Encore icon in the dock) with what's playing,
//! Play / Pause, Next, Previous and the mini player. AppKit asks the application delegate for it
//! (`applicationDockMenu:`); Tauri's delegate (from tao) doesn't answer that, so we add the method to its class
//! at run time. Menu clicks go to a small target object of our own. Main thread only.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Sel};
use objc2::{define_class, msg_send, sel, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSMenu, NSMenuItem};
use objc2_foundation::{NSObject, NSString};
use std::cell::RefCell;
use std::sync::OnceLock;

/// What a click does: "toggle" / "next" / "prev" / "mini" (set by desktop.rs).
static ACTION: OnceLock<Box<dyn Fn(&'static str) + Send + Sync>> = OnceLock::new();

fn run(action: &'static str) {
    if let Some(f) = ACTION.get() {
        f(action);
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "EncoreDockTarget"]
    struct DockTarget;

    impl DockTarget {
        #[unsafe(method(encoreToggle:))]
        fn toggle(&self, _sender: Option<&AnyObject>) {
            run("toggle");
        }

        #[unsafe(method(encoreNext:))]
        fn next(&self, _sender: Option<&AnyObject>) {
            run("next");
        }

        #[unsafe(method(encorePrev:))]
        fn prev(&self, _sender: Option<&AnyObject>) {
            run("prev");
        }

        #[unsafe(method(encoreMini:))]
        fn mini(&self, _sender: Option<&AnyObject>) {
            run("mini");
        }
    }
);

struct Dock {
    menu: Retained<NSMenu>,
    now: Retained<NSMenuItem>,
    toggle: Retained<NSMenuItem>,
    _target: Retained<DockTarget>,
}

thread_local! {
    static DOCK: RefCell<Option<Dock>> = const { RefCell::new(None) };
}

fn item(mtm: MainThreadMarker, title: &str, action: Option<Sel>, target: &DockTarget) -> Retained<NSMenuItem> {
    let it = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(mtm.alloc(), &NSString::from_str(title), action, &NSString::from_str(""))
    };
    if action.is_some() {
        unsafe { it.setTarget(Some(target)) };
    } else {
        it.setEnabled(false);
    }
    it
}

/// `applicationDockMenu:` – our menu (AppKit doesn't take ownership; we keep it alive in DOCK).
unsafe extern "C-unwind" fn dock_menu(_this: *mut AnyObject, _cmd: Sel, _sender: *mut AnyObject) -> *mut NSMenu {
    DOCK.with(|d| d.borrow().as_ref().map(|d| Retained::as_ptr(&d.menu) as *mut NSMenu).unwrap_or(std::ptr::null_mut()))
}

/// Build the menu and teach the app delegate to hand it out. Call once, on the main thread, after Tauri set up.
pub fn install(on_click: Box<dyn Fn(&'static str) + Send + Sync>) {
    let _ = ACTION.set(on_click);
    let Some(mtm) = MainThreadMarker::new() else { return };
    let target: Retained<DockTarget> = unsafe { msg_send![mtm.alloc::<DockTarget>(), init] };
    let menu = NSMenu::new(mtm);
    let now = item(mtm, "Encore", None, &target);
    let toggle = item(mtm, "Play", Some(sel!(encoreToggle:)), &target);
    menu.addItem(&now);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&toggle);
    menu.addItem(&item(mtm, "Next", Some(sel!(encoreNext:)), &target));
    menu.addItem(&item(mtm, "Previous", Some(sel!(encorePrev:)), &target));
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    menu.addItem(&item(mtm, "Mini player", Some(sel!(encoreMini:)), &target));
    DOCK.with(|d| *d.borrow_mut() = Some(Dock { menu, now, toggle, _target: target }));

    let app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = app.delegate() else { return };
    let obj: &AnyObject = (*delegate).as_ref();
    let cls: &AnyClass = obj.class();
    let imp: unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> *mut NSMenu = dock_menu;
    // "@@:@": returns an object, takes self, _cmd and the sender. class_addMethod leaves an existing method alone.
    unsafe {
        objc2::ffi::class_addMethod(
            cls as *const AnyClass as *mut AnyClass,
            sel!(applicationDockMenu:),
            std::mem::transmute::<unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject) -> *mut NSMenu, objc2::runtime::Imp>(imp),
            c"@@:@".as_ptr(),
        );
    }
}

/// What's playing ("" = nothing) and whether it plays.
pub fn update(line: &str, playing: bool) {
    DOCK.with(|d| {
        if let Some(d) = d.borrow().as_ref() {
            let text: String = if line.is_empty() { "Encore".into() } else { line.chars().take(60).collect() };
            d.now.setTitle(&NSString::from_str(&text));
            d.toggle.setTitle(&NSString::from_str(if playing { "Pause" } else { "Play" }));
        }
    });
}
