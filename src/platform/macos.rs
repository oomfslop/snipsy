use crate::{
    geometry::{Color, Rect},
    raster::Screenshot,
};
use anyhow::{Context as _, Result, anyhow, bail, ensure};
use async_channel::{Receiver, Sender};
use block2::RcBlock;
use glam::{Vec2, vec2};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSColor, NSColorPanel, NSColorSpace,
    NSCursor, NSEvent, NSImage, NSImageView, NSModalResponseOK, NSNormalWindowLevel, NSPasteboard,
    NSPasteboardTypeFileURL, NSPopUpMenuWindowLevel, NSPrintOperation, NSSavePanel, NSScreen,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterStatus, AVFileTypeMPEG4, AVMediaTypeVideo,
    AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoHeightKey, AVVideoWidthKey,
};
use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDisplayCopyColorSpace, CGError, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, CGPreflightScreenCaptureAccess,
    CGRequestScreenCaptureAccess, CGWarpMouseCursorPosition, CGWindowListCopyWindowInfo,
    CGWindowListOption, kCGColorSpaceSRGB, kCGWindowNumber,
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSError, NSFileManager, NSNumber, NSObject, NSObjectProtocol,
    NSOperationQueue, NSPoint, NSRect, NSSearchPathDirectory, NSSearchPathDomainMask, NSSize,
    NSString, NSURL,
};
use objc2_screen_capture_kit::{
    SCContentFilter, SCDisplay, SCFrameStatus, SCScreenshotManager, SCShareableContent, SCStream,
    SCStreamConfiguration, SCStreamDelegate, SCStreamFrameInfoStatus, SCStreamOutput,
    SCStreamOutputType, SCWindow,
};
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use objc2_uniform_type_identifiers::{UTType, UTTypeMPEG4Movie};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::Cell,
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// GPUI maps edge resize cursors to `resize*Cursor`, which macOS draws as single-direction
/// arrows; the private window-resize cursors match the diagonal ones GPUI already uses.
pub fn use_window_resize_cursors() {
    let metaclass = NSCursor::class().metaclass();
    for (public, private) in [
        (
            sel!(resizeUpDownCursor),
            sel!(_windowResizeNorthSouthCursor),
        ),
        (
            sel!(resizeLeftRightCursor),
            sel!(_windowResizeEastWestCursor),
        ),
    ] {
        if let (Some(target), Some(source)) = (
            metaclass.instance_method(public),
            metaclass.instance_method(private),
        ) {
            // SAFETY: both are zero-argument NSCursor class methods returning an NSCursor.
            unsafe { target.set_implementation(source.implementation()) };
        }
    }
}

fn get_ns_error(error: *mut NSError, fallback: &str) -> anyhow::Error {
    // SAFETY: AppKit callbacks pass either null or a valid NSError for the duration of the call.
    return unsafe { error.as_ref() }.map_or_else(
        || anyhow!("{fallback}"),
        |error| anyhow!("{}", error.localizedDescription()),
    );
}

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

thread_local! {
    static ACTIVE_COLOR_PICKER_TARGET: Cell<*const AnyObject> = const { Cell::new(std::ptr::null()) };
}

impl Drop for ColorPicker {
    fn drop(&mut self) {
        if !release_color_picker_target(Retained::as_ptr(&self._target).cast()) {
            return;
        }
        // SAFETY: the panel does not retain its target, so clear it before the target is released.
        unsafe { self.panel.setTarget(None) };
        self.panel.orderOut(None);
    }
}

fn release_color_picker_target(target: *const AnyObject) -> bool {
    if ACTIVE_COLOR_PICKER_TARGET.get() != target {
        return false;
    }
    ACTIVE_COLOR_PICKER_TARGET.set(std::ptr::null());
    return true;
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
    // SAFETY: the target implements changeColor: and ColorPicker keeps it alive until the target is cleared.
    unsafe {
        panel.setTarget(Some(&target));
        panel.setAction(Some(sel!(changeColor:)));
    }
    ACTIVE_COLOR_PICKER_TARGET.set(Retained::as_ptr(&target).cast());
    panel.makeKeyAndOrderFront(None);
    return (
        ColorPicker {
            panel,
            _target: target,
        },
        receiver,
    );
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
        .filter_map(|url| url.to_file_path())
        .collect();
}

pub fn get_clipboard_change_count() -> i64 {
    return NSPasteboard::generalPasteboard().changeCount() as i64;
}

pub fn get_app_data_dir() -> Result<PathBuf> {
    let name = if cfg!(debug_assertions) {
        "Snipsy Debug"
    } else {
        "Snipsy"
    };
    return get_user_directory(NSSearchPathDirectory::ApplicationSupportDirectory, name);
}

pub fn get_quick_save_dir() -> Result<PathBuf> {
    return get_user_directory(NSSearchPathDirectory::PicturesDirectory, "Snipsy");
}

fn get_user_directory(directory: NSSearchPathDirectory, name: &str) -> Result<PathBuf> {
    let path = NSFileManager::defaultManager()
        .URLsForDirectory_inDomains(directory, NSSearchPathDomainMask::UserDomainMask)
        .firstObject()
        .and_then(|url| url.to_file_path())
        .context("Requested user directory is unavailable")?
        .join(name);
    std::fs::create_dir_all(&path)?;
    return Ok(path);
}

pub fn get_launches_at_login() -> bool {
    // SAFETY: SMAppService methods have no preconditions beyond running on macOS 13+.
    let status = unsafe { SMAppService::mainAppService().status() };
    return matches!(
        status,
        SMAppServiceStatus::Enabled | SMAppServiceStatus::RequiresApproval
    );
}

