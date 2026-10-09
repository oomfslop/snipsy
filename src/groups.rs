use crate::app;
use gpui::{prelude::*, *};

pub struct Groups {
    focus: FocusHandle,
    error: Option<String>,
}

impl Groups {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        return Self { focus, error: None };
    }
}

impl Render for Groups {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (groups, active) = app::get_groups(cx);
        div()
            .id("groups")
            .size_full()
            .track_focus(&self.focus)
            .bg(rgb(0x202024))
            .text_color(rgb(0xffffff))
            .p_3()
            .flex()
            .flex_col()
            .gap_2()
            .overflow_y_scroll()
            .on_key_down(|event, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            })
            .children(groups.into_iter().enumerate().map(|(index, name)| {
                div()
                    .id(("group", index))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(rgb(if index == active { 0x327fff } else { 0x34343a }))
                    .hover(|style| style.bg(rgb(0x414148)))
                    .child(name)
                    .on_click(cx.listener(move |state, _, _, cx| {
                        state.error = app::switch_group(index, cx).err();
                        cx.notify();
                    }))
            }))
            .child(
                div()
                    .id("new-group")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(0x55555d))
                    .hover(|style| style.bg(rgb(0x34343a)))
                    .child("New group")
                    .on_click(cx.listener(|state, _, _, cx| {
                        state.error = app::create_group(None, cx).err();
                        cx.notify();
                    })),
            )
            .when_some(self.error.clone(), |root, error| {
                root.child(div().text_color(rgb(0xff8686)).child(error))
            })
    }
}
