use async_channel::{Receiver, Sender};
use block2::RcBlock;
use glam::vec2;
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    sel,
};
use objc2_app_kit::{
    NSAlert, NSApplication, NSApplicationActivationPolicy, NSColor, NSColorPanel, NSColorSpace,
    NSEvent, NSModalResponseCancel, NSModalResponseOK, NSPasteboard, NSPasteboardTypeFileURL,
    NSPopUpMenuWindowLevel, NSSavePanel, NSScreen, NSTrackingArea, NSTrackingAreaOptions, NSView,
    NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType};
use objc2_core_graphics::{
    CGImage, CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess,
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGColorSpaceSRGB, kCGWindowNumber,
};
use objc2_foundation::{
    NSArray, NSError, NSFileManager, NSObject, NSObjectProtocol, NSOperationQueue, NSPoint, NSRect,
    NSSearchPathDirectory, NSSearchPathDomainMask, NSSize, NSString, NSURL,
};
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration,
};
use objc2_uniform_type_identifiers::UTType;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{collections::HashMap, path::PathBuf};

use crate::{
    editor::Screenshot,
    geometry::{Color, Rect},
    raster::{self, PixelAlpha},
};

struct ColorPickerIvars {
    sender: Sender<Color>,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[name = "SnipsyColorPickerTarget"]
    #[thread_kind = MainThreadOnly]
    #[ivars = ColorPickerIvars]
    struct ColorPickerTarget;
    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for ColorPickerTarget {}
    impl ColorPickerTarget {
        #[unsafe(method(changeColor:))]
        fn change_color(&self, panel: &NSColorPanel) {
            if let Some(color) = panel.color().colorUsingColorSpace(&NSColorSpace::sRGBColorSpace()) {
                let _ = self.ivars().sender.try_send(Color::from_rgb(
                    (color.redComponent() * 255.0).round() as u8,
                    (color.greenComponent() * 255.0).round() as u8,
                    (color.blueComponent() * 255.0).round() as u8));
            }
        }
    }
);

pub struct ColorPicker {
    panel: Retained<NSColorPanel>,
    _target: Retained<ColorPickerTarget>,
}
impl Drop for ColorPicker {
    fn drop(&mut self) {
        // SAFETY: Clear the non-retained target before releasing it.
        unsafe {
            self.panel.setTarget(None);
        }
        self.panel.orderOut(None);
    }
}

pub fn pick_color(color: Color) -> (ColorPicker, Receiver<Color>) {
    let mtm = MainThreadMarker::new().expect("color picker opens on the main thread");
    let (sender, receiver) = async_channel::unbounded();
    let allocated = ColorPickerTarget::alloc(mtm).set_ivars(ColorPickerIvars { sender });
    // SAFETY: NSObject init initializes this allocated subclass with its ivars already installed.
    let target: Retained<ColorPickerTarget> = unsafe { msg_send![super(allocated), init] };
    let panel = NSColorPanel::sharedColorPanel(mtm);
    panel.setColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
        ((color.0 >> 16) & 255) as f64 / 255.0,
        ((color.0 >> 8) & 255) as f64 / 255.0,
        (color.0 & 255) as f64 / 255.0,
        1.0,
    ));
    panel.setShowsAlpha(false);
    panel.setContinuous(true);
    panel.setLevel(NSPopUpMenuWindowLevel + 1);
    // SAFETY: Target implements changeColor:, and ColorPicker retains it until the target is cleared.
    unsafe {
        panel.setTarget(Some(&target));
        panel.setAction(Some(sel!(changeColor:)));
    }
    panel.makeKeyAndOrderFront(None);
    return (
        ColorPicker {
            panel,
            _target: target,
        },
        receiver,
    );
}

pub fn initialize_app() {
    let mtm = MainThreadMarker::new().expect("app initializes on the main thread");
    NSApplication::sharedApplication(mtm)
        .setActivationPolicy(NSApplicationActivationPolicy::Accessory);
}

