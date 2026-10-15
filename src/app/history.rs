use crate::{
    app::storage,
    geometry::Rect,
    platform,
    raster::{self, Screenshot},
};
use anyhow::Result;
use gpui_kit::{App, Global, RenderImage, Task};
use image::RgbaImage;
use std::{path::Path, sync::Arc};

const LIMIT: usize = 10;

pub struct HistoryEntry {
    pub image: Arc<RgbaImage>,
    pub preview: Arc<RenderImage>,
}

impl HistoryEntry {
    fn new(image: Arc<RgbaImage>) -> Self {
        let preview = raster::create_render_image(&image::imageops::thumbnail(&*image, 220, 140));
        return Self { image, preview };
    }
}

enum PersistCommand {
    Save(Arc<RgbaImage>),
    Clear,
}

#[derive(Default)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
    pub captures: Vec<Screenshot>,
    pub last_selection: Option<(Rect, Rect)>,
    persist: Option<async_channel::Sender<PersistCommand>>,
    load_task: Option<Task<()>>,
}

impl Global for History {}

pub fn init(data_dir: &Path, cx: &mut App) {
    cx.set_global(History::default());
    let directory = data_dir.join("history");
    let (sender, receiver) = async_channel::unbounded();
    let (loaded, load) = async_channel::bounded(1);
    cx.global_mut::<History>().persist = Some(sender);
    cx.background_executor()
        .spawn(async move {
            let _ = loaded.try_send(load_entries(&directory));
            while let Ok(command) = receiver.recv().await {
                let result = match command {
                    PersistCommand::Save(image) => write_entry(&directory, &image),
                    PersistCommand::Clear => clear_entries(&directory),
                };
                if let Err(error) = result {
                    eprintln!("could not persist history: {error:#}");
                }
            }
        })
        .detach();
    let task = cx.spawn(async move |cx| {
        let images = load.recv().await.unwrap_or_else(|error| Err(error.into()));
        cx.update(|cx| {
            match images {
                Ok(images) => {
                    let history = cx.global_mut::<History>();
                    let remaining = LIMIT.saturating_sub(history.entries.len());
                    history
                        .entries
                        .extend(images.into_iter().take(remaining).map(HistoryEntry::new));
                }
                Err(error) => eprintln!("could not load history: {error:#}"),
            }
            cx.refresh_windows();
        });
    });
    cx.global_mut::<History>().load_task = Some(task);
}

pub fn remember(image: Arc<RgbaImage>, capture: Option<Screenshot>, cx: &mut App) {
    let history = cx.global_mut::<History>();
    if let Some(persist) = &history.persist {
        let _ = persist.try_send(PersistCommand::Save(image.clone()));
    }
    if let Some(capture) = capture {
        insert_capture(&mut history.captures, capture);
    }
    history.entries.insert(0, HistoryEntry::new(image));
    let removed = history.entries.split_off(history.entries.len().min(LIMIT));
    for entry in removed {
        cx.drop_image(entry.preview, None);
    }
    cx.refresh_windows();
}

pub fn remember_selection(bounds: Rect, selection: Rect, cx: &mut App) {
    cx.global_mut::<History>().last_selection = Some((bounds, selection));
}

pub fn get_last_selection(bounds: Rect, cx: &App) -> Option<Rect> {
    return cx
        .global::<History>()
        .last_selection
        .and_then(|(saved, selection)| (saved == bounds).then_some(selection));
}

pub fn clear(cx: &mut App) {
    if !platform::confirm(
        "Clear capture history?",
        "Recent captures are deleted from this Mac.",
        "Clear",
    ) {
        return;
    }
    clear_history(cx);
}

fn clear_history(cx: &mut App) {
    let history = cx.global_mut::<History>();
    history.load_task = None;
    let entries = std::mem::take(&mut history.entries);
    history.captures.clear();
    if let Some(persist) = &history.persist {
        let _ = persist.try_send(PersistCommand::Clear);
    }
    for entry in entries {
        cx.drop_image(entry.preview, None);
    }
    cx.refresh_windows();
}

fn insert_capture(captures: &mut Vec<Screenshot>, capture: Screenshot) {
    captures.retain(|entry| !Arc::ptr_eq(&entry.image, &capture.image));
    captures.insert(0, capture);
    captures.truncate(LIMIT);
}

fn write_entry(directory: &Path, image: &RgbaImage) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    storage::write_image(
        image,
        &storage::create_unique_path(directory, "", storage::get_timestamp_id(), "png"),
    )?;
    let files = storage::list_numbered_files(directory, "png")?;
    for (_, path) in &files[..files.len().saturating_sub(LIMIT)] {
        std::fs::remove_file(path)?;
    }
    return Ok(());
}

