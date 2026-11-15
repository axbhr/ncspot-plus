//! Pictures through the kitty graphics protocol. Each picture is sent once
//! and afterwards only placed, so many covers can be shown and moved cheaply.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Cursor, Write as _};
use std::path::PathBuf;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use cursive::Vec2;
use image::ImageFormat;
use image::imageops::FilterType;
use log::error;

const CHUNK: usize = 4096;
/// Pictures kept in the terminal before all of them are freed.
const MAX_IMAGES: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    pub path: PathBuf,
    /// Absolute cell position of the top left corner.
    pub offset: Vec2,
    /// Size in cells.
    pub size: Vec2,
}

/// A rectangle of cells as top left corner and size.
pub type Area = (Vec2, Vec2);

pub fn supported() -> bool {
    viuer::get_kitty_support() != viuer::KittySupport::None
}

pub struct Images {
    cell: Vec2,
    ids: HashMap<(PathBuf, Vec2), u32>,
    next_id: u32,
    rendered: Option<(Vec<Placement>, Option<Area>)>,
}

impl Images {
    /// `cell` is the size of one terminal cell in pixels.
    pub fn new(cell: Vec2) -> Self {
        Self {
            cell,
            ids: HashMap::new(),
            next_id: 1,
            rendered: None,
        }
    }

    /// Show exactly `desired` except within `hole`, writing only if it
    /// changed.
    pub fn show(&mut self, desired: &[Placement], hole: Option<Area>) {
        if self
            .rendered
            .as_ref()
            .is_some_and(|(rendered, h)| rendered == desired && *h == hole)
        {
            return;
        }

        // Save the cursor and remove all placements, keeping the pictures.
        let mut out = String::from("\x1b7\x1b_Ga=d,d=a,q=2\x1b\\");
        if self.ids.len() + desired.len() > MAX_IMAGES {
            out.push_str("\x1b_Ga=d,d=A,q=2\x1b\\");
            self.ids.clear();
        }
        let mut placement_id = 0;
        for placement in desired {
            let Some(id) = self.upload(placement, &mut out) else {
                continue;
            };
            let whole = (placement.offset, placement.size);
            for (offset, size) in uncovered(whole, hole) {
                placement_id += 1;
                let source = (offset - placement.offset) * self.cell;
                let pixels = size * self.cell;
                let _ = write!(
                    out,
                    // Below text and colored cells.
                    "\x1b[{};{}H\x1b_Ga=p,i={id},p={placement_id},x={},y={},w={},h={},c={},r={},C=1,z=-1073741825,q=2\x1b\\",
                    offset.y + 1,
                    offset.x + 1,
                    source.x,
                    source.y,
                    pixels.x,
                    pixels.y,
                    size.x,
                    size.y
                );
            }
        }
        out.push_str("\x1b8");

        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
        self.rendered = Some((desired.to_vec(), hole));
    }

    /// Forget what is on screen, e.g. after the terminal was cleared.
    pub fn invalidate(&mut self) {
        self.rendered = None;
    }

    fn upload(&mut self, placement: &Placement, out: &mut String) -> Option<u32> {
        let key = (placement.path.clone(), placement.size);
        if let Some(id) = self.ids.get(&key) {
            return Some(*id);
        }

        let width = (placement.size.x * self.cell.x) as u32;
        let height = (placement.size.y * self.cell.y) as u32;
        let image = image::ImageReader::open(&placement.path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .decode()
            .map_err(|e| error!("Failed to decode {}: {e}", placement.path.display()))
            .ok()?
            .resize_to_fill(width.max(1), height.max(1), FilterType::Triangle);
        let mut png = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .map_err(|e| error!("Failed to encode cover: {e}"))
            .ok()?;

        let id = self.next_id;
        self.next_id += 1;
        let data = STANDARD.encode(&png);
        let chunks: Vec<&str> = data
            .as_bytes()
            .chunks(CHUNK)
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
            .collect();
        for (i, chunk) in chunks.iter().enumerate() {
            let more = u8::from(i + 1 < chunks.len());
            if i == 0 {
                let _ = write!(out, "\x1b_Ga=t,f=100,i={id},q=2,m={more};{chunk}\x1b\\");
            } else {
                let _ = write!(out, "\x1b_Gm={more};{chunk}\x1b\\");
            }
        }
        self.ids.insert(key, id);
        Some(id)
    }
}

/// Parts of `area` outside of `hole`, at most four rectangles.
fn uncovered(area: Area, hole: Option<Area>) -> Vec<Area> {
    let (pos, size) = area;
    let end = pos + size;
    let Some((hole_pos, hole_size)) = hole else {
        return vec![area];
    };
    let start = pos.zip_map(hole_pos, usize::max);
    let stop = end.zip_map(hole_pos + hole_size, usize::min);
    if start.x >= stop.x || start.y >= stop.y {
        return vec![area];
    }

    [
        (pos, Vec2::new(size.x, start.y - pos.y)),
        (Vec2::new(pos.x, stop.y), Vec2::new(size.x, end.y - stop.y)),
        (
            Vec2::new(pos.x, start.y),
            Vec2::new(start.x - pos.x, stop.y - start.y),
        ),
        (
            Vec2::new(stop.x, start.y),
            Vec2::new(end.x - stop.x, stop.y - start.y),
        ),
    ]
    .into_iter()
    .filter(|(_, size)| size.x > 0 && size.y > 0)
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: usize, y: usize, w: usize, h: usize) -> Area {
        (Vec2::new(x, y), Vec2::new(w, h))
    }

    #[test]
    fn uncovered_keeps_areas_outside_the_hole() {
        assert_eq!(uncovered(area(0, 0, 4, 2), None), vec![area(0, 0, 4, 2)]);
        let hole = Some(area(10, 10, 5, 5));
        assert_eq!(uncovered(area(0, 0, 4, 2), hole), vec![area(0, 0, 4, 2)]);
    }

    #[test]
    fn uncovered_cuts_around_the_hole() {
        let parts = uncovered(area(0, 0, 10, 10), Some(area(3, 4, 4, 2)));
        assert_eq!(
            parts,
            vec![
                area(0, 0, 10, 4),
                area(0, 6, 10, 4),
                area(0, 4, 3, 2),
                area(7, 4, 3, 2),
            ]
        );
        assert!(uncovered(area(4, 4, 2, 2), Some(area(0, 0, 10, 10))).is_empty());
    }
}
