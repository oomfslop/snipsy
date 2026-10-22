use crate::{
    app::recording::{RecordingSettings, VideoCodec},
    editor::{self, BlurMode},
    geometry::{Color, Rect},
    raster::{self, CursorSnapshot, Screenshot, VideoEffect},
};
use anyhow::{Context as _, Result, anyhow, ensure};
use async_channel::{Receiver, Sender};
use block2::RcBlock;
use glam::{Vec2, vec2};
use objc2::{
    AnyThread, ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message as _,
    define_class, msg_send,
    rc::{Retained, autoreleasepool},
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSColor, NSColorPanel, NSColorSpace,
    NSCursor, NSEvent, NSImage, NSImageView, NSModalResponseOK, NSPasteboard,
    NSPasteboardTypeFileURL, NSPopUpMenuWindowLevel, NSPrintOperation, NSSavePanel, NSScreen,
    NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_av_foundation::{
    AVAsset, AVAssetExportPresetHEVCHighestQuality, AVAssetExportPresetHighestQuality,
    AVAssetExportPresetPassthrough, AVAssetExportSession, AVAssetExportSessionStatus,
    AVAssetImageGenerator, AVAssetWriter, AVAssetWriterInput, AVAssetWriterStatus,
    AVAsynchronousCIImageFilteringRequest, AVFileTypeMPEG4, AVMediaTypeVideo,
    AVMutableVideoComposition, AVPlayer, AVPlayerActionAtItemEnd, AVPlayerItem, AVPlayerItemStatus,
    AVPlayerItemVideoOutput, AVURLAsset, AVVideoAverageBitRateKey, AVVideoCodecKey,
    AVVideoCodecTypeH264, AVVideoCodecTypeHEVC, AVVideoCompressionPropertiesKey,
    AVVideoExpectedSourceFrameRateKey, AVVideoHeightKey, AVVideoMaxKeyFrameIntervalKey,
    AVVideoWidthKey,
};
use objc2_core_foundation::{
    CFDictionary, CFNumber, CFRetained, CFString, CFType, CGAffineTransform, CGPoint, CGRect,
    CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDisplayCopyColorSpace, CGError, CGImage,
    CGImageAlphaInfo, CGImageByteOrderInfo, CGPreflightScreenCaptureAccess,
    CGRectMakeWithDictionaryRepresentation, CGRequestScreenCaptureAccess,
    CGWarpMouseCursorPosition, CGWindowListCopyWindowInfo, CGWindowListOption, kCGColorSpaceSRGB,
    kCGDisplayStreamYCbCrMatrix_ITU_R_709_2, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer,
};
use objc2_core_image::{CIImage, CIVector, kCIFormatRGBA8, kCIInputCenterKey, kCIInputScaleKey};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeRange, kCMTimeZero};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetPixelFormatType, kCVPixelBufferIOSurfacePropertiesKey,
    kCVPixelBufferMetalCompatibilityKey, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use objc2_foundation::{
    NSArray, NSData, NSDate, NSDateFormatter, NSDictionary, NSError, NSFileManager, NSNumber,
    NSObject, NSObjectProtocol, NSOperationQueue, NSPoint, NSRect, NSSearchPathDirectory,
    NSSearchPathDomainMask, NSSize, NSString, NSURL,
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
    path::{Path, PathBuf},
    ptr::NonNull,
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
    // AppKit passes clicks through fully transparent pixels unless this is set explicitly.
    native.setIgnoresMouseEvents(false);
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
    return prompt_save(suggested_name, &NSArray::from_retained_slice(&types), None);
}

pub fn prompt_save_video() -> Receiver<Result<Option<PathBuf>>> {
    // SAFETY: UniformTypeIdentifiers provides this constant on every supported macOS version.
    let movie = unsafe { UTTypeMPEG4Movie };
    let formatter = NSDateFormatter::new();
    formatter.setDateFormat(Some(&NSString::from_str("yyyy-MM-dd 'at' HH.mm.ss")));
    let name = format!(
        "Snipsy Recording {}.mp4",
        formatter.stringFromDate(&NSDate::now())
    );
    let directory = get_user_directory(NSSearchPathDirectory::DocumentDirectory, "Snipsy").ok();
    return prompt_save(&name, &NSArray::from_slice(&[movie]), directory.as_deref());
}

fn prompt_save(
    suggested_name: &str,
    types: &NSArray<UTType>,
    directory: Option<&Path>,
) -> Receiver<Result<Option<PathBuf>>> {
    let (sender, receiver) = async_channel::bounded(1);
    let mtm = MainThreadMarker::new().expect("dialogs open on the main thread");
    let panel = NSSavePanel::savePanel(mtm);
    panel.setNameFieldStringValue(&NSString::from_str(suggested_name));
    if let Some(url) = directory.and_then(NSURL::from_directory_path) {
        panel.setDirectoryURL(Some(&url));
    }
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

pub fn get_capture_windows(bounds: Rect) -> Vec<Rect> {
    let Some(info) = CGWindowListCopyWindowInfo(
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements,
        0,
    ) else {
        return Vec::new();
    };
    // SAFETY: CGWindowListCopyWindowInfo returns CFString-keyed dictionaries of CFType values, front to back.
    return unsafe { info.cast_unchecked::<CFDictionary<CFString, CFType>>() }
        .to_vec()
        .into_iter()
        .filter_map(|window| {
            let number = |key: &CFString| window.get(key)?.downcast::<CFNumber>().ok();
            // SAFETY: the window info keys are valid CFString constants.
            let (layer, alpha, frame) = unsafe {
                (
                    number(kCGWindowLayer)?.as_i64()?,
                    number(kCGWindowAlpha)?.as_f64()?,
                    window
                        .get(kCGWindowBounds)?
                        .downcast::<CFDictionary>()
                        .ok()?,
                )
            };
            if layer != 0 || alpha <= 0.0 {
                return None;
            }
            let mut rect = CGRect::ZERO;
            // SAFETY: `frame` is the bounds dictionary CoreGraphics produced and `rect` is a valid out pointer.
            if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&frame), &mut rect) } {
                return None;
            }
            let frame = get_rect(rect);
            let min = frame.pos.max(bounds.pos);
            let max = frame.end().min(bounds.end());
            return ((max - min).min_element() >= 2.0)
                .then(|| Rect::from_corners(min - bounds.pos, max - bounds.pos));
        })
        .collect();
}

