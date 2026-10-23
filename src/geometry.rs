use glam::{Vec2, vec2};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Edges closer than this merge, so nearly aligned rects don't leave slivers in their union.
const UNION_SNAP: f32 = 0.5;

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

/// Outlines of the union of `rects`, each a clockwise polygon on screen without collinear
/// vertices. Holes come out as their own contours, so fill them with the even-odd rule.
pub fn get_union_contours(rects: &[Rect]) -> Vec<Vec<Vec2>> {
    let create_grid = |values: Vec<f32>| {
        let mut values = values;
        values.sort_by(f32::total_cmp);
        values.dedup_by(|value, previous| *value - *previous < UNION_SNAP);
        values
    };
    let xs = create_grid(
        rects
            .iter()
            .flat_map(|rect| [rect.pos.x, rect.end().x])
            .collect(),
    );
    let ys = create_grid(
        rects
            .iter()
            .flat_map(|rect| [rect.pos.y, rect.end().y])
            .collect(),
    );
    let get_index = |values: &[f32], value: f32| {
        values
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| (*a - value).abs().total_cmp(&(*b - value).abs()))
            .map_or(0, |(index, _)| index)
    };
    let (columns, rows) = (xs.len().saturating_sub(1), ys.len().saturating_sub(1));
    let mut filled = vec![false; columns * rows];
    for rect in rects {
        for x in get_index(&xs, rect.pos.x)..get_index(&xs, rect.end().x) {
            for y in get_index(&ys, rect.pos.y)..get_index(&ys, rect.end().y) {
                filled[y * columns + x] = true;
            }
        }
    }
    let is_filled = |x: isize, y: isize| {
        x >= 0
            && y >= 0
            && (x as usize) < columns
            && (y as usize) < rows
            && filled[y as usize * columns + x as usize]
    };
    let mut edges: HashMap<(isize, isize), Vec<(isize, isize)>> = HashMap::new();
    for y in 0..rows as isize {
        for x in 0..columns as isize {
            if !is_filled(x, y) {
                continue;
            }
            for (neighbor, from, to) in [
                ((x, y - 1), (x, y), (x + 1, y)),
                ((x + 1, y), (x + 1, y), (x + 1, y + 1)),
                ((x, y + 1), (x + 1, y + 1), (x, y + 1)),
                ((x - 1, y), (x, y + 1), (x, y)),
            ] {
                if !is_filled(neighbor.0, neighbor.1) {
                    edges.entry(from).or_default().push(to);
                }
            }
        }
    }
    let mut contours = Vec::new();
    while let Some(&start) = edges.keys().next() {
        let mut points = vec![start];
        let mut current = start;
        let mut direction = (0, 0);
        while let Some(targets) = edges.get_mut(&current) {
            // Where two regions touch at a corner, turning right keeps them separate.
            let index = (0..targets.len())
                .max_by_key(|index| {
                    let next = (targets[*index].0 - current.0, targets[*index].1 - current.1);
                    direction.0 * next.1 - direction.1 * next.0
                })
                .unwrap();
            let next = targets.swap_remove(index);
            if targets.is_empty() {
                edges.remove(&current);
            }
            direction = ((next.0 - current.0).signum(), (next.1 - current.1).signum());
            current = next;
            if current == start {
                break;
            }
            points.push(current);
        }
        let count = points.len();
        let corners = (0..count)
            .filter(|index| {
                let (previous, point, next) = (
                    points[(index + count - 1) % count],
                    points[*index],
                    points[(index + 1) % count],
                );
                (point.0 - previous.0).signum() != (next.0 - point.0).signum()
                    || (point.1 - previous.1).signum() != (next.1 - point.1).signum()
            })
            .map(|index| vec2(xs[points[index].0 as usize], ys[points[index].1 as usize]))
            .collect::<Vec<_>>();
        if corners.len() >= 4 {
            contours.push(corners);
        }
    }
    return contours;
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