pub fn set_launches_at_login(enabled: bool) -> Result<()> {
    // SAFETY: SMAppService methods have no preconditions beyond running on macOS 13+.
    unsafe {
        let service = SMAppService::mainAppService();
        if enabled {
            service.registerAndReturnError()
        } else {
            service.unregisterAndReturnError()
        }
        .map_err(|error| anyhow!("{}", error.localizedDescription()))?;
        if service.status() == SMAppServiceStatus::RequiresApproval {
            SMAppService::openSystemSettingsLoginItems();
        }
    }
    return Ok(());
}

pub fn get_capture_bounds() -> Result<Rect> {
    if !CGPreflightScreenCaptureAccess() {
        CGRequestScreenCaptureAccess();
        bail!(
            "Enable Screen Recording for Snipsy (or its terminal) in System Settings, then capture again."
        );
    }
    return get_desktop_bounds();
}

fn get_primary_screen_height(mtm: MainThreadMarker) -> f32 {
    return NSScreen::screens(mtm)
        .firstObject()
        .expect("a display is connected")
        .frame()
        .size
        .height as f32;
}

fn get_rect(frame: CGRect) -> Rect {
    return Rect {
        pos: vec2(frame.origin.x as f32, frame.origin.y as f32),
        size: vec2(frame.size.width as f32, frame.size.height as f32),
    };
}

pub fn get_desktop_bounds() -> Result<Rect> {
    let mtm = MainThreadMarker::new().expect("capture starts on the main thread");
    let height = get_primary_screen_height(mtm);
    return NSScreen::screens(mtm)
        .iter()
        .map(|screen| get_rect(screen.frame()).flip_vertically(height))
        .reduce(Rect::union)
        .context("No display available");
}

pub fn get_desktop_scale() -> f32 {
    let mtm = MainThreadMarker::new().expect("display scale reads on the main thread");
    return NSScreen::screens(mtm)
        .iter()
        .map(|screen| screen.backingScaleFactor() as f32)
        .fold(1.0, f32::max);
}

fn get_native_view(window: &gpui_kit::Window) -> &NSView {
    let handle = HasWindowHandle::window_handle(window).expect("GPUI window has a native handle");
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        unreachable!("macOS uses AppKit windows");
    };
    // SAFETY: GPUI owns this NSView and keeps it alive for the borrowed window's lifetime.
    return unsafe { handle.ns_view.cast::<NSView>().as_ref() };
}

fn get_native_window(window: &gpui_kit::Window) -> Retained<NSWindow> {
    return get_native_view(window)
        .window()
        .expect("GPUI view is attached to its window");
}

/// App activation brings the key and main windows forward, so the window has to become both
/// before activating; GPUI orders it front asynchronously, after the last focused window
/// has already been raised.
pub fn make_key_window(window: &gpui_kit::Window) {
    let native = get_native_window(window);
    native.makeKeyAndOrderFront(None);
    native.makeMainWindow();
}

pub fn hide_overlay(window: &gpui_kit::Window) {
    get_native_window(window).orderOut(None);
    let mtm = MainThreadMarker::new().expect("window hides on the main thread");
    NSApplication::sharedApplication(mtm).deactivate();
}

pub fn get_pointer_position() -> Vec2 {
    let mtm = MainThreadMarker::new().expect("pointer lookup runs on the main thread");
    let pointer = NSEvent::mouseLocation();
    return vec2(
        pointer.x as f32,
        get_primary_screen_height(mtm) - pointer.y as f32,
    );
}

pub fn is_window_under_pointer(window: &gpui_kit::Window) -> bool {
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

pub fn set_window_click_through(window: &gpui_kit::Window, enabled: bool) {
    get_native_window(window).setIgnoresMouseEvents(enabled);
}

pub fn set_window_visible(window: &gpui_kit::Window, visible: bool) {
    let window = get_native_window(window);
    if visible {
        window.orderFrontRegardless();
    } else {
        window.orderOut(None);
    }
}

pub fn move_window(window: &gpui_kit::Window, delta: Vec2) {
    let window = get_native_window(window);
    let origin = window.frame().origin;
    window.setFrameOrigin(NSPoint::new(
        origin.x + delta.x as f64,
        origin.y - delta.y as f64,
    ));
}

pub fn move_pointer(position: Vec2) -> Result<()> {
    let error = CGWarpMouseCursorPosition(CGPoint::new(position.x as f64, position.y as f64));
    ensure!(
        error == CGError::Success,
        "Could not move pointer: {}",
        error.0
    );
    return Ok(());
}

pub fn configure_overlay(window: &gpui_kit::Window, bounds: Rect, resizable: bool) {
    let native = get_native_window(window);
    native.setStyleMask(if resizable {
        NSWindowStyleMask::Borderless | NSWindowStyleMask::Resizable
    } else {
        NSWindowStyleMask::Borderless
    });
    let view = get_native_view(window);
    // SAFETY: GPUI makes its view layer-backed before the window builder runs.
    let layer: Retained<AnyObject> = unsafe { msg_send![view, layer] };
    set_srgb_colorspace(&layer);
    native.setAcceptsMouseMovedEvents(true);
    native.makeFirstResponder(Some(view));
    let tracking_options = NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect
        | NSTrackingAreaOptions::EnabledDuringMouseDrag;
    if !view.trackingAreas().iter().any(|area| {
        area.options().contains(tracking_options)
            && area
                .owner()
                .is_some_and(|owner| std::ptr::eq(Retained::as_ptr(&owner).cast::<NSView>(), view))
    }) {
        // SAFETY: GPUI's NSView handles mouse events and owns the area for its lifetime; no userInfo is needed.
        let tracking = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
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
    set_native_frame(&native, bounds);
}

fn set_native_frame(native: &NSWindow, bounds: Rect) {
    let mtm = MainThreadMarker::new().expect("window frame changes on the main thread");
    let frame = bounds.flip_vertically(get_primary_screen_height(mtm));
    native.setFrame_display(
        NSRect::new(
            NSPoint::new(frame.pos.x as f64, frame.pos.y as f64),
            NSSize::new(frame.size.x as f64, frame.size.y as f64),
        ),
        true,
    );
}

pub fn set_window_bounds(window: &gpui_kit::Window, bounds: Rect) {
    let native = get_native_window(window);
    let block = RcBlock::new(move || set_native_frame(&native, bounds));
    // SAFETY: Foundation copies the block and executes it on the main queue. GPUI drops resizes that arrive during its current update.
    unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&block) };
}