struct DisplayCapture {
    bounds: Rect,
    image: CFRetained<CGImage>,
    color_space: CFRetained<CGColorSpace>,
}

struct ShareableContent(Retained<SCShareableContent>);

// SAFETY: SCShareableContent is an immutable snapshot that ScreenCaptureKit itself hands to arbitrary queues.
unsafe impl Send for ShareableContent {}
unsafe impl Sync for ShareableContent {}

impl ShareableContent {
    fn get_display_bounds(&self) -> Option<Rect> {
        // SAFETY: SCShareableContent getters are plain property reads on a valid snapshot.
        return unsafe { self.0.displays() }
            .iter()
            .map(|display| get_rect(unsafe { display.frame() }))
            .reduce(Rect::union);
    }

    fn get_window(&self, id: u32) -> Option<Retained<SCWindow>> {
        // SAFETY: SCShareableContent getters are plain property reads on a valid snapshot.
        return unsafe { self.0.windows() }
            .iter()
            .find(|window| unsafe { window.windowID() } == id);
    }
}

#[derive(Clone, Default)]
pub struct CaptureSource(Arc<Mutex<Option<Arc<ShareableContent>>>>);

fn load_shareable_content(callback: impl Fn(Result<Arc<ShareableContent>>) + 'static) {
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: ScreenCaptureKit keeps callback arguments alive for this invocation.
            callback(match unsafe { content.as_ref() } {
                Some(content) => Ok(Arc::new(ShareableContent(content.retain()))),
                None if CGPreflightScreenCaptureAccess() => {
                    Err(get_ns_error(error, "Could not enumerate displays"))
                }
                None => {
                    CGRequestScreenCaptureAccess();
                    Err(anyhow!(
                        "Enable Screen Recording for Snipsy (or its terminal) in System Settings, then capture again."
                    ))
                }
            });
        },
    );
    // SAFETY: the framework copies the completion block and invokes it with valid borrowed arguments.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, false, &block,
        );
    }
}

pub fn get_cursor_snapshot() -> Option<CursorSnapshot> {
    // Deprecated in favor of ScreenCaptureKit's showsCursor, which would capture the overlay's cursor instead.
    #[allow(deprecated)]
    let cursor = NSCursor::currentSystemCursor()?;
    let image = cursor.image();
    let size = image.size();
    let size = vec2(size.width as f32, size.height as f32);
    let (width, height) = get_scaled_size(size, get_desktop_scale()).ok()?;
    let mut rect = NSRect::new(NSPoint::ZERO, NSSize::new(width as f64, height as f64));
    // SAFETY: `rect` is a valid in/out pointer; no context or hints are needed.
    let source = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
    let mut pixels = draw_cg_image(&source, width, height)?;
    for pixel in pixels.pixels_mut() {
        let alpha = pixel[3] as u32;
        for channel in &mut pixel.0[..3] {
            if let Some(straight) = (*channel as u32 * 255).checked_div(alpha) {
                *channel = straight.min(255) as u8;
            }
        }
    }
    let hotspot = cursor.hotSpot();
    return Some(CursorSnapshot {
        image: pixels,
        position: get_pointer_position() - vec2(hotspot.x as f32, hotspot.y as f32),
        size,
    });
}

fn draw_cg_image(source: &CGImage, width: u32, height: u32) -> Option<image::RgbaImage> {
    let mut pixels = image::RgbaImage::new(width, height);
    // SAFETY: kCGColorSpaceSRGB is always defined.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    // SAFETY: the context draws into `pixels`, which outlives it and has `width * 4` byte rows.
    let context = unsafe {
        CGBitmapContextCreate(
            pixels.as_mut_ptr().cast(),
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            Some(&srgb),
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        )
    }?;
    CGContext::draw_image(
        Some(&context),
        CGRect::new(CGPoint::ZERO, CGSize::new(width as f64, height as f64)),
        Some(source),
    );
    drop(context);
    return Some(pixels);
}

pub fn get_window_id(window: &gpui_kit::Window) -> u32 {
    return u32::try_from(get_native_window(window).windowNumber()).unwrap_or_default();
}

