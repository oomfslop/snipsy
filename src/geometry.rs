use glam::Vec2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub pos: Vec2,
    pub size: Vec2,
}

impl Rect {
    pub fn intersects(&self, point: Vec2) -> bool {
        return point.cmpge(self.pos).all() && point.cmple(self.pos + self.size).all();
    }

    pub fn flip_vertically(self, height: f32) -> Self {
        return Self {
            pos: Vec2::new(self.pos.x, height - self.pos.y - self.size.y),
            size: self.size,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::Rect;
    use glam::{Vec2, vec2};

    #[test]
    fn display_coordinates_include_menu_bar_and_negative_origins() {
        let primary = Rect {
            pos: Vec2::ZERO,
            size: vec2(1440.0, 900.0),
        };
        assert_eq!(primary.flip_vertically(900.0), primary);
        let above = Rect {
            pos: vec2(-1920.0, -1080.0),
            size: vec2(1920.0, 1080.0),
        };
        assert_eq!(above.flip_vertically(900.0).pos, vec2(-1920.0, 900.0));
        assert_eq!(above.flip_vertically(900.0).flip_vertically(900.0), above);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const WHITE: Self = Self(0xffffff);

    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        return Self((r as u32) << 16 | (g as u32) << 8 | b as u32);
    }
}