/// AppKit derives the shadow from the window's drawn content and caches it until invalidated.
pub fn set_window_shadow(window: &gpui_kit::Window, enabled: bool) {
    let native = get_native_window(window);
    native.setHasShadow(enabled);
    native.invalidateShadow();
}

/// GPUI leaves its CAMetalLayer untagged, so macOS shows the sRGB capture in the display's
/// native gamut without color matching.
fn set_srgb_colorspace(layer: &AnyObject) {
    // SAFETY: kCGColorSpaceSRGB is always defined, and CALayer retains the colorspace.
    unsafe {
        let srgb = CGColorSpace::with_name(Some(kCGColorSpaceSRGB));
        let _: () = msg_send![layer, setColorspace: srgb.as_deref()];
    }
}

fn create_alert(mtm: MainThreadMarker, message: &str, informative: &str) -> Retained<NSAlert> {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(message));
    alert.setInformativeText(&NSString::from_str(informative));
    alert.window().setLevel(NSPopUpMenuWindowLevel + 1);
    return alert;
}

pub fn show_error(error: &str) {
    let error = error.to_owned();
    let block = RcBlock::new(move || {
        let mtm = MainThreadMarker::new().expect("dialogs run on the main thread");
        create_alert(mtm, "Snipsy", &error).runModal();
    });
    // SAFETY: Foundation copies the block and executes it on the main queue, outside GPUI's current update.
    unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&block) };
}

pub fn confirm(message: &str, informative: &str, action: &str) -> bool {
    let mtm = MainThreadMarker::new().expect("confirmation opens on the main thread");
    let alert = create_alert(mtm, message, informative);
    alert.addButtonWithTitle(&NSString::from_str(action));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    return alert.runModal() == NSAlertFirstButtonReturn;
}

pub fn print_png(bytes: &[u8]) -> Result<()> {
    let mtm = MainThreadMarker::new().expect("printing starts on the main thread");
    let data = NSData::with_bytes(bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data).context("Could not decode image")?;
    let view = NSImageView::initWithFrame(
        NSImageView::alloc(mtm),
        NSRect::new(NSPoint::ZERO, image.size()),
    );
    view.setImage(Some(&image));
    let operation = NSPrintOperation::printOperationWithView(&view);
    operation.setShowsPrintPanel(true);
    operation.setShowsProgressPanel(true);
    operation.runOperation();
    return Ok(());
}

pub fn prompt_save_image(suggested_name: &str) -> Receiver<Result<Option<PathBuf>>> {
    let types = ["png", "jpg", "webp", "bmp", "tiff", "tga", "ico", "gif"]
        .into_iter()
        .filter_map(|extension| UTType::typeWithFilenameExtension(&NSString::from_str(extension)))
        .collect::<Vec<_>>();
    return prompt_save(suggested_name, &NSArray::from_retained_slice(&types));
}

pub fn prompt_save_recording() -> Receiver<Result<Option<PathBuf>>> {
    // SAFETY: UniformTypeIdentifiers provides this constant on every supported macOS version.
    let movie = unsafe { UTTypeMPEG4Movie };
    return prompt_save("Snipsy Recording.mp4", &NSArray::from_slice(&[movie]));
}

fn prompt_save(suggested_name: &str, types: &NSArray<UTType>) -> Receiver<Result<Option<PathBuf>>> {
    let (sender, receiver) = async_channel::bounded(1);
    let mtm = MainThreadMarker::new().expect("dialogs open on the main thread");
    let panel = NSSavePanel::savePanel(mtm);
    panel.setNameFieldStringValue(&NSString::from_str(suggested_name));
    panel.setCanCreateDirectories(true);
    panel.setAllowsOtherFileTypes(false);
    panel.setAllowedContentTypes(types);
    panel.setLevel(NSPopUpMenuWindowLevel + 1);
    let callback_panel = panel.clone();
    let callback = RcBlock::new(move |response: isize| {
        let result = if response == NSModalResponseOK {
            callback_panel
                .URL()
                .and_then(|url| url.to_file_path())
                .map(Some)
                .context("Could not read the destination path")
        } else {
            Ok(None)
        };
        let _ = sender.try_send(result);
    });
    panel.beginWithCompletionHandler(&callback);
    return receiver;
}

/// Overlays, pins and dialogs float above normal windows; regular windows like Preferences
/// are captured like any other app's.
fn get_floating_window_ids(mtm: MainThreadMarker) -> Vec<u32> {
    return NSApplication::sharedApplication(mtm)
        .windows()
        .iter()
        .filter(|window| window.isVisible() && window.level() > NSNormalWindowLevel)
        .filter_map(|window| u32::try_from(window.windowNumber()).ok())
        .collect();
}

