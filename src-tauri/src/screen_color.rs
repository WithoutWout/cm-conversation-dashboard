//! Picking a colour from anywhere on the screen — the pipette beside every
//! colour in Insights settings.
//!
//! Only macOS needs this. On Windows the renderer is WebView2, which is
//! Chromium and ships the `EyeDropper` API, so the renderer samples the screen
//! itself and never calls in here. WKWebView has no `EyeDropper`, so on macOS
//! the renderer asks for AppKit's `NSColorSampler` instead: the same magnifier
//! loupe the system Colours panel uses. It needs no Screen Recording
//! permission — the sampling is done by the system, not by this process.
//!
//! **The sampler can outlive its pick.** On macOS 27.0 a pick made from this app
//! delivered its colour and then left the system's `ColorSampler.xpc` overlay on
//! screen — a full-screen window that swallowed every click, hover and scroll on
//! the Mac, and stayed after this app quit, until that helper was killed.
//! TextEdit's pipette on the same machine closes cleanly; the cause was not
//! found. So after every pick `rescue_stuck_overlay` looks for sampler windows
//! still on screen and, if there are any, ends the helper that owns them — the
//! step that gave the mouse back by hand. launchd starts it again on the next
//! pick. See docs/insights.md → "Insights settings".

/// Ok(Some("#rrggbb")) for a picked colour, Ok(None) when the user pressed Esc
/// or a session was already open.
#[cfg(target_os = "macos")]
pub async fn pick(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2_app_kit::{NSColor, NSColorSampler};
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};

    thread_local! {
        // The open session's sampler, held until its handler has run. AppKit
        // says it retains the sampler itself; Chromium's pipette holds it
        // anyway, and so does this. It doubles as "one session at a time": a
        // second click while one is open starts nothing.
        static ACTIVE: RefCell<Option<Retained<NSColorSampler>>> = const { RefCell::new(None) };
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    // The handler is an `Fn` block, and a oneshot sender is spent by sending:
    // the Option is how an `Fn` gives it away exactly once.
    let tx = Arc::new(Mutex::new(Some(tx)));
    let answer = move |hex: Option<String>| {
        if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(hex);
        }
    };
    // AppKit UI belongs on the main thread; the handler is called there too.
    app.run_on_main_thread(move || {
        if ACTIVE.with(|a: &RefCell<Option<Retained<NSColorSampler>>>| a.borrow().is_some()) {
            answer(None);
            return;
        }
        let sampler = NSColorSampler::new();
        ACTIVE.with(|a: &RefCell<Option<Retained<NSColorSampler>>>| {
            *a.borrow_mut() = Some(sampler.clone())
        });
        let done = answer.clone();
        let handler = RcBlock::new(move |color: *mut NSColor| {
            // SAFETY: AppKit passes either nil or a valid NSColor for the
            // duration of the call.
            let hex = unsafe { color.as_ref() }.and_then(to_hex);
            done(hex);
            // The session is over. AppKit holds its own reference for the
            // length of this call, so letting ours go here is safe.
            ACTIVE.with(|a: &RefCell<Option<Retained<NSColorSampler>>>| a.borrow_mut().take());
            std::thread::spawn(rescue_stuck_overlay);
        });
        // SAFETY: the block has the signature AppKit documents, and the
        // sampler is held in ACTIVE until the session ends.
        unsafe { sampler.showSamplerWithSelectionHandler(&handler) };
    })
    .map_err(|e| e.to_string())?;
    rx.await
        .map_err(|_| "The colour sampler closed without an answer".to_string())
}

#[cfg(not(target_os = "macos"))]
pub async fn pick(_app: tauri::AppHandle) -> Result<Option<String>, String> {
    Err("unsupported".into())
}

/// The name the system's sampler helper runs under, and owns its windows as.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SAMPLER_OWNER: &str = "ColorSampler";