pub fn capture_screen(
    source: &CaptureSource,
    overlay: u32,
    bounds: Rect,
    windows: Vec<Rect>,
    sender: Sender<Result<Screenshot>>,
) {
    let cached = source.0.lock().expect("capture source poisoned").clone();
    let source = source.clone();
    if let Some(content) = cached.filter(|content| {
        content.get_display_bounds() == Some(bounds) && content.get_window(overlay).is_some()
    }) {
        capture_displays(&content, overlay, bounds, windows, sender);
        load_shareable_content(move |result| {
            if let Ok(content) = result {
                *source.0.lock().expect("capture source poisoned") = Some(content);
            }
        });
        return;
    }
    load_shareable_content(move |result| match result {
        Ok(content) => {
            *source.0.lock().expect("capture source poisoned") = Some(content.clone());
            capture_displays(&content, overlay, bounds, windows.clone(), sender.clone());
        }
        Err(error) => {
            let _ = sender.try_send(Err(error));
        }
    });
}

fn capture_displays(
    content: &ShareableContent,
    overlay: u32,
    bounds: Rect,
    windows: Vec<Rect>,
    sender: Sender<Result<Screenshot>>,
) {
    let prepared = (|| {
        ensure!(
            content.get_display_bounds() == Some(bounds),
            "Display configuration changed; capture again"
        );
        let overlay = content
            .get_window(overlay)
            .context("Could not exclude the capture overlay from the screenshot")?;
        let excluded = NSArray::from_retained_slice(&[overlay]);
        // SAFETY: SCShareableContent getters are plain property reads on a valid snapshot.
        let captures = unsafe { content.0.displays() }
            .iter()
            .map(|display| {
                let display_bounds = get_rect(unsafe { display.frame() });
                let filter = create_display_filter(&display, &excluded);
                // SAFETY: plain property access and setters on valid objects.
                let (scale, config, display_space) = unsafe {
                    let scale = filter.pointPixelScale();
                    let config = SCStreamConfiguration::new();
                    config.setWidth((display_bounds.size.x * scale).round() as usize);
                    config.setHeight((display_bounds.size.y * scale).round() as usize);
                    config.setShowsCursor(false);
                    (scale, config, CGDisplayCopyColorSpace(display.displayID()))
                };
                (filter, config, display_space, display_bounds, scale)
            })
            .collect::<Vec<_>>();
        ensure!(!captures.is_empty(), "No display available");
        return Ok(captures);
    })();
    let captures = match prepared {
        Ok(captures) => captures,
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
    let (displays, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result =
            stitch_capture_images(bounds, scale, received.iter().take(total), total).map(|image| {
                Screenshot {
                    image: Arc::new(image),
                    bounds,
                    windows,
                }
            });
        let _ = sender.try_send(result);
    });
    for (filter, config, display_space, display_bounds, _) in captures {
        let displays = displays.clone();
        let block = RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
            // SAFETY: callback pointers are valid until this invocation returns.
            let capture = match unsafe { image.as_ref() } {
                Some(image) => Ok(DisplayCapture {
                    bounds: display_bounds,
                    image: objc2_core_foundation::Type::retain(image),
                    color_space: display_space.clone(),
                }),
                None => Err(get_ns_error(error, "Screen capture failed")),
            };
            let _ = displays.send(capture);
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
    video: RecordedVideo,
}

pub struct RecordedVideo {
    pub path: PathBuf,
    pub size: (u32, u32),
    pub codec: VideoCodec,
    _directory: tempfile::TempDir,
}

fn create_recorder(
    content: &ShareableContent,
    overlay: u32,
    bounds: Rect,
    settings: RecordingSettings,
    errors: Sender<String>,
) -> Result<ScreenRecorder> {
    let overlay = content
        .get_window(overlay)
        .context("Could not exclude the recording overlay")?;
    let excluded = NSArray::from_retained_slice(&[overlay]);
    // SAFETY: SCShareableContent getters are valid for the content's lifetime.
    let (display, region) = unsafe { content.0.displays() }
        .iter()
        .find_map(|display| {
            get_recording_region(get_rect(unsafe { display.frame() }), bounds)
                .map(|region| (display, region))
        })
        .context("Select an area within one display")?;
    let filter = create_display_filter(&display, &excluded);
    let (width, height) = get_recording_size(region.size, unsafe { filter.pointPixelScale() })?;
    let directory = tempfile::Builder::new()
        .prefix("snipsy-recording-")
        .tempdir()?;
    let path = directory.path().join("Recording.mp4");
    let url = NSURL::from_file_path(&path).context("Invalid temporary recording path")?;
    let unavailable = "Video recording is unavailable";
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
        config.setMinimumFrameInterval(CMTime::new(1, settings.fps as i32));
        config.setPixelFormat(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange);
        config.setColorMatrix(kCGDisplayStreamYCbCrMatrix_ITU_R_709_2);
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
        let codec = match settings.codec {
            VideoCodec::H264 => AVVideoCodecTypeH264,
            VideoCodec::Hevc => AVVideoCodecTypeHEVC,
        };
        let compression = NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                AVVideoAverageBitRateKey.context(unavailable)?,
                AVVideoExpectedSourceFrameRateKey.context(unavailable)?,
                AVVideoMaxKeyFrameIntervalKey.context(unavailable)?,
            ],
            &[
                NSNumber::new_u64(u64::from(settings.bitrate_mbps) * 1_000_000).as_ref(),
                NSNumber::new_u32(settings.fps).as_ref(),
                NSNumber::new_u32(settings.fps).as_ref(),
            ],
        );
        let video_settings = NSDictionary::<NSString, AnyObject>::from_slices(
            &[
                AVVideoCodecKey.context(unavailable)?,
                AVVideoWidthKey.context(unavailable)?,
                AVVideoHeightKey.context(unavailable)?,
                AVVideoCompressionPropertiesKey.context(unavailable)?,
            ],
            &[
                codec.context(unavailable)?.as_ref(),
                NSNumber::new_usize(width).as_ref(),
                NSNumber::new_usize(height).as_ref(),
                compression.as_ref(),
            ],
        );
        let input = AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
            AVMediaTypeVideo.context(unavailable)?,
            Some(&video_settings),
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
            video: RecordedVideo {
                path,
                size: (width as u32, height as u32),
                codec: settings.codec,
                _directory: directory,
            },
        });
    }
}