fn with_shareable_content(
    callback: impl Fn(Result<(&SCShareableContent, Retained<NSArray<SCWindow>>)>) + 'static,
) {
    let mtm = MainThreadMarker::new().expect("capture starts on the main thread");
    let floating_ids = get_floating_window_ids(mtm);
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            autoreleasepool(|_| {
                // SAFETY: ScreenCaptureKit keeps callback arguments alive for this invocation.
                let Some(content) = (unsafe { content.as_ref() }) else {
                    callback(Err(get_ns_error(error, "Could not enumerate displays")));
                    return;
                };
                // SAFETY: SCShareableContent getters are valid for the content's lifetime.
                let excluded = unsafe { content.windows() }
                    .iter()
                    .filter(|window| floating_ids.contains(&unsafe { window.windowID() }))
                    .collect::<Vec<_>>();
                callback(Ok((content, NSArray::from_retained_slice(&excluded))));
            });
        },
    );
    // SAFETY: the framework copies the completion block and invokes it with valid borrowed arguments.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, true, &block,
        );
    }
}

fn create_display_filter(
    display: &SCDisplay,
    excluded: &NSArray<SCWindow>,
) -> Retained<SCContentFilter> {
    // SAFETY: the display and window list are valid ScreenCaptureKit objects.
    return unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(
            SCContentFilter::alloc(),
            display,
            excluded,
        )
    };
}

fn get_window_stacking_order() -> HashMap<u32, usize> {
    return CGWindowListCopyWindowInfo(CGWindowListOption::OptionOnScreenOnly, 0)
        .map(|info| {
            // SAFETY: CGWindowListCopyWindowInfo returns CFString-keyed dictionaries of CFType values.
            unsafe { info.cast_unchecked::<CFDictionary<CFString, CFType>>() }
                .to_vec()
                .into_iter()
                .filter_map(|item| {
                    // SAFETY: kCGWindowNumber is a valid CFString constant.
                    return item
                        .get(unsafe { kCGWindowNumber })?
                        .downcast::<CFNumber>()
                        .ok()?
                        .as_i64();
                })
                .enumerate()
                .map(|(index, id)| (id as u32, index))
                .collect()
        })
        .unwrap_or_default();
}

fn get_capture_windows(
    content: &SCShareableContent,
    bounds: Rect,
    excluded: &NSArray<SCWindow>,
) -> Vec<Rect> {
    let order = get_window_stacking_order();
    // SAFETY: SCWindow getters are plain property reads on valid objects.
    let excluded = excluded
        .iter()
        .map(|window| unsafe { window.windowID() })
        .collect::<Vec<_>>();
    // SAFETY: SCWindow getters are plain property reads on valid objects.
    let mut windows = unsafe { content.windows() }
        .iter()
        .filter(|window| unsafe { window.isOnScreen() && window.windowLayer() == 0 })
        .filter(|window| !excluded.contains(&unsafe { window.windowID() }))
        .filter_map(|window| {
            let frame = get_rect(unsafe { window.frame() });
            let min = frame.pos.max(bounds.pos);
            let max = frame.end().min(bounds.end());
            ((max - min).min_element() >= 2.0).then(|| {
                (
                    unsafe { window.windowID() },
                    Rect::from_corners(min - bounds.pos, max - bounds.pos),
                )
            })
        })
        .collect::<Vec<_>>();
    windows.sort_by_key(|(id, _)| order.get(id).copied().unwrap_or(usize::MAX));
    return windows.into_iter().map(|(_, rect)| rect).collect();
}

struct DisplayCapture {
    bounds: Rect,
    image: image::RgbaImage,
}

pub fn capture_screen(bounds: Rect, shows_cursor: bool, sender: Sender<Result<Screenshot>>) {
    with_shareable_content(move |result| {
        let result = result.and_then(|(content, excluded)| {
            // SAFETY: SCShareableContent getters are valid for the content's lifetime.
            let displays = unsafe { content.displays() };
            let display_bounds = displays
                .iter()
                .map(|display| get_rect(unsafe { display.frame() }))
                .collect::<Vec<_>>();
            ensure!(
                display_bounds.iter().copied().reduce(Rect::union) == Some(bounds),
                "Display configuration changed; capture again"
            );
            let captures = displays
                .iter()
                .zip(display_bounds)
                .map(|(display, display_bounds)| {
                    let filter = create_display_filter(&display, &excluded);
                    // SAFETY: plain property access and setters on valid objects.
                    let (scale, config, display_space) = unsafe {
                        let scale = filter.pointPixelScale();
                        let display_space = CGDisplayCopyColorSpace(display.displayID());
                        let config = SCStreamConfiguration::new();
                        config.setWidth((display_bounds.size.x * scale).round() as usize);
                        config.setHeight((display_bounds.size.y * scale).round() as usize);
                        config.setShowsCursor(shows_cursor);
                        (scale, config, display_space)
                    };
                    (filter, config, display_space, display_bounds, scale)
                })
                .collect::<Vec<_>>();
            ensure!(!captures.is_empty(), "No display available");
            return Ok((captures, get_capture_windows(content, bounds, &excluded)));
        });
        let (captures, windows) = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = sender.try_send(Err(error));
                return;
            }
        };
        let scale = captures
            .iter()
            .map(|(_, _, _, _, scale)| *scale)
            .fold(1.0, f32::max);
        let total = captures.len();
        let pending = Arc::new(Mutex::new(Some(Vec::with_capacity(total))));
        for (filter, config, display_space, display_bounds, _) in captures {
            let pending = pending.clone();
            let sender = sender.clone();
            let windows = windows.clone();
            let block = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
                autoreleasepool(|_| {
                    // SAFETY: callback pointers are valid until this invocation returns.
                    let image = match unsafe { image.as_ref() } {
                        Some(image) => convert_capture_to_srgb(image, &display_space),
                        None => Err(get_ns_error(error, "Screen capture failed")),
                    };
                    let mut pending = pending.lock().expect("capture state poisoned");
                    let Some(images) = pending.as_mut() else {
                        return;
                    };
                    match image {
                        Ok(image) => images.push(DisplayCapture {
                            bounds: display_bounds,
                            image,
                        }),
                        Err(error) => {
                            *pending = None;
                            let _ = sender.try_send(Err(error));
                            return;
                        }
                    }
                    if images.len() < total {
                        return;
                    }
                    let images = pending.take().unwrap();
                    let result =
                        stitch_capture_images(bounds, images, scale).map(|image| Screenshot {
                            image: Arc::new(image),
                            bounds,
                            windows: windows.clone(),
                        });
                    let _ = sender.try_send(result);
                });
            });
            // SAFETY: the framework copies the completion block; filter and config live through this call.
            unsafe {
                SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                    &filter,
                    &config,
                    Some(&block),
                );
            }
        }
    });
}

