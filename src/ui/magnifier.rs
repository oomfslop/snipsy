use crate::{geometry::Color, ui};
use glam::{Vec2, vec2};
use gpui_kit::{component::ActiveTheme, *};
use image::RgbaImage;
use std::sync::Arc;

const CELL: f32 = 8.0;
const COLUMNS: i32 = 15;
const ROWS: i32 = 11;
const LABEL_HEIGHT: f32 = 40.0;
const OFFSET: f32 = 20.0;

const SIZE: Vec2 = vec2(
    COLUMNS as f32 * CELL + 2.0,
    ROWS as f32 * CELL + LABEL_HEIGHT,
);

#[derive(Clone, Copy)]
pub struct Sample {
    pub x: u32,
    pub y: u32,
    pub color: Color,
}

pub fn sample_pixel(image: &RgbaImage, x: u32, y: u32) -> Option<Sample> {
    let pixel = image.get_pixel_checked(x, y)?;
    return Some(Sample {
        x,
        y,
        color: Color::from_rgb(pixel[0], pixel[1], pixel[2]),
    });
}

pub fn get_position(pointer: Vec2, container: Vec2) -> Vec2 {
    let mut position = pointer + OFFSET;
    if position.x + SIZE.x > container.x {
        position.x = pointer.x - SIZE.x - OFFSET;
    }
    if position.y + SIZE.y > container.y {
        position.y = pointer.y - SIZE.y - OFFSET;
    }
    return position.clamp(Vec2::ZERO, (container - SIZE).max(Vec2::ZERO));
}

pub fn render(
    image: Arc<RgbaImage>,
    sample: Sample,
    position: Vec2,
    rgb_label: bool,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let grid = theme.border;
    let center_column = COLUMNS / 2;
    let center_row = ROWS / 2;
    return ui::panel(cx)
        .absolute()
        .left(px(position.x))
        .top(px(position.y))
        .w(px(SIZE.x))
        .overflow_hidden()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let cell = |column: i32, row: i32| {
                        Bounds::new(
                            bounds.origin + point(px(column as f32 * CELL), px(row as f32 * CELL)),
                            size(px(CELL), px(CELL)),
                        )
                    };
                    for row in 0..ROWS {
                        for column in 0..COLUMNS {
                            let x = sample.x as i64 + (column - center_column) as i64;
                            let y = sample.y as i64 + (row - center_row) as i64;
                            if let Ok(x) = u32::try_from(x)
                                && let Ok(y) = u32::try_from(y)
                                && let Some(pixel) = image.get_pixel_checked(x, y)
                            {
                                window.paint_quad(fill(
                                    cell(column, row),
                                    rgb(Color::from_rgb(pixel[0], pixel[1], pixel[2]).0),
                                ));
                            }
                        }
                    }
                    let mut crosshair = grid;
                    crosshair.a = 0.5;
                    window.paint_quad(fill(
                        Bounds::new(
                            cell(center_column, 0).origin,
                            size(px(CELL), bounds.size.height),
                        ),
                        crosshair,
                    ));
                    window.paint_quad(fill(
                        Bounds::new(
                            cell(0, center_row).origin,
                            size(bounds.size.width, px(CELL)),
                        ),
                        crosshair,
                    ));
                    let center = cell(center_column, center_row);
                    window.paint_quad(fill(center, rgb(sample.color.0)));
                    window.paint_quad(outline(center, black(), BorderStyle::Solid));
                    window.paint_quad(outline(
                        Bounds::new(
                            center.origin + point(px(1.), px(1.)),
                            size(px(CELL - 2.), px(CELL - 2.)),
                        ),
                        white(),
                        BorderStyle::Solid,
                    ));
                },
            )
            .w(px(COLUMNS as f32 * CELL))
            .h(px(ROWS as f32 * CELL)),
        )
        .child(
            div()
                .h(px(LABEL_HEIGHT))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .font_family(theme.mono_font_family.clone())
                .text_xs()
                .child(format!("{}, {}", sample.x, sample.y))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .size(px(10.))
                                .rounded_xs()
                                .bg(rgb(sample.color.0))
                                .border_1()
                                .border_color(theme.border),
                        )
                        .child(sample.color.format(rgb_label)),
                ),
        );
}
