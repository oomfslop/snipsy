use crate::{
    app::{
        self,
        cli::SnipOptions,
        config::{self, Config},
        hotkeys::{self, CustomSnipHotkey},
    },
    capture,
};
use glam::UVec2;
use gpui_kit::{
    component::{
        ActiveTheme, Disableable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{InputEvent, InputState, NumberInput},
        menu::{DropdownMenu as _, PopupMenuItem},
        v_flex,
    },
    prelude::*,
    *,
};

const SIZE_PRESETS: [(u32, u32); 8] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 800),
    (1440, 900),
    (1920, 1080),
    (2560, 1440),
];

#[derive(Clone, Copy, PartialEq)]
pub enum Target {
    Capture,
    Shortcut(Option<usize>),
}

pub struct CustomSnipView {
    target: Target,
    focus: FocusHandle,
    use_position: bool,
    use_size: bool,
    use_delay: bool,
    x: Entity<InputState>,
    y: Entity<InputState>,
    width: Entity<InputState>,
    height: Entity<InputState>,
    delay: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl CustomSnipView {
    pub fn new(target: Target, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let config = cx.global::<Config>();
        let options = match target {
            Target::Capture => config.custom_snip,
            Target::Shortcut(index) => index
                .and_then(|index| config.hotkeys.custom.get(index))
                .map(|custom| custom.snip)
                .unwrap_or_default(),
        };
        let position = options.position.unwrap_or(UVec2::ZERO);
        let size = options.size.unwrap_or(UVec2::new(800, 600));
        let delay = options.delay.unwrap_or(3.0);
        let mut subscriptions = Vec::new();
        let mut create_input = |value: String,
                                configure: fn(InputState) -> InputState,
                                enable: fn(&mut Self),
                                cx: &mut Context<Self>| {
            let input = cx.new(|cx| configure(InputState::new(window, cx).default_value(value)));
            subscriptions.push(cx.subscribe_in(
                &input,
                window,
                move |view, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        enable(view);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => view.confirm(window, cx),
                    _ => {}
                },
            ));
            input
        };
        let coordinate = |input: InputState| {
            input
                .min(0.0)
                .validate(|value, _| value.parse::<u32>().is_ok())
        };
        let length = |input: InputState| {
            input
                .min(1.0)
                .validate(|value, _| value.parse::<u32>().is_ok())
        };
        let enable_position = |view: &mut Self| view.use_position = true;
        let enable_size = |view: &mut Self| view.use_size = true;
        let x = create_input(position.x.to_string(), coordinate, enable_position, cx);
        let y = create_input(position.y.to_string(), coordinate, enable_position, cx);
        let width = create_input(size.x.to_string(), length, enable_size, cx);
        let height = create_input(size.y.to_string(), length, enable_size, cx);
        let delay = create_input(
            format!("{delay:.1}"),
            |input| {
                input
                    .min(0.0)
                    .max(SnipOptions::MAX_DELAY as f64)
                    .step(0.5)
                    .validate(|value, _| {
                        value
                            .parse::<f32>()
                            .is_ok_and(|delay| (0.0..=SnipOptions::MAX_DELAY).contains(&delay))
                    })
            },
            |view| view.use_delay = true,
            cx,
        );
        let focus = cx.focus_handle();
        window.focus(&delay.focus_handle(cx), cx);
        return Self {
            target,
            focus,
            use_position: options.position.is_some(),
            use_size: options.size.is_some(),
            use_delay: options.delay.is_some(),
            x,
            y,
            width,
            height,
            delay,
            _subscriptions: subscriptions,
        };
    }

    fn get_options(&self, cx: &App) -> Option<SnipOptions> {
        let parse = |input: &Entity<InputState>| input.read(cx).value().trim().parse::<u32>().ok();
        let read_pair = |enabled: bool, x, y| -> Option<Option<UVec2>> {
            if !enabled {
                return Some(None);
            }
            return Some(Some(UVec2::new(parse(x)?, parse(y)?)));
        };
        let delay = if self.use_delay {
            Some(self.delay.read(cx).value().trim().parse::<f32>().ok()?)
        } else {
            None
        };
        let options = SnipOptions {
            position: read_pair(self.use_position, &self.x, &self.y)?,
            size: read_pair(self.use_size, &self.width, &self.height)?,
            delay,
        };
        return options.validate().is_ok().then_some(options);
    }