struct RecordingOutputIvars {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    started: Mutex<bool>,
    failed: AtomicBool,
    errors: Sender<String>,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = AnyThread]
    #[name = "SnipsyRecordingOutput"]
    #[ivars = RecordingOutputIvars]
    struct RecordingOutput;
    unsafe impl NSObjectProtocol for RecordingOutput {}
    unsafe impl SCStreamOutput for RecordingOutput {
        #[allow(non_snake_case)]
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        unsafe fn stream_didOutputSampleBuffer_ofType(
            &self,
            _: &SCStream,
            sample_buffer: &CMSampleBuffer,
            output_type: SCStreamOutputType,
        ) {
            let ivars = self.ivars();
            if output_type != SCStreamOutputType::Screen
                || ivars.failed.load(Ordering::Acquire)
                || !unsafe { sample_buffer.is_valid() }
                || !unsafe { sample_buffer.data_is_ready() }
            {
                return;
            }
            let Some(attachments) = (unsafe { sample_buffer.sample_attachments_array(false) })
            else {
                return;
            };
            // SAFETY: CoreMedia sample attachments contain CFString-keyed dictionaries of CFType values.
            let attachments =
                unsafe { attachments.cast_unchecked::<CFDictionary<CFString, CFType>>() };
            if !attachments
                .get(0)
                .as_deref()
                .is_some_and(is_complete_recording_frame)
            {
                return;
            }
            let mut started = ivars.started.lock().expect("video writer poisoned");
            if !unsafe { ivars.input.isReadyForMoreMediaData() } {
                return;
            }
            if !*started {
                unsafe {
                    ivars
                        .writer
                        .startSessionAtSourceTime(sample_buffer.presentation_time_stamp())
                };
                *started = true;
            }
            if !unsafe { ivars.input.appendSampleBuffer(sample_buffer) } {
                self.report_error(get_writer_error(&ivars.writer));
            }
        }
    }
    unsafe impl SCStreamDelegate for RecordingOutput {
        #[allow(non_snake_case)]
        #[unsafe(method(stream:didStopWithError:))]
        unsafe fn stream_didStopWithError(&self, _: &SCStream, error: &NSError) {
            self.report_error(error.localizedDescription().to_string());
        }
    }
);

fn is_complete_recording_frame(attachments: &CFDictionary<CFString, CFType>) -> bool {
    // SAFETY: ScreenCaptureKit exposes this constant on every supported macOS version.
    return attachments
        .get(unsafe { SCStreamFrameInfoStatus }.as_ref())
        .and_then(|status| status.downcast::<CFNumber>().ok())
        .and_then(|status| status.as_isize())
        == Some(SCFrameStatus::Complete.0);
}

impl RecordingOutput {
    fn report_error(&self, error: String) {
        if !self.ivars().failed.swap(true, Ordering::AcqRel) {
            let _ = self.ivars().errors.try_send(error);
        }
    }
}

fn get_writer_error(writer: &AVAssetWriter) -> String {
    return unsafe { writer.error() }
        .map(|error| error.localizedDescription().to_string())
        .unwrap_or_else(|| "Could not encode screen recording".into());
}

pub struct ScreenRecorder {
    stream: Retained<SCStream>,
    output: Retained<RecordingOutput>,
    temporary_directory: tempfile::TempDir,
    destination: PathBuf,
}

