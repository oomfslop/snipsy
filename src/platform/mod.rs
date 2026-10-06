#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    capture_screen, configure_overlay, get_capture_bounds, hide_overlay, initialize_app,
    prompt_save_image, show_error,
};

#[cfg(not(target_os = "macos"))]
compile_error!("Screen capture currently requires macOS 14 or later");
