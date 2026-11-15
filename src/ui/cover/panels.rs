//! The list columns next to the cover: queue and recently played.

use std::path::PathBuf;

use cursive::theme::{Color, ColorStyle, ColorType};
use cursive::{Printer, Vec2};

use super::images::Placement;

/// Thumbnail size in cells, roughly square with 1:2 cells.
const THUMB: (usize, usize) = (4, 2);

/// A two-line entry of a column.
pub struct Entry {
    pub title: String,
    pub subtitle: String,
    pub right: String,
    pub cover_url: Option<String>,
    pub playing: bool,
    /// Already played, drawn dimmed.
    pub played: bool,
}

pub struct Column<'a> {
    pub header: String,
    /// First entry shown, the caller keeps the selection visible.
    pub first: usize,
    /// Selected entry while the column has focus.
    pub selected: Option<usize>,
    /// A labelled divider drawn before the entry at this index.
    pub divider: Option<(usize, &'a str)>,
    pub accent: Color,
    pub thumbnails: bool,
}

/// Number of entries that fit into a column of `height` rows.
pub fn rows(height: usize) -> usize {
    height.saturating_sub(2) / 2
}

/// First entry to show: `anchor`, moved just enough to keep `selected`
/// within the `rows` visible entries.
pub fn scroll(selected: Option<usize>, anchor: usize, rows: usize) -> usize {
    match selected {
        Some(selected) if selected < anchor => selected,
        Some(selected) if rows > 0 && selected >= anchor + rows => selected + 1 - rows,
        _ => anchor,
    }
}

/// Draw `entries` below the header, starting at `column.first`.
/// Thumbnails are added to `placements`; `cover_path` gives the downloaded
/// file of a cover URL once it is available.
pub fn draw(
    printer: &Printer<'_, '_>,
    entries: &[Entry],
    column: &Column<'_>,
    cover_path: &dyn Fn(&str) -> Option<PathBuf>,
    placements: &mut Vec<Placement>,
) {
    let width = printer.size.x;
    let height = printer.size.y;
    printer.with_color(ColorStyle::title_primary(), |p| {
        p.cropped((width, 1)).print((0, 0), &column.header);
    });

    if rows(height) == 0 {
        return;
    }
    let first = column.first;
    let text_x = if column.thumbnails { THUMB.0 + 1 } else { 0 };
    let accent = ColorStyle::new(ColorType::Color(column.accent), ColorType::InheritParent);

    let mut y = 2;
    for (index, entry) in entries.iter().enumerate().skip(first) {
        if y + 1 >= height {
            break;
        }
        if let Some((at, label)) = column.divider
            && at == index
            && index > first
        {
            printer.with_color(ColorStyle::secondary(), |p| {
                p.print_hline((0, y), width, "─");
                p.print((2, y), &format!(" {label} "));
            });
            y += 1;
            if y + 1 >= height {
                break;
            }
        }

        if column.thumbnails
            && let Some(path) = entry.cover_url.as_deref().and_then(cover_path)
        {
            placements.push(Placement {
                path,
                offset: printer.offset + (0, y),
                size: Vec2::new(THUMB.0, THUMB.1),
            });
        }

        let selected = column.selected == Some(index);
        let style = if selected {
            ColorStyle::highlight()
        } else if entry.playing {
            accent
        } else if entry.played {
            ColorStyle::secondary()
        } else {
            ColorStyle::primary()
        };
        let marker = if entry.playing { "▶ " } else { "  " };
        let right_width = entry.right.chars().count();
        let title_width = width.saturating_sub(right_width + 1);

        printer.with_color(style, |p| {
            if selected {
                p.print_hline((text_x, y), width - text_x, " ");
                p.print_hline((text_x, y + 1), width - text_x, " ");
            }
            p.cropped((title_width, y + 1))
                .print((text_x, y), &format!("{marker}{}", entry.title));
            p.print((width.saturating_sub(right_width), y), &entry.right);
        });
        let subtitle_style = if selected {
            style
        } else {
            ColorStyle::secondary()
        };
        printer.with_color(subtitle_style, |p| {
            p.cropped((width, y + 2))
                .print((text_x + 2, y + 1), &entry.subtitle);
        });
        y += 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_keeps_anchor_while_selection_is_visible() {
        assert_eq!(scroll(None, 3, 5), 3);
        assert_eq!(scroll(Some(5), 3, 5), 3);
    }

    #[test]
    fn scroll_follows_selection_out_of_view() {
        assert_eq!(scroll(Some(1), 3, 5), 1);
        assert_eq!(scroll(Some(9), 3, 5), 5);
    }
}