fn load_entries(directory: &Path) -> Result<Vec<Arc<RgbaImage>>> {
    return Ok(storage::list_numbered_files(directory, "png")?
        .into_iter()
        .rev()
        .take(LIMIT)
        .filter_map(|(_, path)| match image::open(&path) {
            Ok(image) => Some(Arc::new(image.to_rgba8())),
            Err(error) => {
                eprintln!("could not load {}: {error}", path.display());
                None
            }
        })
        .collect());
}

fn clear_entries(directory: &Path) -> Result<()> {
    for (_, path) in storage::list_numbered_files(directory, "png")? {
        std::fs::remove_file(path)?;
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::{
        History, clear_entries, clear_history, init, insert_capture, load_entries, remember,
        write_entry,
    };
    use crate::{app::storage, geometry::Rect, raster::Screenshot};
    use gpui_kit::TestAppContext;
    use image::{Rgba, RgbaImage};
    use std::sync::Arc;

    #[gpui_kit::test(iterations = 20)]
    fn loading_history_keeps_captures_added_during_startup(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let old = Arc::new(RgbaImage::from_pixel(1, 1, Rgba([1, 0, 0, 255])));
        let new = Arc::new(RgbaImage::from_pixel(1, 1, Rgba([2, 0, 0, 255])));
        write_entry(&directory.path().join("history"), &old).unwrap();
        cx.update(|cx| {
            init(directory.path(), cx);
            remember(new.clone(), None, cx);
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let entries = &cx.global::<History>().entries;
            assert_eq!(entries.len(), 2);
            assert!(Arc::ptr_eq(&entries[0].image, &new));
            assert_eq!(*entries[1].image, *old);
        });
    }

    #[gpui_kit::test(iterations = 20)]
    fn clearing_history_during_startup_does_not_restore_deleted_captures(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        let image = Arc::new(RgbaImage::new(1, 1));
        let path = directory.path().join("history");
        write_entry(&path, &image).unwrap();
        cx.update(|cx| {
            init(directory.path(), cx);
            clear_history(cx);
        });
        cx.run_until_parked();
        cx.update(|cx| assert!(cx.global::<History>().entries.is_empty()));
        assert!(load_entries(&path).unwrap().is_empty());
        cx.update(|cx| remember(image.clone(), None, cx));
        cx.run_until_parked();
        cx.update(|cx| {
            let entries = &cx.global::<History>().entries;
            assert_eq!(entries.len(), 1);
            assert!(Arc::ptr_eq(&entries[0].image, &image));
        });
        assert_eq!(load_entries(&path).unwrap().len(), 1);
    }

    #[test]
    fn capture_frames_deduplicate_and_stay_bounded() {
        let screenshot = |image: &Arc<RgbaImage>| Screenshot {
            image: image.clone(),
            bounds: Rect {
                pos: glam::Vec2::ZERO,
                size: glam::Vec2::ONE,
            },
            windows: Vec::new(),
        };
        let repeated = Arc::new(RgbaImage::new(1, 1));
        let mut captures = Vec::new();
        insert_capture(&mut captures, screenshot(&repeated));
        for _ in 0..11 {
            insert_capture(&mut captures, screenshot(&Arc::new(RgbaImage::new(1, 1))));
        }
        insert_capture(&mut captures, screenshot(&repeated));
        assert_eq!(captures.len(), 10);
        assert!(Arc::ptr_eq(&captures[0].image, &repeated));
        assert_eq!(
            captures
                .iter()
                .filter(|entry| Arc::ptr_eq(&entry.image, &repeated))
                .count(),
            1
        );
    }

    #[test]
    fn persisted_entries_load_newest_first_and_drop_stale_files() {
        let directory = tempfile::tempdir().unwrap();
        let first = RgbaImage::from_pixel(2, 1, Rgba([1, 2, 3, 255]));
        let second = RgbaImage::from_pixel(1, 2, Rgba([4, 5, 6, 255]));
        write_entry(directory.path(), &first).unwrap();
        write_entry(directory.path(), &second).unwrap();
        let loaded = load_entries(directory.path()).unwrap();
        assert_eq!(*loaded[0], second);
        assert_eq!(*loaded[1], first);
        for value in 0..10 {
            write_entry(
                directory.path(),
                &RgbaImage::from_pixel(1, 1, Rgba([value, 0, 0, 255])),
            )
            .unwrap();
        }
        let files = storage::list_numbered_files(directory.path(), "png").unwrap();
        assert_eq!(files.len(), 10);
        clear_entries(directory.path()).unwrap();
        assert!(load_entries(directory.path()).unwrap().is_empty());
    }
}
