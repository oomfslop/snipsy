#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    ScreenRecorder, StatusCommand, StatusItem, capture_screen, configure_overlay, confirm,
    create_status_item, get_app_data_dir, get_capture_bounds, get_clipboard_change_count,
    get_clipboard_paths, get_desktop_bounds, get_desktop_scale, get_quick_save_dir, hide_overlay,
    initialize_app, is_window_under_pointer, move_pointer, move_window, pick_color, print_png,
    prompt_save_image, prompt_save_recording, set_window_click_through, set_window_visible,
    show_error, start_screen_recording, stop_screen_recording,
};

#[cfg(not(target_os = "macos"))]
compile_error!("Screen capture currently requires macOS 14 or later");