fn create_recorder(
    content: &SCShareableContent,
    excluded: &NSArray<SCWindow>,
    bounds: Rect,
    destination: PathBuf,
    errors: Sender<String>,
) -> Result<ScreenRecorder> {
    // SAFETY: SCShareableContent getters are valid for the content's lifetime.
    let (display, region) = unsafe { content.displays() }
        .iter()
        .find_map(|display| {
            get_recording_region(get_rect(unsafe { display.frame() }), bounds)
                .map(|region| (display, region))
        })
        .context("Select an area within one display")?;
    let filter = create_display_filter(&display, excluded);
    let (width, height) = get_recording_size(region.size, unsafe { filter.pointPixelScale() })?;
    let temporary_directory = tempfile::Builder::new()
        .prefix(".snipsy-recording-")
        .tempdir_in(destination.parent().context("Invalid recording path")?)?;
    let url = NSURL::from_file_path(temporary_directory.path().join("recording.mp4"))
        .context("Invalid temporary recording path")?;
    let unavailable = "H.264 recording is unavailable";
    // SAFETY: ScreenCaptureKit and AVFoundation objects are configured with valid arguments
    // and the output object is fully initialized before it is handed to the stream.
    unsafe {
        let config = SCStreamConfiguration::new();
        config.setSourceRect(CGRect::new(
            CGPoint::new(region.pos.x as f64, region.pos.y as f64),
            CGSize::new(region.size.x as f64, region.size.y as f64),
        ));
        config.setWidth(width);
        config.setHeight(height);
        config.setMinimumFrameInterval(CMTime::new(1, 60));
        config.setQueueDepth(6);
        config.setColorSpaceName(kCGColorSpaceSRGB);
        config.setShowsCursor(true);
        config.setCapturesAudio(false);
        let writer = AVAssetWriter::initWithURL_fileType_error(
            AVAssetWriter::alloc(),
            &url,
            AVFileTypeMPEG4.context(unavailable)?,
        )
        .map_err(|error| anyhow!("{}", error.localizedDescription()))?;
        let width_number = NSNumber::new_usize(width);
        let height_number = NSNumber::new_usize(height);
        let settings = NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                AVVideoCodecKey.context(unavailable)?,
                AVVideoWidthKey.context(unavailable)?,
                AVVideoHeightKey.context(unavailable)?,
            ],
            &[
                AVVideoCodecTypeH264.context(unavailable)?.as_ref(),
                width_number.as_ref(),
                height_number.as_ref(),
            ],
        );
        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            AVMediaTypeVideo.context(unavailable)?,
            Some(&settings),
        );
        input.setExpectsMediaDataInRealTime(true);
        ensure!(
            writer.canAddInput(&input),
            "Could not configure the video encoder"
        );
        writer.addInput(&input);
        writer.setShouldOptimizeForNetworkUse(true);
        let allocated = RecordingOutput::alloc().set_ivars(RecordingOutputIvars {
            writer: writer.clone(),
            input,
            started: Mutex::new(false),
            failed: AtomicBool::new(false),
            errors,
        });
        let output: Retained<RecordingOutput> = msg_send![super(allocated), init];
        let stream = SCStream::initWithFilter_configuration_delegate(
            SCStream::alloc(),
            &filter,
            &config,
            Some(ProtocolObject::from_ref(&*output)),
        );
        stream
            .addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                None,
            )
            .map_err(|error| anyhow!("{}", error.localizedDescription()))?;
        ensure!(writer.startWriting(), get_writer_error(&writer));
        return Ok(ScreenRecorder {
            stream,
            output,
            temporary_directory,
            destination,
        });
    }
}

pub fn start_screen_recording(
    bounds: Rect,
    destination: PathBuf,
    sender: Sender<Result<ScreenRecorder>>,
    errors: Sender<String>,
) {
    if !bounds.pos.is_finite() || !bounds.size.is_finite() || bounds.size.min_element() <= 0.0 {
        let _ = sender.try_send(Err(anyhow!("Invalid recording region")));
        return;
    }
    with_shareable_content(move |result| {
        let recorder = match result.and_then(|(content, excluded)| {
            create_recorder(
                content,
                &excluded,
                bounds,
                destination.clone(),
                errors.clone(),
            )
        }) {
            Ok(recorder) => recorder,
            Err(error) => {
                let _ = sender.try_send(Err(error));
                return;
            }
        };
        let stream = recorder.stream.clone();
        let recorder = Mutex::new(Some(recorder));
        let sender = sender.clone();
        let block = RcBlock::new(move |error: *mut NSError| {
            let Some(recorder) = recorder.lock().expect("recorder poisoned").take() else {
                return;
            };
            if error.is_null() {
                let _ = sender.try_send(Ok(recorder));
                return;
            }
            unsafe { recorder.output.ivars().writer.cancelWriting() };
            let _ = sender.try_send(Err(get_ns_error(error, "Could not start recording")));
        });
        // SAFETY: the framework copies the completion block.
        unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };
    });
}

pub fn stop_screen_recording(recorder: ScreenRecorder, sender: Sender<Result<PathBuf>>) {
    let stream = recorder.stream.clone();
    let recorder = Mutex::new(Some(recorder));
    let block = RcBlock::new(move |error: *mut NSError| {
        let Some(recorder) = recorder.lock().expect("recorder poisoned").take() else {
            return;
        };
        let stop_error =
            (!error.is_null()).then(|| get_ns_error(error, "Recording stopped").to_string());
        let ivars = recorder.output.ivars();
        unsafe { ivars.input.markAsFinished() };
        let writer = ivars.writer.clone();
        let recorder = Mutex::new(Some(recorder));
        let sender = sender.clone();
        let finish = RcBlock::new(move || {
            let Some(recorder) = recorder.lock().expect("recorder poisoned").take() else {
                return;
            };
            let writer = &recorder.output.ivars().writer;
            let result = if unsafe { writer.status() } != AVAssetWriterStatus::Completed {
                Err(anyhow!(get_writer_error(writer)))
            } else {
                save_recording(
                    recorder.temporary_directory,
                    recorder.destination,
                    stop_error.as_deref(),
                )
            };
            let _ = sender.try_send(result);
        });
        // SAFETY: the framework copies the completion block; the recorder it captures keeps the stream and files alive.
        unsafe { writer.finishWritingWithCompletionHandler(&finish) };
    });
    // SAFETY: the framework copies the completion block.
    unsafe { stream.stopCaptureWithCompletionHandler(Some(&block)) };
}

fn save_recording(
    temporary_directory: tempfile::TempDir,
    destination: PathBuf,
    stop_error: Option<&str>,
) -> Result<PathBuf> {
    if let Err(error) = std::fs::rename(
        temporary_directory.path().join("recording.mp4"),
        &destination,
    ) {
        let recovery_path = temporary_directory.keep().join("recording.mp4");
        return Err(error).with_context(|| {
            format!(
                "Could not save recording; recover it at {}",
                recovery_path.display()
            )
        });
    }
    if let Some(error) = stop_error {
        bail!("{error}; recording saved to {}", destination.display());
    }
    return Ok(destination);
}

