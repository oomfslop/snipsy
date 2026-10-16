pub mod history;
pub mod magnifier;
pub mod preferences;

use crate::{app, geometry::Rect, pin, platform};
use anyhow::Result;
use glam::vec2;
use gpui_kit::{
    base::Root,
    component::{self, ActiveTheme, Sizable as _, Theme, ThemeRegistry, toolbar::Toolbar},
    *,
};
use std::borrow::Cow;

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        MousePointer2,
        Circle,
        MoveUpRight,
        PenLine,
        Highlighter,
        Type,
        Grid2x2,
        Droplet,
        Eraser,
        ScanSearch,
        Pin,
        Save,
        Printer,
        Layers,
        CircleDot,
        Bold,
        Italic,
        Underline,
        Trash,
        Keyboard,
        PanelTop,
        GripVertical,
    ]
);

pub fn init_theme(cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    registry
        .load_themes_from_str(include_str!("theme.json"))
        .expect("bundled theme is valid");
    let light = registry.themes()["Snipsy Light"].clone();
    let dark = registry.themes()["Snipsy Dark"].clone();
    Theme::change(cx.window_appearance(), None, cx);
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    Theme::change(cx.window_appearance(), None, cx);
}

/// Regular windows (Preferences, History) answer the Window menu; overlays and pins
/// leave its items disabled.
pub fn handle_window_actions(root: Stateful<Div>) -> Stateful<Div> {
    return root
        .on_action(|_: &app::CloseWindow, window, _| window.remove_window())
        .on_action(|_: &app::MinimizeWindow, window, _| window.minimize_window());
}

pub struct Assets;

pub const SQUARE_FILLED_ICON: &str = "icons/snipsy/square-filled.svg";
pub const CIRCLE_FILLED_ICON: &str = "icons/snipsy/circle-filled.svg";

const CUSTOM_ICONS: [(&str, &[u8]); 2] = [
    (SQUARE_FILLED_ICON, include_bytes!("square-filled.svg")),
    (CIRCLE_FILLED_ICON, include_bytes!("circle-filled.svg")),
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some((_, bytes)) = CUSTOM_ICONS.iter().find(|(icon, _)| *icon == path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        return gpui_kit::assets::Assets.load(path);
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.extend(
            CUSTOM_ICONS
                .iter()
                .filter(|(icon, _)| icon.starts_with(path))
                .map(|(icon, _)| (*icon).into()),
        );
        return Ok(paths);
    }
}

pub struct View<T> {
    pub window: AnyWindowHandle,
    pub entity: Entity<T>,
}

impl<T> Clone for View<T> {
    fn clone(&self) -> Self {
        return Self {
            window: self.window,
            entity: self.entity.clone(),
        };
    }
}

impl<T: 'static> View<T> {
    pub fn update<R>(
        &self,
        cx: &mut App,
        update: impl FnOnce(&mut T, &mut Window, &mut Context<T>) -> R,
    ) -> Result<R> {
        let entity = self.entity.clone();
        return self.window.update(cx, |_, window, cx| {
            entity.update(cx, |state, cx| update(state, window, cx))
        });
    }
}

pub fn open_overlay<V: Render>(
    bounds: Rect,
    resizable: bool,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Result<View<V>> {
    let mut entity = None;
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(to_bounds(bounds))),
            titlebar: None,
            show: false,
            focus: false,
            is_movable: resizable,
            is_resizable: resizable,
            is_minimizable: false,
            window_background: WindowBackgroundAppearance::Transparent,
            window_min_size: resizable.then(|| size(px(pin::MIN_SIZE), px(pin::MIN_SIZE))),
            ..Default::default()
        },
        |window, cx| {
            platform::configure_overlay(window, bounds, resizable);
            let view = build(window, cx);
            entity = Some(view.clone());
            cx.new(|cx| Root::new(view, window, cx).bg(transparent_black()))
        },
    )?;
    return Ok(View {
        window: window.into(),
        entity: entity.expect("overlay builder ran"),
    });
}

pub fn to_bounds(rect: Rect) -> Bounds<Pixels> {
    return Bounds::new(
        point(px(rect.pos.x), px(rect.pos.y)),
        size(px(rect.size.x), px(rect.size.y)),
    );
}

pub fn to_rect(bounds: Bounds<Pixels>) -> Rect {
    return Rect {
        pos: vec2(bounds.origin.x.into(), bounds.origin.y.into()),
        size: vec2(bounds.size.width.into(), bounds.size.height.into()),
    };
}

pub fn create_toolbar(id: impl Into<ElementId>) -> Toolbar {
    return Toolbar::new(id)
        .with_size(component::Size::Medium)
        .h_auto()
        .p(TOOLBAR_PADDING)
        .gap(TOOLBAR_PADDING);
}

const TOOLBAR_PADDING: Pixels = px(4.);

pub fn create_toolbar_panel(cx: &App) -> Div {
    return panel(cx).rounded(cx.theme().radius + TOOLBAR_PADDING + px(1.));
}

pub fn panel(cx: &App) -> Div {
    let theme = cx.theme();
    return div()
        .bg(theme.popover)
        .text_color(theme.popover_foreground)
        .border_1()
        .border_color(theme.border)
        .rounded(theme.radius)
        .shadow_md();
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, Window,
        assets::IconName,
        component::{ActiveTheme as _, Theme, ThemeMode, button::Button},
        div, px,
    };

    struct ToolbarHost;

    impl Render for ToolbarHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            window.set_rem_size(px(13.));
            return div().flex().items_start().child(
                super::create_toolbar_panel(cx)
                    .debug_selector(|| "panel".into())
                    .child(
                        super::create_toolbar("toolbar").child(
                            Button::new("button")
                                .icon(IconName::Copy)
                                .debug_selector(|| "button".into()),
                        ),
                    ),
            );
        }
    }

    #[gpui_kit::test]
    fn bundled_theme_replaces_default_colors(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            super::init_theme(cx);
            for mode in [ThemeMode::Light, ThemeMode::Dark] {
                Theme::change(mode, None, cx);
                let theme = cx.theme();
                assert!(theme.font_size == px(13.));
                assert!(theme.group_box != theme.background, "{mode:?}");
            }
        });
    }

    #[gpui_kit::test]
    fn toolbar_buttons_have_equal_insets(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(|_, _| ToolbarHost);
        cx.run_until_parked();
        let panel = cx.debug_bounds("panel").unwrap();
        let button = cx.debug_bounds("button").unwrap();
        let start = button.origin - panel.origin;
        let end = panel.bottom_right() - button.bottom_right();
        assert_eq!(start.x, start.y);
        assert_eq!(start, end);
    }
}