pub fn get_clipboard_paths() -> Vec<PathBuf> {
    // SAFETY: AppKit provides this pasteboard type constant on every supported macOS version.
    let file_url_type = unsafe { NSPasteboardTypeFileURL };
    return NSPasteboard::generalPasteboard()
        .pasteboardItems()
        .into_iter()
        .flat_map(|items| items.to_vec())
        .filter_map(|item| item.stringForType(file_url_type))
        .filter_map(|url| NSURL::URLWithString(&url))
        .filter(|url| url.isFileURL())
        .filter_map(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
        .collect();
}

pub fn get_clipboard_change_count() -> i64 {
    return NSPasteboard::generalPasteboard().changeCount() as i64;
}

pub fn get_app_data_dir() -> Result<PathBuf, String> {
    let base = NSFileManager::defaultManager()
        .URLsForDirectory_inDomains(
            NSSearchPathDirectory::ApplicationSupportDirectory,
            NSSearchPathDomainMask::UserDomainMask,
        )
        .firstObject()
        .ok_or("Application Support directory is unavailable")?;
    let url = base
        .URLByAppendingPathComponent_isDirectory(&NSString::from_str("Snipsy"), true)
        .ok_or("Could not create the Snipsy data path")?;
    let path = url
        .path()
        .map(|path| PathBuf::from(path.to_string()))
        .ok_or("Snipsy data path is not a file URL")?;
    std::fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    return Ok(path);
}

pub fn get_capture_bounds() -> Result<Rect, String> {
    if !CGPreflightScreenCaptureAccess() {
        CGRequestScreenCaptureAccess();
        return Err("Enable Screen Recording for Snipsy (or its terminal) in System Settings, then capture again.".into());
    }
    let mtm = MainThreadMarker::new().expect("capture starts on the main thread");
    let mouse = NSEvent::mouseLocation();
    let screens = NSScreen::screens(mtm);
    let primary = screens.firstObject().ok_or("No display available")?;
    let pointer = vec2(
        mouse.x as f32,
        (primary.frame().size.height - mouse.y) as f32,
    );
    return screens
        .iter()
        .map(|screen| {
            let frame = screen.frame();
            Rect {
                pos: vec2(frame.origin.x as f32, frame.origin.y as f32),
                size: vec2(frame.size.width as f32, frame.size.height as f32),
            }
            .flip_vertically(primary.frame().size.height as f32)
        })
        .find(|bounds| bounds.intersects(pointer))
        .ok_or_else(|| "No display under the pointer".into());
}

fn get_native_view(window: &gpui::Window) -> &NSView {
    let handle = HasWindowHandle::window_handle(window).expect("GPUI window has a native handle");
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        unreachable!("macOS uses AppKit windows");
    };
    // SAFETY: GPUI owns this NSView and keeps it alive for the borrowed window's lifetime.
    return unsafe { handle.ns_view.cast::<NSView>().as_ref() };
}

fn get_native_window(window: &gpui::Window) -> Retained<NSWindow> {
    return get_native_view(window)
        .window()
        .expect("GPUI view is attached to its window");
}

pub fn hide_overlay(window: &gpui::Window) {
    get_native_window(window).orderOut(None);
    let mtm = MainThreadMarker::new().expect("window hides on the main thread");
    NSApplication::sharedApplication(mtm).deactivate();
}

pub fn is_window_under_pointer(window: &gpui::Window) -> bool {
    let target = get_native_window(window);
    let pointer = NSEvent::mouseLocation();
    let mtm = MainThreadMarker::new().expect("window lookup runs on the main thread");
    return NSApplication::sharedApplication(mtm)
        .orderedWindows()
        .iter()
        .find(|window| {
            let frame = window.frame();
            window.isVisible()
                && pointer.x >= frame.origin.x
                && pointer.x < frame.origin.x + frame.size.width
                && pointer.y >= frame.origin.y
                && pointer.y < frame.origin.y + frame.size.height
        })
        .is_some_and(|window| Retained::as_ptr(&window) == Retained::as_ptr(&target));
}

