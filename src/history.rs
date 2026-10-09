use crate::{app::AppState, pin};
use gpui::{prelude::*, *};

pub struct History {
    focus: FocusHandle,
}

impl History {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        return Self { focus };
    }
}

impl Render for History {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let captures = cx.global::<AppState>().history.clone();
        div()
            .id("history")
            .size_full()
            .track_focus(&self.focus)
            .bg(rgb(0x202024))
            .p_3()
            .on_key_down(|event, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            })
            .when(captures.is_empty(), |root| {
                root.flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0xa0a0a8))
                    .child("No captures yet")
            })
            .when(!captures.is_empty(), |root| {
                root.flex()
                    .flex_wrap()
                    .content_start()
                    .gap_3()
                    .overflow_y_scroll()
                    .children(
                        captures
                            .into_iter()
                            .enumerate()
                            .map(|(index, (image, preview))| {
                                div()
                                    .id(("capture", index))
                                    .w(px(220.0))
                                    .h(px(140.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .overflow_hidden()
                                    .rounded_md()
                                    .bg(rgb(0x151518))
                                    .border_1()
                                    .border_color(rgb(0x3a3a40))
                                    .cursor_pointer()
                                    .hover(|style| style.border_color(rgb(0x327fff)))
                                    .child(img(preview).size_full().object_fit(ObjectFit::Contain))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        pin::open_pin(image.clone(), None, cx)
                                    }))
                            }),
                    )
            })
    }
}