    fn set_size(&mut self, size: (u32, u32), window: &mut Window, cx: &mut Context<Self>) {
        for (input, value) in [(&self.width, size.0), (&self.height, size.1)] {
            input.update(cx, |input, cx| {
                input.set_value(value.to_string(), window, cx)
            });
        }
        self.use_size = true;
        cx.notify();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(options) = self.get_options(cx) else {
            return;
        };
        let result = match self.target {
            Target::Capture => config::update(cx, |config| config.custom_snip = options),
            Target::Shortcut(index) => hotkeys::update_settings(cx, |settings| {
                match index.and_then(|index| settings.custom.get_mut(index)) {
                    Some(custom) => custom.snip = options,
                    None => settings.custom.push(CustomSnipHotkey {
                        hotkey: None,
                        snip: options,
                    }),
                }
            }),
        };
        if let Err(error) = result {
            app::show_error(error);
            if self.target != Target::Capture {
                return;
            }
        }
        window.remove_window();
        if self.target == Target::Capture {
            cx.defer(move |cx| capture::start(options, cx));
        }
    }

    fn render_row(
        &self,
        id: &'static str,
        label: &'static str,
        checked: bool,
        toggle: fn(&mut Self, bool),
        cx: &mut Context<Self>,
    ) -> Div {
        return h_flex().gap_2().child(
            div()
                .w(px(96.))
                .child(
                    Checkbox::new(id)
                        .label(label)
                        .checked(checked)
                        .on_click(cx.listener(move |view, checked: &bool, _, cx| {
                            toggle(view, *checked);
                            cx.notify();
                        })),
                ),
        );
    }
}

impl Render for CustomSnipView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let number = |input: &Entity<InputState>| NumberInput::new(input).small().w(px(104.));
        let separator = || div().text_color(muted).child("×");
        return v_flex()
            .id("custom-snip")
            .map(crate::ui::handle_window_actions)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            }))
            .size_full()
            .p_5()
            .gap_3()
            .bg(theme.background)
            .text_sm()
            .child(
                self.render_row(
                    "use-position",
                    "Top left",
                    self.use_position,
                    |view, checked| view.use_position = checked,
                    cx,
                )
                .child(number(&self.x))
                .child(separator())
                .child(number(&self.y)),
            )
            .child(
                self.render_row(
                    "use-size",
                    "Size",
                    self.use_size,
                    |view, checked| view.use_size = checked,
                    cx,
                )
                .child(number(&self.width))
                .child(separator())
                .child(number(&self.height))
                .child(
                    Button::new("size-preset")
                        .small()
                        .outline()
                        .label("Preset")
                        .dropdown_caret(true)
                        .dropdown_menu(move |menu, window, _| {
                            SIZE_PRESETS.into_iter().fold(menu, |menu, size| {
                                menu.item(
                                    PopupMenuItem::new(format!("{} × {}", size.0, size.1))
                                        .on_click(window.listener_for(
                                            &view,
                                            move |view, _, window, cx| {
                                                view.set_size(size, window, cx)
                                            },
                                        )),
                                )
                            })
                        }),
                ),
            )
            .child(
                self.render_row(
                    "use-delay",
                    "Delay",
                    self.use_delay,
                    |view, checked| view.use_delay = checked,
                    cx,
                )
                .child(number(&self.delay))
                .child(
                    div()
                        .text_color(muted)
                        .child(format!("seconds, up to {}", SnipOptions::MAX_DELAY)),
                ),
            )
            .child(
                h_flex()
                    .mt_auto()
                    .justify_end()
                    .gap_2()
                    .child(
                        Button::new("cancel")
                            .label("Cancel")
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .child(
                        Button::new("confirm")
                            .primary()
                            .label("OK")
                            .disabled(self.get_options(cx).is_none())
                            .on_click(cx.listener(|view, _, window, cx| view.confirm(window, cx))),
                    ),
            );
    }
}
