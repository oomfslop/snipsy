use crate::app::config;
use anyhow::{Result, ensure};
use gpui_kit::App;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolbarItem {
    Record,
    Select,
    Shape,
    Arrow,
    Pen,
    Highlighter,
    Text,
    Blur,
    Eraser,
    Undo,
    Redo,
    Layers,
    Search,
    Print,
    Save,
    Pin,
    Cancel,
    Copy,
    Separator,
}

impl ToolbarItem {
    pub const ALL: [Self; 19] = [
        Self::Record,
        Self::Select,
        Self::Shape,
        Self::Arrow,
        Self::Pen,
        Self::Highlighter,
        Self::Text,
        Self::Blur,
        Self::Eraser,
        Self::Undo,
        Self::Redo,
        Self::Layers,
        Self::Search,
        Self::Print,
        Self::Save,
        Self::Pin,
        Self::Cancel,
        Self::Copy,
        Self::Separator,
    ];
}

pub const DEFAULT_ITEMS: [ToolbarItem; 21] = [
    ToolbarItem::Record,
    ToolbarItem::Separator,
    ToolbarItem::Select,
    ToolbarItem::Shape,
    ToolbarItem::Arrow,
    ToolbarItem::Pen,
    ToolbarItem::Highlighter,
    ToolbarItem::Text,
    ToolbarItem::Blur,
    ToolbarItem::Eraser,
    ToolbarItem::Separator,
    ToolbarItem::Undo,
    ToolbarItem::Redo,
    ToolbarItem::Layers,
    ToolbarItem::Separator,
    ToolbarItem::Search,
    ToolbarItem::Print,
    ToolbarItem::Save,
    ToolbarItem::Pin,
    ToolbarItem::Cancel,
    ToolbarItem::Copy,
];

pub fn validate(items: &[ToolbarItem]) -> Result<()> {
    ensure!(
        items.iter().enumerate().all(|(index, item)| {
            *item == ToolbarItem::Separator || !items[index + 1..].contains(item)
        }),
        "Toolbar items must be unique"
    );
    return Ok(());
}

pub fn set_items(items: Vec<ToolbarItem>, cx: &mut App) -> Result<()> {
    validate(&items)?;
    return config::update(cx, |config| config.toolbar = items);
}

pub fn reset(cx: &mut App) -> Result<()> {
    return set_items(DEFAULT_ITEMS.to_vec(), cx);
}

/// Drops separators that would sit at either end or next to another separator once hidden
/// items are removed.
pub fn get_visible_items(
    items: &[ToolbarItem],
    is_hidden: impl Fn(ToolbarItem) -> bool,
) -> Vec<ToolbarItem> {
    let mut visible = Vec::new();
    for item in items.iter().copied().filter(|item| !is_hidden(*item)) {
        if item == ToolbarItem::Separator
            && visible
                .last()
                .is_none_or(|last| *last == ToolbarItem::Separator)
        {
            continue;
        }
        visible.push(item);
    }
    if visible.last() == Some(&ToolbarItem::Separator) {
        visible.pop();
    }
    return visible;
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_ITEMS, ToolbarItem, get_visible_items};

    #[test]
    fn hidden_items_do_not_leave_stray_separators() {
        use ToolbarItem::*;
        assert_eq!(
            get_visible_items(&[Record, Separator, Pen, Separator], |item| item == Record),
            [Pen]
        );
        assert_eq!(
            get_visible_items(&[Pen, Separator, Record, Separator, Copy], |item| {
                item == Record
            }),
            [Pen, Separator, Copy]
        );
        assert_eq!(
            get_visible_items(&DEFAULT_ITEMS, |_| false),
            DEFAULT_ITEMS.to_vec()
        );
    }
}
