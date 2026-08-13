//! The handful of `libxapp` calls we need, opened at runtime.
//!
//! Mint's own tray apps (`mintUpdate`, `mintreport-tray`) register with the
//! panel through `XAppStatusIcon`. Unlike the generic StatusNotifierItem
//! protocol, it lets an icon own the menu shown on a LEFT click
//! (`set_primary_menu`) — which is the whole reason this shell exists, because
//! Cinnamon ignores the StatusNotifierItem flag that is supposed to do the
//! same thing and calls `Activate` regardless.
//!
//! Opened with `dlopen` rather than linked: `libxapp1` is installed by default
//! on Mint but is not everywhere, and linking it would put it in the .deb's
//! Depends and make Kintrinsic uninstallable on plain Debian. Missing library
//! simply means "this is not a Mint desktop after all" and the caller falls
//! back to the generic tray.

use gtk::glib::translate::ToGlibPtr;
use libloading::{Library, Symbol};
use std::ffi::{c_char, c_int, c_void, CString};

/// The versioned soname, never the `.so` devel symlink — that one only exists
/// when the -dev package is installed, which on a ward's laptop it is not.
const SONAME: &str = "libxapp.so.1";

type NewFn = unsafe extern "C" fn() -> *mut c_void;
type SetStrFn = unsafe extern "C" fn(*mut c_void, *const c_char);
type SetMenuFn = unsafe extern "C" fn(*mut c_void, *mut c_void);
type SetBoolFn = unsafe extern "C" fn(*mut c_void, c_int);

/// A live Mint status icon. Dropping it drops the library too, which is why the
/// caller keeps it for the life of the process.
pub struct StatusIcon {
    // Field order matters: `icon` is freed by GTK, but `lib` must outlive every
    // symbol we hold, so it is declared last and dropped last.
    icon: *mut c_void,
    set_icon_name: RawSym<SetStrFn>,
    set_tooltip_text: RawSym<SetStrFn>,
    set_label: RawSym<SetStrFn>,
    set_primary_menu: RawSym<SetMenuFn>,
    set_secondary_menu: RawSym<SetMenuFn>,
    set_visible: RawSym<SetBoolFn>,
    _lib: Library,
}

/// A symbol lifted out of the `Library` borrow. Safe because we keep the
/// `Library` alive in the same struct for exactly as long.
struct RawSym<T>(T);

impl StatusIcon {
    /// Open libxapp and create the icon. `Err` means "not a Mint panel" as far
    /// as the caller is concerned — it is never fatal.
    pub fn new(name: &str) -> Result<Self, String> {
        // SAFETY: we load a well-known system library by soname and immediately
        // resolve a fixed set of symbols with the signatures libxapp documents.
        // Any missing symbol is an error, not undefined behaviour.
        unsafe {
            let lib = Library::new(SONAME).map_err(|e| format!("{SONAME}: {e}"))?;
            let sym = |n: &[u8]| -> Result<*const c_void, String> {
                let s: Symbol<*const c_void> = lib
                    .get(n)
                    .map_err(|e| format!("{}: {e}", String::from_utf8_lossy(n)))?;
                Ok(*s)
            };
            let new: NewFn = std::mem::transmute(sym(b"xapp_status_icon_new\0")?);
            let set_name: SetStrFn = std::mem::transmute(sym(b"xapp_status_icon_set_name\0")?);
            let set_icon_name: SetStrFn =
                std::mem::transmute(sym(b"xapp_status_icon_set_icon_name\0")?);
            let set_tooltip_text: SetStrFn =
                std::mem::transmute(sym(b"xapp_status_icon_set_tooltip_text\0")?);
            let set_label: SetStrFn = std::mem::transmute(sym(b"xapp_status_icon_set_label\0")?);
            let set_primary_menu: SetMenuFn =
                std::mem::transmute(sym(b"xapp_status_icon_set_primary_menu\0")?);
            let set_secondary_menu: SetMenuFn =
                std::mem::transmute(sym(b"xapp_status_icon_set_secondary_menu\0")?);
            let set_visible: SetBoolFn =
                std::mem::transmute(sym(b"xapp_status_icon_set_visible\0")?);

            let icon = new();
            if icon.is_null() {
                return Err("xapp_status_icon_new returned nothing".into());
            }
            // The name the panel sorts and logs by, not anything a ward reads.
            let c = CString::new(name).map_err(|e| e.to_string())?;
            set_name(icon, c.as_ptr());

            Ok(Self {
                icon,
                set_icon_name: RawSym(set_icon_name),
                set_tooltip_text: RawSym(set_tooltip_text),
                set_label: RawSym(set_label),
                set_primary_menu: RawSym(set_primary_menu),
                set_secondary_menu: RawSym(set_secondary_menu),
                set_visible: RawSym(set_visible),
                _lib: lib,
            })
        }
    }

    pub fn set_icon_name(&self, name: &str) {
        self.with_str(self.set_icon_name.0, name);
    }

    pub fn set_tooltip_text(&self, text: &str) {
        self.with_str(self.set_tooltip_text.0, text);
    }

    /// The text beside the icon in the panel. Empty string hides it.
    pub fn set_label(&self, text: &str) {
        self.with_str(self.set_label.0, text);
    }

    /// The menu a LEFT click opens — the reason this shell exists.
    pub fn set_primary_menu(&self, menu: &gtk::Menu) {
        self.with_menu(self.set_primary_menu.0, menu);
    }

    /// The menu a RIGHT click opens. Given the same menu, so both gestures
    /// land in the same place — as they do for sound and battery.
    pub fn set_secondary_menu(&self, menu: &gtk::Menu) {
        self.with_menu(self.set_secondary_menu.0, menu);
    }

    pub fn set_visible(&self, visible: bool) {
        // SAFETY: `icon` came from xapp_status_icon_new and is still owned here.
        unsafe { (self.set_visible.0)(self.icon, visible as c_int) }
    }

    fn with_str(&self, f: SetStrFn, s: &str) {
        // A NUL in the middle can only come from data we built ourselves; drop
        // the update rather than panic in a ward's tray.
        let Ok(c) = CString::new(s) else { return };
        // SAFETY: `icon` is live and `c` outlives the call (libxapp copies).
        unsafe { f(self.icon, c.as_ptr()) }
    }

    fn with_menu(&self, f: SetMenuFn, menu: &gtk::Menu) {
        let ptr: *mut gtk::ffi::GtkMenu = menu.to_glib_none().0;
        // SAFETY: `icon` is live; libxapp takes its own reference on the menu,
        // and the caller keeps the Rust `gtk::Menu` alive besides.
        unsafe { f(self.icon, ptr.cast()) }
    }
}