/// Once the pick has been answered, the sampler has no business on screen. Give
/// it a moment to close normally, then end whatever sampler windows are left.
/// Looked at twice, because a slow close is not a stuck one.
#[cfg(target_os = "macos")]
fn rescue_stuck_overlay() {
    use std::time::Duration;
    std::thread::sleep(Duration::from_millis(600));
    if sampler_window_owners().is_empty() {
        return;
    }
    std::thread::sleep(Duration::from_millis(900));
    let owners = sampler_window_owners();
    if owners.is_empty() {
        return;
    }
    log::warn!(
        "colour sampler still on screen after the pick; ending {SAMPLER_OWNER} (pid {owners:?})"
    );
    for pid in owners {
        let _ = std::process::Command::new("/bin/kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
}

/// The pids owning an on-screen window whose owner is the sampler helper.
#[cfg(target_os = "macos")]
fn sampler_window_owners() -> Vec<i32> {
    let me = std::process::id() as i32;
    let mut pids: Vec<i32> = Vec::new();
    for (owner, pid) in on_screen_window_owners() {
        if is_sampler_window(&owner, pid, me) && !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids
}

/// Every on-screen window's owner name and pid. Owner names are readable
/// without Screen Recording permission; only window titles are withheld.
#[cfg(target_os = "macos")]
fn on_screen_window_owners() -> Vec<(String, i32)> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        copy_window_info, kCGNullWindowID, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerName,
        kCGWindowOwnerPID,
    };

    let Some(list) = copy_window_info(kCGWindowListOptionOnScreenOnly, kCGNullWindowID) else {
        return Vec::new();
    };
    // SAFETY: the keys are CoreGraphics' own constant CFStrings.
    let (name_key, pid_key) = unsafe {
        (
            CFString::wrap_under_get_rule(kCGWindowOwnerName),
            CFString::wrap_under_get_rule(kCGWindowOwnerPID),
        )
    };
    let mut out = Vec::new();
    for item in list.get_all_values() {
        // SAFETY: every element of the window list is a CFDictionary.
        let window: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(item as CFDictionaryRef) };
        let owner = window
            .find(&name_key)
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string());
        let pid = window
            .find(&pid_key)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i32());
        if let (Some(owner), Some(pid)) = (owner, pid) {
            out.push((owner, pid));
        }
    }
    out
}

/// Only the sampler helper's windows, and never this process's own.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_sampler_window(owner: &str, pid: i32, me: i32) -> bool {
    owner == SAMPLER_OWNER && pid > 1 && pid != me
}

/// The sampled colour in sRGB — the space the export's hex values mean. The
/// sampler answers in the display's own space, which on a P3 screen would
/// otherwise write a slightly different hex than the pixel looked.
#[cfg(target_os = "macos")]
fn to_hex(color: &objc2_app_kit::NSColor) -> Option<String> {
    use objc2_app_kit::NSColorSpace;
    let c = color.colorUsingColorSpace(&NSColorSpace::sRGBColorSpace())?;
    Some(hex_of(c.redComponent(), c.greenComponent(), c.blueComponent()))
}

/// Components in 0…1 to `#rrggbb`. Clamped: an extended-range colour can read
/// slightly outside the unit interval after conversion.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn hex_of(r: f64, g: f64, b: f64) -> String {
    let ch = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", ch(r), ch(g), ch(b))
}

#[cfg(test)]
mod tests {
    use super::{hex_of, is_sampler_window};

    #[test]
    fn components_become_a_clamped_hex() {
        assert_eq!(hex_of(1.0, 0.0, 0.5), "#ff0080");
        assert_eq!(hex_of(-0.02, 1.03, 0.0), "#00ff00");
    }

    /// The rescue is only as good as this read: an empty list would make a
    /// stuck overlay look gone. The Window Server always has a window on
    /// screen, under that name in every language (the Dock may not be listed).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_window_list_names_its_owners() {
        let owners = super::on_screen_window_owners();
        assert!(
            owners.iter().any(|(o, pid)| o == "Window Server" && *pid > 1),
            "no Window Server window among {owners:?}"
        );
    }

    #[test]
    fn only_the_sampler_helper_is_ever_ended() {
        assert!(is_sampler_window("ColorSampler", 4807, 100));
        assert!(!is_sampler_window("TextEdit", 4807, 100));
        assert!(!is_sampler_window("ColorSampler", 100, 100), "never this app");
        assert!(!is_sampler_window("ColorSampler", 1, 100), "never launchd");
        assert!(!is_sampler_window("ColorSampler", 0, 100));
    }
}