pub fn set_window_click_through(window: &gpui::Window, enabled: bool) {
    get_native_window(window).setIgnoresMouseEvents(enabled);
}

pub fn set_window_visible(window: &gpui::Window, visible: bool) {
    let window = get_native_window(window);
    if visible {
        window.orderFrontRegardless();
    } else {
        window.orderOut(None);
    }
}

pub fn move_window(window: &gpui::Window, x: f32, y: f32) {
    let window = get_native_window(window);
    let origin = window.frame().origin;
    window.setFrameOrigin(NSPoint::new(origin.x + x as f64, origin.y - y as f64));
}

pub fn configure_overlay(window: &gpui::Window, bounds: Rect, resizable: bool) {
    let native = get_native_window(window);
    native.setStyleMask(if resizable {
        NSWindowStyleMask::Borderless | NSWindowStyleMask::Resizable
    } else {
        NSWindowStyleMask::Borderless
    });
    let view = get_native_view(window);
    native.setAcceptsMouseMovedEvents(true);
    native.makeFirstResponder(Some(view));
    let tracking_options = NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect
        | NSTrackingAreaOptions::EnabledDuringMouseDrag;
    if !view.trackingAreas().iter().any(|area| {
        area.options().contains(tracking_options)
            && area.owner().is_some_and(|owner| {
                Retained::as_ptr(&owner).cast::<NSView>() == view as *const NSView
            })
    }) {
        // SAFETY: GPUI's NSView handles mouse events and owns the area for its lifetime; no userInfo is needed.
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
                tracking_options,
                Some(view),
                None,
            )
        };
        view.addTrackingArea(&tracking);
    }
    native.setHasShadow(false);
    native.setHidesOnDeactivate(false);
    native.setLevel(NSPopUpMenuWindowLevel);
    native.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    let mtm = MainThreadMarker::new().expect("window config runs on the main thread");
    let primary = NSScreen::screens(mtm)
        .firstObject()
        .expect("overlay has a display");
    let frame = bounds.flip_vertically(primary.frame().size.height as f32);
    native.setFrame_display(
        NSRect::new(
            NSPoint::new(frame.pos.x as f64, frame.pos.y as f64),
            NSSize::new(bounds.size.x as f64, bounds.size.y as f64),
        ),
        false,
    );
}

pub fn show_error(error: &str) {
    let error = error.to_owned();
    let block = RcBlock::new(move || {
        let mtm = MainThreadMarker::new().expect("dialogs run on the main thread");
        let alert = NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str("Snipsy"));
        alert.setInformativeText(&NSString::from_str(&error));
        alert.window().setLevel(NSPopUpMenuWindowLevel + 1);
        alert.runModal();
    });
    // SAFETY: Foundation copies the block and executes it on the main queue, outside GPUI's current update.
    unsafe {
        NSOperationQueue::mainQueue().addOperationWithBlock(&block);
    }
}

