#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    capture_screen, configure_overlay, get_app_data_dir, get_capture_bounds,
    get_clipboard_change_count, get_clipboard_paths, get_desktop_bounds, get_desktop_scale,
    get_quick_save_dir, hide_overlay, initialize_app, is_window_under_pointer, move_pointer,
    move_window, pick_color, prompt_save_image, set_window_click_through, set_window_visible,
    show_error,
};

#[cfg(not(target_os = "macos"))]
compile_error!("Screen capture currently requires macOS 14 or later");