fn stitch_capture_images(
    bounds: Rect,
    captures: Vec<DisplayCapture>,
    scale: f32,
) -> Result<image::RgbaImage> {
    let (width, height) = get_scaled_size(bounds.size, scale)?;
    let mut output = image::RgbaImage::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
    for capture in captures {
        let size = get_scaled_size(capture.bounds.size, scale)?;
        let image = if capture.image.dimensions() == size {
            capture.image
        } else {
            image::imageops::resize(
                &capture.image,
                size.0,
                size.1,
                image::imageops::FilterType::Triangle,
            )
        };
        let offset = (capture.bounds.pos - bounds.pos) * scale;
        ensure!(
            offset.min_element() >= 0.0
                && offset.x + size.0 as f32 <= width as f32
                && offset.y + size.1 as f32 <= height as f32,
            "Display capture lies outside desktop bounds"
        );
        image::imageops::replace(
            &mut output,
            &image,
            offset.x.round() as i64,
            offset.y.round() as i64,
        );
    }
    return Ok(output);
}

fn get_scaled_size(size: Vec2, scale: f32) -> Result<(u32, u32)> {
    ensure!(
        size.is_finite() && scale.is_finite() && scale > 0.0,
        "Invalid desktop capture geometry"
    );
    let scaled = (size * scale).round();
    ensure!(
        scaled.min_element() > 0.0 && scaled.max_element() < u32::MAX as f32,
        "Desktop capture is too large"
    );
    return Ok((scaled.x as u32, scaled.y as u32));
}

fn get_recording_region(display: Rect, bounds: Rect) -> Option<Rect> {
    let region = Rect {
        pos: bounds.pos - display.pos,
        size: bounds.size,
    };
    return (region.pos.min_element() >= 0.0 && region.end().cmple(display.size).all())
        .then_some(region);
}

fn get_recording_size(size: Vec2, scale: f32) -> Result<(usize, usize)> {
    let (width, height) = get_scaled_size(size, scale)?;
    let even = |value: u32| (value + value % 2).max(2) as usize;
    return Ok((even(width), even(height)));
}

/// ScreenCaptureKit's own sRGB conversion is off by a level or two in dark tones, so captures
/// stay in the display's color space and ColorSync converts them.
fn convert_capture_to_srgb(
    image: &CGImage,
    display_space: &CGColorSpace,
) -> Result<image::RgbaImage> {
    let width = CGImage::width(Some(image));
    let height = CGImage::height(Some(image));
    let mut pixels = image::RgbaImage::new(u32::try_from(width)?, u32::try_from(height)?);
    let image = CGImage::new_copy_with_color_space(Some(image), Some(display_space))
        .context("Could not tag screen image with the display color space")?;
    // SAFETY: kCGColorSpaceSRGB is always defined.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .context("sRGB color space is unavailable")?;
    // SAFETY: the context draws into `pixels`, which outlives it and has `width * 4` byte rows.
    let context = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            Some(&srgb),
            CGImageAlphaInfo::NoneSkipLast.0 | CGImageByteOrderInfo::Order32Big.0,
        )
    }
    .context("Could not convert screen image to sRGB")?;
    CGContext::draw_image(
        Some(&context),
        CGRect::new(CGPoint::ZERO, CGSize::new(width as f64, height as f64)),
        Some(&image),
    );
    drop(context);
    for pixel in pixels.pixels_mut() {
        pixel[3] = 255;
    }
    return Ok(pixels);
}