pub fn capture_screen(bounds: Rect, sender: Sender<Result<Screenshot, String>>) {
    let mtm = MainThreadMarker::new().expect("capture starts on the main thread");
    let own_window_ids: Vec<u32> = NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .filter(|window| window.isVisible())
        .filter_map(|window| u32::try_from(window.windowNumber()).ok())
        .collect();
    let callback = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            autoreleasepool(|_| {
                // SAFETY: ScreenCaptureKit keeps callback arguments alive for this invocation.
                let result = unsafe {
                    let Some(content) = content.as_ref() else {
                        let message = error
                            .as_ref()
                            .map(|error| error.localizedDescription().to_string())
                            .unwrap_or_else(|| "Could not enumerate displays".into());
                        let _ = sender.try_send(Err(message));
                        return;
                    };
                    let displays = content.displays();
                    let Some(display) = displays.iter().find(|display| {
                        let frame = display.frame();
                        Rect {
                            pos: vec2(frame.origin.x as f32, frame.origin.y as f32),
                            size: vec2(frame.size.width as f32, frame.size.height as f32),
                        } == bounds
                    }) else {
                        let _ = sender
                            .try_send(Err("Display configuration changed; capture again".into()));
                        return;
                    };
                    let frame = display.frame();
                    let bounds = Rect {
                        pos: vec2(frame.origin.x as f32, frame.origin.y as f32),
                        size: vec2(frame.size.width as f32, frame.size.height as f32),
                    };
                    let excluded_windows: Vec<_> = content
                        .windows()
                        .iter()
                        .filter(|window| {
                            is_own_window(
                                window.windowID(),
                                window.owningApplication().map(|app| app.processID()),
                                &own_window_ids,
                            )
                        })
                        .collect();
                    if own_window_ids.iter().any(|id| {
                        !excluded_windows
                            .iter()
                            .any(|window| window.windowID() == *id)
                    }) {
                        let _ = sender.try_send(Err(
                            "Could not exclude Snipsy's overlay from capture; try again".into(),
                        ));
                        return;
                    }
                    let filter = SCContentFilter::initWithDisplay_excludingWindows(
                        SCContentFilter::alloc(),
                        &display,
                        &NSArray::from_retained_slice(&excluded_windows),
                    );
                    let config = SCStreamConfiguration::new();
                    config.setColorSpaceName(kCGColorSpaceSRGB);
                    let scale = filter.pointPixelScale() as f64;
                    config.setWidth((frame.size.width * scale).round() as usize);
                    config.setHeight((frame.size.height * scale).round() as usize);
                    config.setShowsCursor(false);
                    let order: HashMap<_, _> =
                        CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0)
                            .map(|info| {
                                // CGWindowListCopyWindowInfo returns CFString-keyed dictionaries of CFType values.
                                info.cast_unchecked::<CFDictionary<CFString, CFType>>()
                                    .to_vec()
                                    .into_iter()
                                    .filter_map(|item| {
                                        return item
                                            .get(kCGWindowNumber)?
                                            .downcast::<CFNumber>()
                                            .ok()?
                                            .as_i64();
                                    })
                                    .enumerate()
                                    .map(|(index, id)| (id as u32, index))
                                    .collect()
                            })
                            .unwrap_or_default();
                    let mut windows = content
                        .windows()
                        .iter()
                        .filter_map(|window| {
                            if !window.isOnScreen()
                                || window.windowLayer() != 0
                                || is_own_window(
                                    window.windowID(),
                                    window.owningApplication().map(|app| app.processID()),
                                    &own_window_ids,
                                )
                            {
                                return None;
                            }
                            let frame = window.frame();
                            let min =
                                vec2(frame.origin.x as f32, frame.origin.y as f32).max(bounds.pos);
                            let max = (vec2(frame.origin.x as f32, frame.origin.y as f32)
                                + vec2(frame.size.width as f32, frame.size.height as f32))
                            .min(bounds.pos + bounds.size);
                            return ((max - min).min_element() >= 2.0).then_some((
                                window.windowID(),
                                Rect {
                                    pos: min - bounds.pos,
                                    size: max - min,
                                },
                            ));
                        })
                        .collect::<Vec<_>>();
                    windows.sort_by_key(|(id, _)| order.get(id).copied().unwrap_or(usize::MAX));
                    let windows = windows
                        .into_iter()
                        .map(|(_, rect)| rect)
                        .collect::<Vec<_>>();
                    (filter, config, bounds, windows)
                };
                let (filter, config, bounds, windows) = result;
                let sender = sender.clone();
                let callback = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
                    autoreleasepool(|_| {
                        // SAFETY: Callback pointers are valid until this invocation returns.
                        let result = unsafe {
                            if let Some(image) = image.as_ref() {
                                copy_capture_image(image).map(|image| Screenshot {
                                    image: std::sync::Arc::new(image),
                                    bounds,
                                    windows: windows.clone(),
                                })
                            } else {
                                Err(error
                                    .as_ref()
                                    .map(|error| error.localizedDescription().to_string())
                                    .unwrap_or_else(|| "Screen capture failed".into()))
                            }
                        };
                        let _ = sender.try_send(result);
                    });
                });
                // SAFETY: Framework copies the completion block; filter/config live through this call.
                unsafe {
                    SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                        &filter,
                        &config,
                        Some(&callback),
                    );
                }
            });
        },
    );
    // SAFETY: Framework copies the completion block and invokes it with valid borrowed arguments.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
        true, true, &callback);
    }
}

