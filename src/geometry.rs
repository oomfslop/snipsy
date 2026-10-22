use glam::{Vec2, vec2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub pos: Vec2,
    pub size: Vec2,
}

impl Rect {
    pub fn from_corners(a: Vec2, b: Vec2) -> Self {
        return Self {
            pos: a.min(b),
            size: (a - b).abs(),
        };
    }

    pub fn end(&self) -> Vec2 {
        return self.pos + self.size;
    }

    pub fn contains(&self, point: Vec2) -> bool {
        return point.cmpge(self.pos).all() && point.cmple(self.end()).all();
    }

    pub fn union(self, other: Self) -> Self {
        return Self::from_corners(self.pos.min(other.pos), self.end().max(other.end()));
    }

    pub fn flip_vertically(self, height: f32) -> Self {
        return Self {
            pos: vec2(self.pos.x, height - self.pos.y - self.size.y),
            size: self.size,
        };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u32);

impl Color {
    pub const WHITE: Self = Self(0xffffff);

    pub const fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        return Self((r as u32) << 16 | (g as u32) << 8 | b as u32);
    }

    pub fn format(self, rgb: bool) -> String {
        if !rgb {
            return format!("#{:06x}", self.0);
        }
        return format!(
            "{}, {}, {}",
            self.0 >> 16 & 255,
            self.0 >> 8 & 255,
            self.0 & 255
        );
    }
}