#[cfg(test)]
mod tests {
    use super::{
        ACTIVE_COLOR_PICKER_TARGET, DisplayCapture, convert_capture_to_srgb, get_recording_region,
        get_recording_size, get_scaled_size, is_complete_recording_frame,
        release_color_picker_target, save_recording, set_srgb_colorspace, stitch_capture_images,
    };
    use crate::geometry::Rect;
    use glam::vec2;
    use image::{Rgba, RgbaImage};
    use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};
    use objc2_core_foundation::{CFDictionary, CFNumber, CFString, CFType};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImageAlphaInfo,
        CGImageByteOrderInfo, kCGColorSpaceDisplayP3, kCGColorSpaceSRGB,
    };
    use objc2_foundation::NSObject;
    use objc2_screen_capture_kit::{SCFrameStatus, SCStreamFrameInfoStatus};

    #[test]
    fn capture_converts_display_colors_to_srgb() {
        let mut pixel = [30u8, 30, 45, 255];
        let device = CGColorSpace::new_device_rgb().unwrap();
        // SAFETY: the context draws into `pixel`, which outlives it and holds one RGBA pixel.
        let context = unsafe {
            CGBitmapContextCreate(
                pixel.as_mut_ptr().cast(),
                1,
                1,
                8,
                4,
                Some(&device),
                CGImageAlphaInfo::NoneSkipLast.0 | CGImageByteOrderInfo::Order32Big.0,
            )
        }
        .unwrap();
        let image = CGBitmapContextCreateImage(Some(&context)).unwrap();
        let display_p3 = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceDisplayP3 })).unwrap();
        let converted = convert_capture_to_srgb(&image, &display_p3).unwrap();
        assert_eq!(converted.get_pixel(0, 0).0, [30, 30, 46, 255]);
    }

    #[test]
    fn overlay_layer_is_tagged_srgb() {
        // SAFETY: CAMetalLayer is a plain CALayer subclass; colorspace returns a borrowed CGColorSpace.
        let layer: Retained<AnyObject> = unsafe { msg_send![class!(CAMetalLayer), new] };
        set_srgb_colorspace(&layer);
        let colorspace: Option<&CGColorSpace> = unsafe { msg_send![&layer, colorspace] };
        assert_eq!(
            CGColorSpace::name(colorspace).as_deref(),
            Some(unsafe { kCGColorSpaceSRGB })
        );
    }

    #[test]
    fn stale_color_picker_does_not_release_the_new_shared_panel_target() {
        let targets = [NSObject::new(), NSObject::new()];
        let first = Retained::as_ptr(&targets[0]).cast();
        let second = Retained::as_ptr(&targets[1]).cast();
        ACTIVE_COLOR_PICKER_TARGET.set(first);
        ACTIVE_COLOR_PICKER_TARGET.set(second);

        assert!(!release_color_picker_target(first));
        assert_eq!(ACTIVE_COLOR_PICKER_TARGET.get(), second);
        assert!(release_color_picker_target(second));
        assert!(ACTIVE_COLOR_PICKER_TARGET.get().is_null());
        assert!(!release_color_picker_target(first));
        assert!(!release_color_picker_target(second));
    }

    #[test]
    fn recording_skips_incomplete_and_missing_frame_metadata() {
        // SAFETY: ScreenCaptureKit exposes this constant on every supported macOS version.
        let key = unsafe { SCStreamFrameInfoStatus }.as_ref();
        for status in [
            SCFrameStatus::Complete,
            SCFrameStatus::Idle,
            SCFrameStatus::Blank,
            SCFrameStatus::Suspended,
            SCFrameStatus::Started,
            SCFrameStatus::Stopped,
            SCFrameStatus(-1),
        ] {
            let value = CFNumber::new_isize(status.0);
            let attachments = CFDictionary::<CFString, CFType>::from_slices(&[key], &[&value]);
            assert_eq!(
                is_complete_recording_frame(&attachments),
                status == SCFrameStatus::Complete
            );
        }
        assert!(!is_complete_recording_frame(&CFDictionary::from_slices(
            &[],
            &[]
        )));
        assert!(!is_complete_recording_frame(&CFDictionary::<
            CFString,
            CFType,
        >::from_slices(
            &[key],
            &[&CFString::from_str("0")]
        )));
    }

    #[test]
    fn completed_recording_is_saved_after_stream_stop_error() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = tempfile::tempdir_in(directory.path()).unwrap();
        let temporary_path = temporary.path().to_path_buf();
        let destination = directory.path().join("capture.mp4");
        std::fs::write(temporary.path().join("recording.mp4"), b"completed video").unwrap();

        let error =
            save_recording(temporary, destination.clone(), Some("Stream stopped")).unwrap_err();

        assert_eq!(std::fs::read(&destination).unwrap(), b"completed video");
        assert!(error.to_string().contains(destination.to_str().unwrap()));
        assert!(!temporary_path.exists());
    }

    #[test]
    fn completed_recording_survives_destination_failure() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = tempfile::tempdir_in(directory.path()).unwrap();
        let recovery_path = temporary.path().join("recording.mp4");
        let destination = directory.path().join("capture.mp4");
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(&recovery_path, b"completed video").unwrap();

        let error = save_recording(temporary, destination.clone(), None).unwrap_err();

        assert_eq!(std::fs::read(&recovery_path).unwrap(), b"completed video");
        assert!(error.to_string().contains(recovery_path.to_str().unwrap()));
        assert!(destination.is_dir());
    }

    #[test]
    fn recording_dimensions_reject_float_rounded_u32_overflow() {
        assert!(get_scaled_size(vec2(u32::MAX as f32, 100.0), 1.0).is_err());
        assert!(get_recording_size(vec2(u32::MAX as f32, 100.0), 1.0).is_err());
    }

    #[test]
    fn stitch_preserves_display_positions_and_gaps() {
        let left = Rect {
            pos: vec2(-100.0, 0.0),
            size: vec2(100.0, 100.0),
        };
        let right = Rect {
            pos: vec2(100.0, 0.0),
            size: vec2(100.0, 100.0),
        };
        let image = stitch_capture_images(
            left.union(right),
            vec![
                DisplayCapture {
                    bounds: left,
                    image: RgbaImage::from_pixel(100, 100, Rgba([255, 0, 0, 255])),
                },
                DisplayCapture {
                    bounds: right,
                    image: RgbaImage::from_pixel(200, 200, Rgba([0, 0, 255, 255])),
                },
            ],
            2.0,
        )
        .unwrap();
        assert_eq!(image.dimensions(), (600, 200));
        assert_eq!(image.get_pixel(100, 100).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(300, 100).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(500, 100).0, [0, 0, 255, 255]);
        assert!(get_scaled_size(vec2(f32::MAX, 100.0), 2.0).is_err());
        assert!(get_scaled_size(vec2(100.0, 100.0), f32::NAN).is_err());
    }

    #[test]
    fn recording_regions_stay_within_one_display_and_use_even_pixels() {
        let display = Rect {
            pos: vec2(100.0, 50.0),
            size: vec2(800.0, 600.0),
        };
        assert_eq!(
            get_recording_region(
                display,
                Rect {
                    pos: vec2(150.0, 75.0),
                    size: vec2(200.0, 100.0),
                }
            ),
            Some(Rect {
                pos: vec2(50.0, 25.0),
                size: vec2(200.0, 100.0),
            })
        );
        assert!(
            get_recording_region(
                display,
                Rect {
                    pos: vec2(850.0, 75.0),
                    size: vec2(100.0, 100.0),
                }
            )
            .is_none()
        );
        assert_eq!(
            get_recording_size(vec2(100.5, 50.5), 2.0).unwrap(),
            (202, 102)
        );
    }
}