fn is_own_window(id: u32, owner: Option<i32>, own_ids: &[u32]) -> bool {
    return own_ids.contains(&id) || owner == Some(std::process::id() as i32);
}

#[cfg(test)]
mod tests {
    use super::is_own_window;

    #[test]
    fn overlay_exclusion_does_not_require_application_metadata() {
        assert!(is_own_window(42, None, &[42, 43]));
        assert!(is_own_window(99, Some(std::process::id() as i32), &[42]));
        assert!(!is_own_window(99, None, &[42]));
        assert!(!is_own_window(99, Some(-1), &[42]));
    }
}

pub fn prompt_save_image(suggested_name: &str) -> Receiver<Result<Option<PathBuf>, String>> {
    let (sender, receiver) = async_channel::bounded(1);
    let mtm = MainThreadMarker::new().expect("dialogs open on the main thread");
    let panel = NSSavePanel::savePanel(mtm);
    panel.setNameFieldStringValue(&NSString::from_str(suggested_name));
    panel.setCanCreateDirectories(true);
    panel.setAllowsOtherFileTypes(false);
    panel.setAllowedContentTypes(&NSArray::from_retained_slice(
        &["png", "jpg"]
            .into_iter()
            .filter_map(|extension| {
                UTType::typeWithFilenameExtension(&NSString::from_str(extension))
            })
            .collect::<Vec<_>>(),
    ));
    panel.setLevel(NSPopUpMenuWindowLevel + 1);
    let callback_panel = panel.clone();
    let callback = RcBlock::new(move |response: isize| {
        let result = if response == NSModalResponseOK {
            callback_panel
                .URL()
                .and_then(|url| url.to_file_path())
                .map(Some)
                .ok_or_else(|| "Could not read the destination path".into())
        } else if response == NSModalResponseCancel {
            Ok(None)
        } else {
            Err("Could not open the save dialog".into())
        };
        let _ = sender.try_send(result);
    });
    panel.beginWithCompletionHandler(&callback);
    return receiver;
}

fn copy_capture_image(image: &CGImage) -> Result<image::RgbaImage, String> {
    use objc2_core_graphics::{CGDataProvider, CGImageAlphaInfo, CGImageByteOrderInfo};
    if CGImage::bits_per_component(Some(image)) != 8
        || CGImage::bits_per_pixel(Some(image)) != 32
        || CGImage::byte_order_info(Some(image)) != CGImageByteOrderInfo::Order32Little
    {
        return Err("Unsupported screen image pixel format".into());
    }
    let alpha = match CGImage::alpha_info(Some(image)) {
        CGImageAlphaInfo::PremultipliedFirst => PixelAlpha::Premultiplied,
        CGImageAlphaInfo::First => PixelAlpha::Straight,
        CGImageAlphaInfo::NoneSkipFirst => PixelAlpha::Opaque,
        _ => return Err("Unsupported screen image alpha format".into()),
    };
    let provider =
        CGImage::data_provider(Some(image)).ok_or("Screen image has no pixel provider")?;
    let data = CGDataProvider::data(Some(&provider)).ok_or("Could not read screen image pixels")?;
    // SAFETY: CGDataProviderCopyData returns immutable owned CFData, retained throughout decoding.
    let bytes = unsafe { data.as_bytes_unchecked() };
    return raster::decode_bgra(
        bytes,
        u32::try_from(CGImage::width(Some(image))).map_err(|error| error.to_string())?,
        u32::try_from(CGImage::height(Some(image))).map_err(|error| error.to_string())?,
        CGImage::bytes_per_row(Some(image)),
        alpha,
    );
}
