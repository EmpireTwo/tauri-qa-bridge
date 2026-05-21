use super::ScreenshotError;

/// Minimal description of a layer-0 on-screen window, used to populate the
/// `available_windows` payload returned with `WINDOW_NOT_FOUND` errors so
/// callers can pick the right `window_id` without a second round-trip.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub(crate) struct DiscoveredWindow {
    pub(crate) window_id: u32,
    pub(crate) owner: String,
    pub(crate) title: String,
    pub(crate) layer: i32,
}

/// Logical (in points, not pixels) bounds of a window, used to derive
/// `scale_factor` by comparing against the captured PNG's pixel dimensions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WindowBounds {
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// Walk the on-screen window list and return every layer-0 entry, capped at
/// [`AVAILABLE_WINDOWS_CAP`] to keep `WINDOW_NOT_FOUND` payloads bounded.
///
/// # Errors
///
/// Returns [`ScreenshotError::CaptureFailed`] when `CoreGraphics` returns null.
pub(crate) fn list_layer_zero_windows() -> Result<Vec<DiscoveredWindow>, ScreenshotError> {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        CGWindowListCopyWindowInfo, kCGNullWindowID, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerName,
    };

    let raw = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    if raw.is_null() {
        return Err(ScreenshotError::CaptureFailed {
            message: "CGWindowListCopyWindowInfo returned null".to_owned(),
        });
    }
    let windows: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { TCFType::wrap_under_create_rule(raw) };

    let owner_key = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerName) };
    let title_key = unsafe { CFString::wrap_under_get_rule(kCGWindowName) };
    let layer_key = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    let number_key = unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) };

    let mut out: Vec<DiscoveredWindow> = Vec::new();
    for idx in 0..windows.len() {
        if out.len() >= AVAILABLE_WINDOWS_CAP {
            break;
        }
        let Some(dict) = windows.get(idx) else {
            continue;
        };
        let layer = dict
            .find(&layer_key)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|v| v.to_i32())
            .unwrap_or(-1);
        if layer != 0 {
            continue;
        }
        let number_i64 = dict
            .find(&number_key)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|v| v.to_i64())
            .unwrap_or(0);
        let Ok(window_id) = u32::try_from(number_i64) else {
            continue;
        };
        if window_id == 0 {
            continue;
        }
        let owner = dict
            .find(&owner_key)
            .and_then(|v| v.downcast::<CFString>())
            .map(|v| v.to_string())
            .unwrap_or_default();
        let title = dict
            .find(&title_key)
            .and_then(|v| v.downcast::<CFString>())
            .map(|v| v.to_string())
            .unwrap_or_default();
        out.push(DiscoveredWindow {
            window_id,
            owner,
            title,
            layer,
        });
    }
    Ok(out)
}

/// Look up the logical bounds of a window by `window_id`.
///
/// Used to derive `scale_factor` for the IPC response by comparing pixel
/// dimensions of the captured PNG to these logical (point) dimensions.
///
/// # Errors
///
/// Returns [`ScreenshotError::CaptureFailed`] when `CoreGraphics` returns null
/// or the window list does not contain `window_id`.
pub(crate) fn get_window_bounds(window_id: u32) -> Result<WindowBounds, ScreenshotError> {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::geometry::CGRect;
    use core_graphics::window::{
        CGWindowListCopyWindowInfo, kCGWindowBounds, kCGWindowListExcludeDesktopElements,
        kCGWindowListOptionIncludingWindow, kCGWindowNumber,
    };

    let raw = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionIncludingWindow | kCGWindowListExcludeDesktopElements,
            window_id,
        )
    };
    if raw.is_null() {
        return Err(ScreenshotError::CaptureFailed {
            message: "CGWindowListCopyWindowInfo returned null".to_owned(),
        });
    }
    let windows: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { TCFType::wrap_under_create_rule(raw) };

    let bounds_key = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };
    let number_key = unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) };

    for idx in 0..windows.len() {
        let Some(dict) = windows.get(idx) else {
            continue;
        };
        let entry_id = dict
            .find(&number_key)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|v| v.to_i64())
            .unwrap_or(0);
        if u32::try_from(entry_id).ok() != Some(window_id) {
            continue;
        }
        // CGWindowList stores the bounds rect as a `{Width,Height,X,Y}` dict
        // rather than a CGRect struct; CoreGraphics ships
        // `CGRectMakeWithDictionaryRepresentation` for exactly this case so
        // we don't have to walk the keys by hand.
        if let Some(bounds_dict) = dict
            .find(&bounds_key)
            .and_then(|v| v.downcast::<CFDictionary>())
            && let Some(rect) = CGRect::from_dict_representation(&bounds_dict)
        {
            return Ok(WindowBounds {
                width: rect.size.width,
                height: rect.size.height,
            });
        }
    }
    Err(ScreenshotError::CaptureFailed {
        message: format!("window_id {window_id} has no bounds entry"),
    })
}

/// Hard cap on the `available_windows` list returned in `WINDOW_NOT_FOUND`
/// errors. A host with many open windows can otherwise produce a multi-KB
/// payload; 20 is enough to spot a typo or wrong owner without bloating the
/// JSON-RPC frame.
pub(crate) const AVAILABLE_WINDOWS_CAP: usize = 20;

/// Find a `CGWindowID` by owner name (exact match) and an optional title substring.
///
/// Walks the on-screen window list, skipping desktop elements, and returns the
/// first layer-0 window whose owner equals `owner` and whose title contains
/// `title` (when provided).
///
/// # Errors
///
/// Returns [`ScreenshotError::CaptureFailed`] when the `CoreGraphics` window
/// list call returns null or no layer-0 window matches the filter.
#[allow(
    dead_code,
    reason = "kept for owner-name lookups in a follow-up milestone"
)]
pub(crate) fn get_window_id(owner: &str, title: Option<&str>) -> Result<u32, ScreenshotError> {
    use core_foundation::array::CFArray;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_graphics::window::{
        CGWindowListCopyWindowInfo, kCGNullWindowID, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerName,
    };

    let raw = unsafe {
        CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID,
        )
    };
    if raw.is_null() {
        return Err(ScreenshotError::CaptureFailed {
            message: "CGWindowListCopyWindowInfo returned null".to_owned(),
        });
    }
    let windows: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { TCFType::wrap_under_create_rule(raw) };

    let owner_key = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerName) };
    let title_key = unsafe { CFString::wrap_under_get_rule(kCGWindowName) };
    let layer_key = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    let number_key = unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) };

    for idx in 0..windows.len() {
        let Some(dict) = windows.get(idx) else {
            continue;
        };
        let owner_value = dict
            .find(&owner_key)
            .and_then(|v| v.downcast::<CFString>())
            .map(|v| v.to_string())
            .unwrap_or_default();
        let title_value = dict
            .find(&title_key)
            .and_then(|v| v.downcast::<CFString>())
            .map(|v| v.to_string())
            .unwrap_or_default();
        let layer = dict
            .find(&layer_key)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|v| v.to_i32())
            .unwrap_or(-1);
        let number_i64 = dict
            .find(&number_key)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|v| v.to_i64())
            .unwrap_or(0);

        if owner_value != owner {
            continue;
        }
        if let Some(needle) = title
            && !title_value.contains(needle)
        {
            continue;
        }
        if layer != 0 {
            continue;
        }
        let Ok(number) = u32::try_from(number_i64) else {
            continue;
        };
        if number == 0 {
            continue;
        }
        return Ok(number);
    }
    Err(ScreenshotError::CaptureFailed {
        message: format!("no layer-0 window matched owner={owner}"),
    })
}
