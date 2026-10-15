use crate::app::{self, groups::Groups};
use gpui_kit::{
    component::{
        ActiveTheme, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        v_flex,
    },
    prelude::*,
    *,
};

pub struct GroupsView {
    focus: FocusHandle,
}

impl GroupsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.observe_global::<Groups>(|_, cx| cx.notify()).detach();
        return Self { focus };
    }
}

impl Render for GroupsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let groups = cx.global::<Groups>();
        return v_flex()
            .id("groups")
            .size_full()
            .track_focus(&self.focus)
            .bg(cx.theme().background)
            .p_3()
            .gap_1()
            .overflow_y_scroll()
            .on_key_down(|event, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            })
            .children(groups.names.iter().enumerate().map(|(index, name)| {
                Button::new(("group", index))
                    .ghost()
                    .small()
                    .w_full()
                    .justify_start()
                    .label(name.clone())
                    .selected(index == groups.active)
                    .on_click(move |_, _, cx| {
                        if let Err(error) = app::groups::switch(index, cx) {
                            app::show_error(error);
                        }
                    })
            }))
            .child(
                Button::new("new-group")
                    .outline()
                    .small()
                    .w_full()
                    .mt_2()
                    .label("New Group")
                    .on_click(|_, _, cx| {
                        if let Err(error) = app::groups::create(None, cx) {
                            app::show_error(error);
                        }
                    }),
            );
    }
}
