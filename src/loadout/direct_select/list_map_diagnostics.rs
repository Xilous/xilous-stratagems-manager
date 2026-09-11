use image::{Rgba, RgbaImage};
use tracing::{debug, warn};

use super::{GridPosition, ListMap};

const CELL_PADDING: u32 = 2;
const CELL_BORDER: Rgba<u8> = Rgba([64, 64, 64, 255]);
const SELECTED_BORDER: Rgba<u8> = Rgba([255, 220, 32, 255]);
const BACKGROUND: Rgba<u8> = Rgba([8, 8, 8, 255]);

impl Drop for ListMap {
    fn drop(&mut self) {
        let Some(image) = render(self) else {
            return;
        };
        match crate::vision::save_list_map_image(self.item_kind, &image) {
            Ok(path) => debug!(
                item_kind = self.item_kind.label(),
                path = %path.display(),
                mapped_slots = self.slot_luma.len(),
                "temporary list map image saved"
            ),
            Err(error) => warn!(
                item_kind = self.item_kind.label(),
                error = %error,
                "failed to save temporary list map image"
            ),
        }
    }
}

fn render(map: &ListMap) -> Option<RgbaImage> {
    let min_row = map.slot_luma.keys().map(|position| position.row).min()?;
    let max_row = map.slot_luma.keys().map(|position| position.row).max()?;
    let min_col = map.slot_luma.keys().map(|position| position.col).min()?;
    let max_col = map.slot_luma.keys().map(|position| position.col).max()?;
    let cell_width = map.slot_luma.values().map(|slot| slot.sample.width).max()? + 2 * CELL_PADDING;
    let cell_height = map
        .slot_luma
        .values()
        .map(|slot| slot.sample.height)
        .max()?
        + 2 * CELL_PADDING;
    let columns = max_col - min_col + 1;
    let rows = (max_row - min_row + 1) as u32;
    let mut image = RgbaImage::from_pixel(columns * cell_width, rows * cell_height, BACKGROUND);

    for (&position, mapped) in &map.slot_luma {
        let sample = &mapped.sample;
        let cell_x = (position.col - min_col) * cell_width;
        let cell_y = (position.row - min_row) as u32 * cell_height;
        let x = cell_x + (cell_width - sample.width) / 2;
        let y = cell_y + (cell_height - sample.height) / 2;
        for sample_y in 0..sample.height {
            for sample_x in 0..sample.width {
                let luma =
                    sample.pixels[sample_y as usize * sample.width as usize + sample_x as usize];
                image.put_pixel(x + sample_x, y + sample_y, Rgba([luma, luma, luma, 255]));
            }
        }
        let border = if map.selected.contains(&GridPosition {
            row: position.row,
            col: position.col,
        }) {
            SELECTED_BORDER
        } else {
            CELL_BORDER
        };
        draw_border(&mut image, cell_x, cell_y, cell_width, cell_height, border);
    }
    Some(image)
}

fn draw_border(image: &mut RgbaImage, x: u32, y: u32, width: u32, height: u32, color: Rgba<u8>) {
    for px in x..x + width {
        image.put_pixel(px, y, color);
        image.put_pixel(px, y + height - 1, color);
    }
    for py in y..y + height {
        image.put_pixel(x, py, color);
        image.put_pixel(x + width - 1, py, color);
    }
}
