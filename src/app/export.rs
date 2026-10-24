use crate::{
    app::{
        self,
        content::{self, PinContent},
        storage,
    },
    lens, platform, raster,
};
use anyhow::Result;
use gpui_kit::{App, ClipboardItem, Image, ImageFormat};
use image::RgbaImage;
use std::{path::PathBuf, sync::Arc};

pub fn copy_image(image: Arc<RgbaImage>, cx: &mut App) {
    run_in_background(
        move || raster::encode_png(&image),
        |png, cx| {
            cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                ImageFormat::Png,
                png,
            )));
        },
        cx,
    );
}

pub fn print_image(image: Arc<RgbaImage>, cx: &mut App) {
    run_in_background(
        move || raster::encode_png(&image),
        |png, _| {
            if let Err(error) = platform::print_png(&png) {
                app::show_error(error);
            }
        },
        cx,
    );
}

pub fn search_image(image: Arc<RgbaImage>, cx: &mut App) {
    run_in_background(
        move || lens::create_handoff(&raster::encode_png(&image)?),
        |path, cx| {
            cx.open_with_system(&path);
            lens::schedule_cleanup(path, cx);
        },
        cx,
    );
}

pub fn save_content(content: PinContent, cx: &mut App) {
    let name = format!(
        "Snipsy-{}.{}",
        storage::get_timestamp_id() / 1_000_000_000,
        content.get_default_extension()
    );
    let path = platform::prompt_save_image(&name);
    cx.spawn(async move |cx| {
        let result = path.recv().await.unwrap_or_else(|error| Err(error.into()));
        cx.update(|cx| match result {
            Ok(Some(path)) => write_content(content, path, cx),
            Ok(None) => {}
            Err(error) => app::show_error(error),
        });
    })
    .detach();
}

pub fn quick_save_content(content: PinContent, cx: &mut App) {
    match platform::get_quick_save_dir() {
        Ok(directory) => {
            let path = storage::create_unique_path(
                &directory,
                "Snipsy-",
                storage::get_timestamp_id() / 1_000_000_000,
                content.get_default_extension(),
            );
            write_content(content, path, cx);
        }
        Err(error) => app::show_error(error),
    }
}

fn write_content(content: PinContent, path: PathBuf, cx: &mut App) {
    run_in_background(move || content::write_path(&content, &path), |(), _| {}, cx);
}

fn run_in_background<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
    finish: impl FnOnce(T, &mut App) + 'static,
    cx: &mut App,
) {
    let task = cx.background_executor().spawn(async move { work() });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| match result {
            Ok(value) => finish(value, cx),
            Err(error) => app::show_error(error),
        });
    })
    .detach();
}
