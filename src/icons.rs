use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        let path = match path {
            "line" => "M3 12h18",
            "outline" => "M4 4h16v16H4Z",
            "filled" => "M4 4h16v16H4ZM4 8h16M4 12h16M4 16h16",
            "blur" => "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18M8 12h8M12 8v8",
            "bold" => "M6 3h7a4.5 4.5 0 0 1 0 9H6ZM6 12h8a4.5 4.5 0 0 1 0 9H6Z",
            "italic" => "M10 3h10M4 21h10M15 3 9 21",
            "underline" => "M5 3v7a7 7 0 0 0 14 0V3M4 21h16",
            "palette" => {
                "M12 3a9 9 0 1 0 0 18h2a2 2 0 0 0 0-4h-1a2 2 0 0 1 0-4h4a4 4 0 0 0 4-4c0-3-4-6-9-6ZM7 8h.1M12 6h.1M17 8h.1M6 13h.1"
            }
            "layers" => "M12 3 2 8l10 5 10-5ZM2 12l10 5 10-5M2 16l10 5 10-5",
            "forward" => "M6 15l6-6 6 6",
            "backward" => "M6 9l6 6 6-6",
            "delete" => "M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7",
            "select" => "M4 3l7 17 2-7 7-2Z",
            "rectangle" => "M4 4h16v16H4Z",
            "ellipse" => "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18",
            "arrow" => "M4 20 20 4M10 4h10v10",
            "pen" => "m16 3 5 5-13 13H3v-5ZM13 6l5 5",
            "highlight" => "m15 3 6 6-9 9-6-6ZM6 12l-2 6 2 2 6-2M3 22h8",
            "text" => "M4 5V3h16v2M12 3v18M8 21h8",
            "mosaic" => "M3 3h8v8H3ZM13 3h8v8h-8ZM3 13h8v8H3ZM13 13h8v8h-8Z",
            "eraser" => "m15 3 6 6-12 12H5l-4-4ZM7 11l8 8M12 21h9",
            "undo" => "M3 10h11a6 6 0 0 1 0 12M3 10l6-6M3 10l6 6",
            "redo" => "M21 10H10a6 6 0 0 0 0 12M21 10l-6-6M21 10l-6 6",
            "cancel" => "M6 6l12 12M6 18 18 6",
            "pin" => "m16 3 5 5M17 4l-7 7-4 1 6 6 1-4 7-7M9 15l-6 6",
            "save" => "M4 3h14l3 3v15H3V3ZM7 3v6h10V3M7 21v-8h10v8",
            "copy" => "M8 8h13v13H8ZM16 8V3H3v13h5",
            "search" => "M11 3a8 8 0 1 0 0 16 8 8 0 0 0 0-16M21 21l-5-5",
            "capture" => "M8 3H3v5M16 3h5v5M21 16v5h-5M8 21H3v-5M7 7h10v10H7Z",
            "apply" => "m4 12 5 5L20 6",
            "print" => "M6 9V3h12v6M6 18H3v-7h18v7h-3M6 15h12v6H6Z",
            _ => return Ok(None),
        };
        return Ok(Some(Cow::Owned(format!("<svg xmlns='http://www.w3.org/2000/svg' width='24' height='24' viewBox='0 0 24 24'><path d='{path}' fill='none' stroke='black' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round'/></svg>").into_bytes())));
    }

    fn list(&self, _: &str) -> gpui::Result<Vec<SharedString>> {
        return Ok(Vec::new());
    }
}

#[cfg(test)]
mod tests {
    use super::Icons;
    use gpui::AssetSource;

    #[test]
    fn toolbar_icons_have_visible_strokes() {
        for name in [
            "line",
            "outline",
            "filled",
            "blur",
            "bold",
            "italic",
            "underline",
            "palette",
            "layers",
            "forward",
            "backward",
            "delete",
            "select",
            "rectangle",
            "ellipse",
            "arrow",
            "pen",
            "highlight",
            "text",
            "mosaic",
            "eraser",
            "undo",
            "redo",
            "cancel",
            "pin",
            "save",
            "copy",
            "search",
            "capture",
            "apply",
            "print",
        ] {
            let svg = Icons.load(name).unwrap().unwrap();
            let tree = resvg::usvg::Tree::from_data(&svg, &Default::default()).unwrap();
            let mut pixmap = resvg::tiny_skia::Pixmap::new(48, 48).unwrap();
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(2.0, 2.0),
                &mut pixmap.as_mut(),
            );
            assert!(
                pixmap.pixels().iter().any(|pixel| pixel.alpha() != 0),
                "{name}"
            );
        }
    }
}
