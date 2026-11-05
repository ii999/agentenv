use std::io;
use std::sync::Arc;

use eframe::CreationContext;
use egui_winit::winit::{raw_window_handle::HasDisplayHandle, window::Window};

use super::Failure;

#[cfg(target_os = "macos")]
pub(super) fn preflight() -> Result<(), Failure> {
    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SessionGetInfo(session: u32, actual_session: *mut u32, attributes: *mut u32) -> i32;
    }
    let mut session = 0;
    let mut attributes = 0;
    // AuthSession.h: callerSecuritySession is -1 and sessionHasGraphicAccess
    // is 0x10. Check before asking AppKit to connect to the WindowServer.
    let status = unsafe { SessionGetInfo(u32::MAX, &mut session, &mut attributes) };
    if status == 0 && attributes & 0x10 != 0 {
        Ok(())
    } else {
        Err(Failure::DesktopUnavailable)
    }
}

#[cfg(target_os = "linux")]
pub(super) fn preflight() -> Result<(), Failure> {
    let backend =
        linux_backend(|name| std::env::var(name).ok()).ok_or(Failure::DesktopUnavailable)?;
    // Winit's X11 keyboard accessor otherwise uses expect after dlopen fails.
    if xkbcommon_dl::xkbcommon_option().is_none()
        || (backend == LinuxBackend::X11 && xkbcommon_dl::x11::xkbcommon_x11_option().is_none())
    {
        return Err(Failure::KeyboardUnavailable);
    }
    Ok(())
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinuxBackend {
    Wayland,
    X11,
}

/// Match winit's backend selection before checking backend-specific libraries.
#[cfg(any(target_os = "linux", test))]
fn linux_backend(environment: impl Fn(&str) -> Option<String>) -> Option<LinuxBackend> {
    let present = |name| environment(name).is_some_and(|value| !value.is_empty());
    if present("WAYLAND_DISPLAY") || present("WAYLAND_SOCKET") {
        Some(LinuxBackend::Wayland)
    } else if present("DISPLAY") {
        Some(LinuxBackend::X11)
    } else {
        None
    }
}

#[cfg(windows)]
pub(super) fn preflight() -> Result<(), Failure> {
    use windows_sys::Win32::System::StationsAndDesktops::{
        GetProcessWindowStation, GetUserObjectInformationW, UOI_FLAGS, USEROBJECTFLAGS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::WSF_VISIBLE;
    let station = unsafe { GetProcessWindowStation() };
    if station.is_null() {
        return Err(Failure::DesktopUnavailable);
    }
    let mut flags = USEROBJECTFLAGS::default();
    let mut needed = 0;
    let ok = unsafe {
        GetUserObjectInformationW(
            station,
            UOI_FLAGS,
            (&mut flags as *mut USEROBJECTFLAGS).cast(),
            std::mem::size_of::<USEROBJECTFLAGS>() as u32,
            &mut needed,
        )
    };
    if ok != 0 && flags.dwFlags & WSF_VISIBLE as u32 != 0 {
        Ok(())
    } else {
        Err(Failure::DesktopUnavailable)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub(super) fn preflight() -> Result<(), Failure> {
    Err(Failure::DesktopUnavailable)
}

/// Places a system glass material behind the transparent egui surface:
/// Liquid Glass (`NSGlassEffectView`) on macOS 26 and later, the
/// under-window vibrancy material on earlier releases. Returns whether a
/// backdrop is in place; without one the dialog paints an opaque background.
#[cfg(target_os = "macos")]
pub(super) fn install_backdrop(creation: &CreationContext<'_>) -> bool {
    use egui_winit::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use objc2::{rc::Retained, runtime::AnyClass, MainThreadMarker};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSGlassEffectView, NSGlassEffectViewStyle, NSView,
        NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
        NSVisualEffectView, NSWindowOrderingMode,
    };

    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let Some(handle) = creation
        .winit_window()
        .and_then(|window| window.window_handle().ok())
    else {
        return false;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return false;
    };
    // SAFETY: winit's AppKit handle is the window's live content view, and
    // this runs on the main thread while the window exists.
    let content: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    // The content view belongs to winit, so the backdrop becomes its sibling
    // in the window frame view, ordered below it.
    let Some(frame) = (unsafe { content.superview() }) else {
        return false;
    };
    let bounds = frame.bounds();
    let backdrop: Retained<NSView> = if AnyClass::get(c"NSGlassEffectView").is_some() {
        let glass = NSGlassEffectView::initWithFrame(main_thread.alloc(), bounds);
        glass.setStyle(NSGlassEffectViewStyle::Regular);
        glass.setCornerRadius(f64::from(super::theme::WINDOW_CORNER_RADIUS));
        Retained::into_super(glass)
    } else {
        let effect = NSVisualEffectView::initWithFrame(main_thread.alloc(), bounds);
        effect.setMaterial(NSVisualEffectMaterial::UnderWindowBackground);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        Retained::into_super(effect)
    };
    backdrop.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    frame.addSubview_positioned_relativeTo(&backdrop, NSWindowOrderingMode::Below, Some(content));
    true
}

#[cfg(not(target_os = "macos"))]
pub(super) fn install_backdrop(_creation: &CreationContext<'_>) -> bool {
    false
}

/// The clipboard's Wayland connection borrows the window's display. Fields
/// drop in declaration order, so the clipboard is destroyed before its owner.
pub(super) struct Clipboard {
    clipboard: egui_winit::clipboard::Clipboard,
    _window: Arc<Window>,
}

impl Clipboard {
    pub fn new(creation: &CreationContext<'_>) -> io::Result<Self> {
        let window = creation
            .winit_window()
            .cloned()
            .ok_or_else(|| io::Error::other("the credential window is unavailable"))?;
        let display = window
            .display_handle()
            .map_err(|_| io::Error::other("the desktop display is unavailable"))?
            .as_raw();
        // Keeping the Window alive keeps its HasDisplayHandle valid until the
        // clipboard is dropped. No clipboard contents are read at startup.
        let clipboard = unsafe { egui_winit::clipboard::Clipboard::new(Some(display)) };
        Ok(Self {
            clipboard,
            _window: window,
        })
    }

    pub fn text(&mut self) -> Option<String> {
        self.clipboard.get().filter(|text| !text.is_empty())
    }
    pub fn primary(&mut self) -> Option<String> {
        self.clipboard
            .get_primary_text()
            .filter(|text| !text.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_selection_accepts_inherited_wayland_sockets_and_prefers_wayland_over_x11() {
        for (values, expected) in [
            (vec![("WAYLAND_SOCKET", "3")], Some(LinuxBackend::Wayland)),
            (
                vec![("WAYLAND_SOCKET", "3"), ("DISPLAY", ":0")],
                Some(LinuxBackend::Wayland),
            ),
            (
                vec![("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")],
                Some(LinuxBackend::Wayland),
            ),
            (
                vec![
                    ("WAYLAND_DISPLAY", ""),
                    ("WAYLAND_SOCKET", ""),
                    ("DISPLAY", ":0"),
                ],
                Some(LinuxBackend::X11),
            ),
            (vec![("DISPLAY", "")], None),
            (vec![], None),
        ] {
            let selected = linux_backend(|name| {
                values
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).into())
            });
            assert_eq!(selected, expected);
        }
    }
}