pub fn start_screen_recording(
    bounds: Rect,
    settings: RecordingSettings,
    overlay: u32,
    sender: Sender<Result<ScreenRecorder>>,
    errors: Sender<String>,
) {
    if !bounds.pos.is_finite() || !bounds.size.is_finite() || bounds.size.min_element() <= 0.0 {
        let _ = sender.try_send(Err(anyhow!("Invalid recording region")));
        return;
    }
    load_shareable_content(move |result| {
        let recorder = match result.and_then(|content| {
            create_recorder(&content, overlay, bounds, settings, errors.clone())
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

pub fn stop_screen_recording(recorder: ScreenRecorder, sender: Sender<Result<RecordedVideo>>) {
    let stream = recorder.stream.clone();
    let recorder = Mutex::new(Some(recorder));
    let block = RcBlock::new(move |_: *mut NSError| {
        let Some(recorder) = recorder.lock().expect("recorder poisoned").take() else {
            return;
        };
        let ivars = recorder.output.ivars();
        if !*ivars.started.lock().expect("video writer poisoned") {
            unsafe { ivars.writer.cancelWriting() };
            let _ = sender.try_send(Err(anyhow!("The recording has no frames")));
            return;
        }
        unsafe { ivars.input.markAsFinished() };
        let writer = ivars.writer.clone();
        let recorder = Mutex::new(Some(recorder));
        let sender = sender.clone();
        let finish = RcBlock::new(move || {
            let Some(recorder) = recorder.lock().expect("recorder poisoned").take() else {
                return;
            };
            let writer = &recorder.output.ivars().writer;
            let _ = sender.try_send(
                if unsafe { writer.status() } == AVAssetWriterStatus::Completed {
                    Ok(recorder.video)
                } else {
                    Err(anyhow!(get_writer_error(writer)))
                },
            );
        });
        // SAFETY: the framework copies the completion block; the recorder it captures keeps the stream and files alive.
        unsafe { writer.finishWritingWithCompletionHandler(&finish) };
    });
    // SAFETY: the framework copies the completion block.
    unsafe { stream.stopCaptureWithCompletionHandler(Some(&block)) };
}

enum CoreImageEffect {
    Overlay {
        image: Retained<CIImage>,
        time: Option<(f64, f64)>,
    },
    Blur {
        rect: Rect,
        mode: BlurMode,
        amount: f32,
        time: Option<(f64, f64)>,
    },
}

#[derive(Default)]
struct VideoEffects(Mutex<Vec<CoreImageEffect>>);

// SAFETY: CIImage is immutable and Core Image documents it as safe to share between threads.
unsafe impl Send for VideoEffects {}
unsafe impl Sync for VideoEffects {}

fn create_core_image_effects(effects: Vec<VideoEffect>) -> Vec<CoreImageEffect> {
    // SAFETY: kCGColorSpaceSRGB is always defined.
    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
    return effects
        .into_iter()
        .map(|effect| match effect {
            VideoEffect::Overlay { mut image, time } => {
                raster::update_pixels(&mut image, |pixel| {
                    let alpha = pixel[3] as u32;
                    for channel in &mut pixel[..3] {
                        *channel = ((*channel as u32 * alpha + 127) / 255) as u8;
                    }
                });
                let size = CGSize::new(image.width() as f64, image.height() as f64);
                let data = NSData::with_bytes(&image);
                // SAFETY: `data` holds premultiplied RGBA rows of `width * 4` bytes, which Core Image copies.
                let overlay = unsafe {
                    CIImage::imageWithBitmapData_bytesPerRow_size_format_colorSpace(
                        &data,
                        image.width() as usize * 4,
                        size,
                        kCIFormatRGBA8,
                        srgb.as_deref(),
                    )
                };
                CoreImageEffect::Overlay {
                    image: overlay,
                    time,
                }
            }
            VideoEffect::Blur {
                rect,
                mode,
                amount,
                time,
            } => CoreImageEffect::Blur {
                rect,
                mode,
                amount,
                time,
            },
        })
        .collect();
}

fn apply_video_effects(
    source: Retained<CIImage>,
    effects: &[CoreImageEffect],
    crop: Option<Rect>,
    time: f64,
) -> Retained<CIImage> {
    // SAFETY: Core Image builds lazy, immutable filter graphs from valid images and parameters.
    unsafe {
        let height = source.extent().size.height;
        let get_core_image_rect = |rect: Rect| {
            CGRect::new(
                CGPoint::new(rect.pos.x as f64, height - rect.end().y as f64),
                CGSize::new(rect.size.x as f64, rect.size.y as f64),
            )
        };
        let mut image = source;
        for effect in effects {
            let (CoreImageEffect::Overlay { time: range, .. }
            | CoreImageEffect::Blur { time: range, .. }) = effect;
            if !editor::get_is_in_time_range(*range, time) {
                continue;
            }
            image = match effect {
                CoreImageEffect::Overlay { image: overlay, .. } => {
                    overlay.imageByCompositingOverImage(&image)
                }
                CoreImageEffect::Blur {
                    rect, mode, amount, ..
                } => {
                    let rect = get_core_image_rect(*rect);
                    let clamped = image.imageByClampingToExtent();
                    let blurred = match mode {
                        BlurMode::Gaussian => {
                            clamped.imageByApplyingGaussianBlurWithSigma(*amount as f64)
                        }
                        BlurMode::Pixelated => {
                            let center = CIVector::vectorWithX_Y(
                                rect.origin.x,
                                rect.origin.y + rect.size.height,
                            );
                            let scale = NSNumber::new_f32(*amount);
                            let parameters = NSDictionary::<NSString, AnyObject>::from_slices(
                                &[kCIInputScaleKey, kCIInputCenterKey],
                                &[scale.as_ref(), center.as_ref()],
                            );
                            clamped.imageByApplyingFilter_withInputParameters(
                                &NSString::from_str("CIPixellate"),
                                &parameters,
                            )
                        }
                    };
                    blurred
                        .imageByCroppingToRect(rect)
                        .imageByCompositingOverImage(&image)
                }
            };
        }
        let Some(crop) = crop else {
            return image;
        };
        let crop = get_core_image_rect(crop);
        return image
            .imageByCroppingToRect(crop)
            .imageByApplyingTransform(CGAffineTransform {
                a: 1.0,
                b: 0.0,
                c: 0.0,
                d: 1.0,
                tx: -crop.origin.x,
                ty: -crop.origin.y,
            });
    }
}

fn create_video_composition(
    asset: &AVAsset,
    effects: Arc<VideoEffects>,
    crop: Option<Rect>,
) -> Retained<AVMutableVideoComposition> {
    let handler = RcBlock::new(
        move |request: NonNull<AVAsynchronousCIImageFilteringRequest>| {
            autoreleasepool(|_| {
                // SAFETY: AVFoundation keeps the request alive until it is finished.
                let request = unsafe { request.as_ref() };
                let effects = effects.0.lock().expect("video effects poisoned");
                let (source, time) =
                    unsafe { (request.sourceImage(), request.compositionTime().seconds()) };
                let image = apply_video_effects(source, &effects, crop, time);
                unsafe { request.finishWithImage_context(&image, None) };
            });
        },
    );
    // SAFETY: the framework copies the handler, which only touches thread-safe effects.
    unsafe {
        // Only the mutable variant accepts a renderSize, which cropping needs.
        #[allow(deprecated)]
        let composition =
            AVMutableVideoComposition::videoCompositionWithAsset_applyingCIFiltersWithHandler(
                asset, &handler,
            );
        if let Some(crop) = crop {
            composition.setRenderSize(CGSize::new(crop.size.x as f64, crop.size.y as f64));
        }
        return composition;
    }
}

fn get_media_time(seconds: f64) -> CMTime {
    // SAFETY: plain value conversion.
    return unsafe { CMTime::with_seconds(seconds, 60_000) };
}

fn open_video_asset(path: &Path) -> Result<Retained<AVURLAsset>> {
    let url = NSURL::from_file_path(path).context("Invalid video path")?;
    // SAFETY: the URL is a valid file URL; no options are needed.
    return Ok(unsafe { AVURLAsset::URLAssetWithURL_options(&url, None) });
}

pub struct VideoPlayer {
    asset: Retained<AVURLAsset>,
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    output: Retained<AVPlayerItemVideoOutput>,
    effects: Arc<VideoEffects>,
    nudged: Cell<bool>,
}

impl VideoPlayer {
    pub fn open(path: &Path) -> Result<Self> {
        let mtm = MainThreadMarker::new().expect("video playback starts on the main thread");
        let asset = open_video_asset(path)?;
        let effects = Arc::new(VideoEffects::default());
        // SAFETY: AVFoundation objects are created on the main thread with valid arguments;
        // the toll-free bridged CoreVideo keys are valid NSString dictionary keys.
        unsafe {
            let item = AVPlayerItem::playerItemWithAsset(&asset, mtm);
            let get_key = |key: &CFString| &*(key as *const CFString).cast::<NSString>();
            let format = NSNumber::new_u32(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange);
            let metal = NSNumber::new_bool(true);
            let surface = NSDictionary::<NSString, AnyObject>::new();
            let attributes = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    get_key(kCVPixelBufferPixelFormatTypeKey),
                    get_key(kCVPixelBufferMetalCompatibilityKey),
                    get_key(kCVPixelBufferIOSurfacePropertiesKey),
                ],
                &[format.as_ref(), metal.as_ref(), surface.as_ref()],
            );
            let output = AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                AVPlayerItemVideoOutput::alloc(),
                Some(&attributes),
            );
            item.addOutput(&output);
            let player = AVPlayer::playerWithPlayerItem(Some(&item), mtm);
            player.setActionAtItemEnd(AVPlayerActionAtItemEnd::Pause);
            return Ok(Self {
                asset,
                player,
                item,
                output,
                effects,
                nudged: Cell::new(false),
            });
        }
    }

    pub fn get_is_ready(&self) -> Result<bool> {
        // SAFETY: plain property reads on a valid item.
        return match unsafe { self.item.status() } {
            AVPlayerItemStatus::ReadyToPlay => Ok(true),
            AVPlayerItemStatus::Failed => Err(unsafe { self.item.error() }.map_or_else(
                || anyhow!("Could not play the recording"),
                |error| anyhow!("{}", error.localizedDescription()),
            )),
            _ => Ok(false),
        };
    }

    pub fn get_duration(&self) -> f64 {
        // SAFETY: plain property reads on valid objects.
        let duration = unsafe { self.asset.duration().seconds() };
        return if duration.is_finite() {
            duration.max(0.0)
        } else {
            0.0
        };
    }

    pub fn get_time(&self) -> f64 {
        // SAFETY: plain property read on a valid item.
        let time = unsafe { self.item.currentTime().seconds() };
        return if time.is_finite() { time } else { 0.0 };
    }

    pub fn get_is_playing(&self) -> bool {
        // SAFETY: plain property read on a valid player.
        return unsafe { self.player.rate() } != 0.0;
    }

    pub fn play(&self, end: f64) {
        // SAFETY: plain setters on valid objects.
        unsafe {
            self.item.setForwardPlaybackEndTime(get_media_time(end));
            self.player.play();
        }
    }

    pub fn pause(&self) {
        // SAFETY: plain call on a valid player.
        unsafe { self.player.pause() };
    }

    pub fn seek(&self, time: f64) {
        // SAFETY: zero tolerances request the exact frame.
        unsafe {
            self.player.seekToTime_toleranceBefore_toleranceAfter(
                get_media_time(time),
                kCMTimeZero,
                kCMTimeZero,
            );
        }
    }

    pub fn copy_frame(&self) -> Option<gpui_kit::SurfaceSource> {
        // SAFETY: the output vends retained pixel buffers for valid item times.
        let buffer = unsafe {
            let time = self.item.currentTime();
            if !self.output.hasNewPixelBufferForItemTime(time) {
                return None;
            }
            self.output
                .copyPixelBufferForItemTime_itemTimeForDisplay(time, std::ptr::null_mut())?
        };
        // SAFETY: both wrappers own a single +1 reference to the same CoreVideo object.
        let buffer: CFRetained<CVPixelBuffer> =
            unsafe { CFRetained::from_raw(NonNull::new_unchecked(Retained::into_raw(buffer))) };
        // GPUI's surface renderer asserts this format.
        return (CVPixelBufferGetPixelFormatType(&buffer)
            == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange)
            .then(|| buffer.into());
    }

    pub fn set_effects(&self, effects: Vec<VideoEffect>) {
        let composited = !effects.is_empty();
        *self.effects.0.lock().expect("video effects poisoned") =
            create_core_image_effects(effects);
        // SAFETY: plain property access on a valid item; the composition is created from its own asset.
        unsafe {
            if composited != self.item.videoComposition().is_some() {
                let composition = composited
                    .then(|| create_video_composition(&self.asset, self.effects.clone(), None));
                self.item
                    .setVideoComposition(composition.as_deref().map(|composition| &**composition));
            }
        }
        if self.get_is_playing() {
            return;
        }
        // AVPlayer only composes frames it decodes and ignores seeks to the current time,
        // so a paused frame is redrawn by nudging the time back and forth within the frame.
        let nudge = if self.nudged.replace(!self.nudged.get()) {
            -1e-4
        } else {
            1e-4
        };
        self.seek(self.get_time() + nudge);
    }
}

pub struct VideoThumbnails(Retained<AVAssetImageGenerator>);

impl Drop for VideoThumbnails {
    fn drop(&mut self) {
        // SAFETY: cancelling finished requests has no effect.
        unsafe { self.0.cancelAllCGImageGeneration() };
    }
}

pub fn generate_video_thumbnails(
    path: &Path,
    times: &[f64],
    size: (u32, u32),
    sender: Sender<(usize, image::RgbaImage)>,
) -> Result<VideoThumbnails> {
    let asset = open_video_asset(path)?;
    // SAFETY: the generator is configured with plain values before any request.
    let generator = unsafe {
        let generator = AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset);
        generator.setAppliesPreferredTrackTransform(true);
        generator.setMaximumSize(CGSize::new(size.0 as f64, size.1 as f64));
        generator
    };
    for (index, time) in times.iter().enumerate() {
        let sender = sender.clone();
        let handler = RcBlock::new(move |image: *mut CGImage, _: CMTime, _: *mut NSError| {
            // SAFETY: the image is valid for the duration of the handler when generation succeeded.
            let Some(image) = (unsafe { image.as_ref() }) else {
                return;
            };
            let (width, height) = (CGImage::width(Some(image)), CGImage::height(Some(image)));
            if let Some(pixels) = draw_cg_image(image, width as u32, height as u32) {
                let _ = sender.try_send((index, pixels));
            }
        });
        // SAFETY: the handler only touches the thread-safe sender.
        unsafe {
            generator.generateCGImageAsynchronouslyForTime_completionHandler(
                get_media_time(*time),
                &handler,
            )
        };
    }
    return Ok(VideoThumbnails(generator));
}

pub struct VideoExport(Retained<AVAssetExportSession>);

impl VideoExport {
    pub fn get_progress(&self) -> f32 {
        // SAFETY: plain property read on a valid session.
        return unsafe { self.0.progress() };
    }
}

impl Drop for VideoExport {
    fn drop(&mut self) {
        // SAFETY: cancelling a finished session has no effect.
        unsafe { self.0.cancelExport() };
    }
}

pub fn export_video(
    source: &Path,
    destination: PathBuf,
    range: (f64, f64),
    crop: Option<Rect>,
    effects: Vec<VideoEffect>,
    codec: VideoCodec,
    sender: Sender<Result<()>>,
) -> Result<VideoExport> {
    let asset = open_video_asset(source)?;
    let directory = tempfile::Builder::new()
        .prefix(".snipsy-export-")
        .tempdir_in(destination.parent().context("Invalid export path")?)?;
    let output = directory.path().join("export.mp4");
    let url = NSURL::from_file_path(&output).context("Invalid export path")?;
    let reencode = crop.is_some() || !effects.is_empty();
    let unavailable = "Video export is unavailable";
    // SAFETY: the export session is configured with valid objects before it starts.
    let session = unsafe {
        let preset = match (reencode, codec) {
            (false, _) => AVAssetExportPresetPassthrough,
            (true, VideoCodec::H264) => AVAssetExportPresetHighestQuality,
            (true, VideoCodec::Hevc) => AVAssetExportPresetHEVCHighestQuality,
        };
        let session = AVAssetExportSession::exportSessionWithAsset_presetName(&asset, preset)
            .context(unavailable)?;
        session.setOutputURL(Some(&url));
        session.setOutputFileType(Some(AVFileTypeMPEG4.context(unavailable)?));
        session.setShouldOptimizeForNetworkUse(true);
        session.setTimeRange(CMTimeRange {
            start: get_media_time(range.0),
            duration: get_media_time(range.1 - range.0),
        });
        if reencode {
            let effects = VideoEffects(Mutex::new(create_core_image_effects(effects)));
            session.setVideoComposition(Some(&create_video_composition(
                &asset,
                Arc::new(effects),
                crop,
            )));
        }
        session
    };
    let callback_session = session.clone();
    let directory = Mutex::new(Some(directory));
    let handler = RcBlock::new(move || {
        let Some(directory) = directory.lock().expect("export directory poisoned").take() else {
            return;
        };
        // SAFETY: plain property reads on a finished session.
        let result = match unsafe { callback_session.status() } {
            AVAssetExportSessionStatus::Completed => {
                std::fs::rename(&output, &destination).context("Could not save the video")
            }
            AVAssetExportSessionStatus::Cancelled => Err(anyhow!("Export cancelled")),
            _ => Err(unsafe { callback_session.error() }.map_or_else(
                || anyhow!("Could not export the video"),
                |error| anyhow!("{}", error.localizedDescription()),
            )),
        };
        drop(directory);
        let _ = sender.try_send(result);
    });
    // SAFETY: the framework copies the completion handler.
    unsafe { session.exportAsynchronouslyWithCompletionHandler(&handler) };
    return Ok(VideoExport(session));
}

fn stitch_capture_images(
    bounds: Rect,
    scale: f32,
    captures: impl Iterator<Item = Result<DisplayCapture>>,
    total: usize,
) -> Result<image::RgbaImage> {
    let (width, height) = get_scaled_size(bounds.size, scale)?;
    let mut output = image::RgbaImage::new(width, height);
    let mut drawn = 0;
    for capture in captures {
        let capture = capture?;
        let size = get_scaled_size(capture.bounds.size, scale)?;
        let offset = ((capture.bounds.pos - bounds.pos) * scale).round();
        ensure!(
            offset.min_element() >= 0.0
                && offset.x + size.0 as f32 <= width as f32
                && offset.y + size.1 as f32 <= height as f32,
            "Display capture lies outside desktop bounds"
        );
        let image =
            CGImage::new_copy_with_color_space(Some(&capture.image), Some(&capture.color_space))
                .context("Could not tag screen image with the display color space")?;
        let stride = width as usize * 4;
        let top = offset.y as usize * stride;
        let rows = &mut (*output)[top..top + size.1 as usize * stride];
        draw_display_rows(rows, width, &image, offset.x, size)?;
        drawn += 1;
    }
    ensure!(drawn == total, "Screen capture failed");
    raster::update_pixels(&mut output, |pixel| pixel[3] = 255);
    return Ok(output);
}

/// ColorSync converts on one thread per draw, so same-sized captures are split into row bands
/// converted in parallel. Scaled captures stay whole because resampling would seam at band edges.
fn draw_display_rows(
    rows: &mut [u8],
    width: u32,
    image: &CGImage,
    left: f32,
    size: (u32, u32),
) -> Result<()> {
    let stride = width as usize * 4;
    let same_size = (CGImage::width(Some(image)), CGImage::height(Some(image)))
        == (size.0 as usize, size.1 as usize);
    let bands = if same_size {
        std::thread::available_parallelism().map_or(1, |count| count.get())
    } else {
        1
    };
    let band_rows = (size.1 as usize).div_ceil(bands).max(1);
    return std::thread::scope(|scope| {
        let tasks = rows
            .chunks_mut(band_rows * stride)
            .enumerate()
            .map(|(index, band)| {
                scope.spawn(move || {
                    let height = band.len() / stride;
                    let source = if same_size {
                        CGImage::with_image_in_rect(
                            Some(image),
                            CGRect::new(
                                CGPoint::new(0.0, (index * band_rows) as f64),
                                CGSize::new(size.0 as f64, height as f64),
                            ),
                        )
                        .context("Could not split the screen image")?
                    } else {
                        objc2_core_foundation::Type::retain(image)
                    };
                    // SAFETY: kCGColorSpaceSRGB is always defined.
                    let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
                        .context("sRGB color space is unavailable")?;
                    // SAFETY: the context draws into `band`, which outlives it and has `stride` byte rows.
                    let context = unsafe {
                        CGBitmapContextCreate(
                            band.as_mut_ptr().cast(),
                            width as usize,
                            height,
                            8,
                            stride,
                            Some(&srgb),
                            CGImageAlphaInfo::NoneSkipLast.0 | CGImageByteOrderInfo::Order32Big.0,
                        )
                    }
                    .context("Could not convert screen image to sRGB")?;
                    CGContext::draw_image(
                        Some(&context),
                        CGRect::new(
                            CGPoint::new(left as f64, 0.0),
                            CGSize::new(size.0 as f64, height as f64),
                        ),
                        Some(&source),
                    );
                    return anyhow::Ok(());
                })
            })
            .collect::<Vec<_>>();
        return tasks
            .into_iter()
            .try_for_each(|task| task.join().expect("capture conversion panicked"));
    });
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
#[cfg(test)]
mod tests {
    use super::{
        ACTIVE_COLOR_PICKER_TARGET, DisplayCapture, get_recording_region, get_recording_size,
        get_scaled_size, is_complete_recording_frame, release_color_picker_target,
        set_srgb_colorspace, stitch_capture_images,
    };
    use crate::geometry::Rect;
    use glam::vec2;
    use image::{Rgba, RgbaImage};
    use objc2::{class, msg_send, rc::Retained, runtime::AnyObject};
    use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImageAlphaInfo,
        CGImageByteOrderInfo, kCGColorSpaceDisplayP3, kCGColorSpaceSRGB,
    };
    use objc2_foundation::NSObject;
    use objc2_screen_capture_kit::{SCFrameStatus, SCStreamFrameInfoStatus};

    fn create_display_capture(
        bounds: Rect,
        size: (usize, usize),
        pixel: [u8; 4],
        color_space: CFRetained<CGColorSpace>,
    ) -> DisplayCapture {
        let pixels = RgbaImage::from_pixel(size.0 as u32, size.1 as u32, Rgba(pixel));
        return create_display_capture_from(bounds, pixels, color_space);
    }

    fn create_display_capture_from(
        bounds: Rect,
        mut pixels: RgbaImage,
        color_space: CFRetained<CGColorSpace>,
    ) -> DisplayCapture {
        let size = (pixels.width() as usize, pixels.height() as usize);
        let device = CGColorSpace::new_device_rgb().unwrap();
        // SAFETY: the context draws into `pixels`, which outlives it and has `width * 4` byte rows.
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                size.0,
                size.1,
                8,
                size.0 * 4,
                Some(&device),
                CGImageAlphaInfo::NoneSkipLast.0 | CGImageByteOrderInfo::Order32Big.0,
            )
        }
        .unwrap();
        return DisplayCapture {
            bounds,
            image: CGBitmapContextCreateImage(Some(&context)).unwrap(),
            color_space,
        };
    }

    fn get_srgb() -> CFRetained<CGColorSpace> {
        return CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).unwrap();
    }

    #[test]
    fn parallel_conversion_keeps_every_row_in_place() {
        let bounds = Rect {
            pos: vec2(0.0, 0.0),
            size: vec2(64.0, 97.0),
        };
        let pixels = RgbaImage::from_fn(128, 194, |x, y| Rgba([y as u8, x as u8, 7, 255]));
        let image = stitch_capture_images(
            bounds,
            2.0,
            [Ok(create_display_capture_from(
                bounds,
                pixels.clone(),
                get_srgb(),
            ))]
            .into_iter(),
            1,
        )
        .unwrap();
        assert_eq!(image, pixels);
    }

    #[test]
    fn capture_converts_display_colors_to_srgb() {
        let bounds = Rect {
            pos: vec2(0.0, 0.0),
            size: vec2(1.0, 1.0),
        };
        let display_p3 = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceDisplayP3 })).unwrap();
        let converted = stitch_capture_images(
            bounds,
            1.0,
            [Ok(create_display_capture(
                bounds,
                (1, 1),
                [30, 30, 45, 255],
                display_p3,
            ))]
            .into_iter(),
            1,
        )
        .unwrap();
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
            2.0,
            [
                Ok(create_display_capture(
                    left,
                    (100, 100),
                    [255, 0, 0, 255],
                    get_srgb(),
                )),
                Ok(create_display_capture(
                    right,
                    (200, 200),
                    [0, 0, 255, 255],
                    get_srgb(),
                )),
            ]
            .into_iter(),
            2,
        )
        .unwrap();
        assert_eq!(image.dimensions(), (600, 200));
        assert_eq!(image.get_pixel(100, 100).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(300, 100).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(500, 100).0, [0, 0, 255, 255]);

        let top = Rect {
            pos: vec2(0.0, 0.0),
            size: vec2(100.0, 50.0),
        };
        let bottom = Rect {
            pos: vec2(50.0, 50.0),
            size: vec2(50.0, 50.0),
        };
        let image = stitch_capture_images(
            top.union(bottom),
            1.0,
            [
                Ok(create_display_capture(
                    top,
                    (100, 50),
                    [255, 0, 0, 255],
                    get_srgb(),
                )),
                Ok(create_display_capture(
                    bottom,
                    (50, 50),
                    [0, 255, 0, 255],
                    get_srgb(),
                )),
            ]
            .into_iter(),
            2,
        )
        .unwrap();
        assert_eq!(image.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(10, 90).0, [0, 0, 0, 255]);
        assert_eq!(image.get_pixel(90, 90).0, [0, 255, 0, 255]);
        let missing = [Ok(create_display_capture(
            top,
            (100, 50),
            [255, 0, 0, 255],
            get_srgb(),
        ))];
        assert!(stitch_capture_images(top.union(bottom), 1.0, missing.into_iter(), 2).is_err());
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
