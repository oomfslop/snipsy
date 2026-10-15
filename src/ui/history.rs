use crate::{app::history::History, pin};
use gpui_kit::{component::ActiveTheme, prelude::*, *};

pub struct HistoryView {
    focus: FocusHandle,
}

impl HistoryView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        return Self { focus };
    }
}

impl Render for HistoryView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let root = div()
            .id("history")
            .map(crate::ui::handle_window_actions)
            .size_full()
            .track_focus(&self.focus)
            .bg(theme.background)
            .p_3()
            .on_key_down(|event, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            });
        let entries = &cx.global::<History>().entries;
        if entries.is_empty() {
            return root
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child("Captures you copy, save, or pin appear here");
        }
        return root
            .flex()
            .flex_wrap()
            .content_start()
            .gap_3()
            .overflow_y_scroll()
            .children(entries.iter().map(|entry| {
                let image = entry.image.clone();
                div()
                    .id(("capture", std::sync::Arc::as_ptr(&entry.image) as usize))
                    .w(px(220.))
                    .h(px(140.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .border_1()
                    .border_color(theme.border)
                    .hover(|style| style.border_color(theme.ring))
                    .child(
                        img(entry.preview.clone())
                            .size_full()
                            .object_fit(ObjectFit::Contain),
                    )
                    .tooltip(|window, cx| component::tooltip::Tooltip::new("Pin").build(window, cx))
                    .on_click(move |_, _, cx| pin::open_image(image.clone(), None, cx))
            }));
    }
}
