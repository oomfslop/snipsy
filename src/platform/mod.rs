#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    capture_screen, configure_overlay, get_capture_bounds, hide_overlay, initialize_app,
    is_window_under_pointer, pick_color, prompt_save_image, set_window_click_through, show_error,
};

#[cfg(not(target_os = "macos"))]
compile_error!("Screen capture currently requires macOS 14 or later");
