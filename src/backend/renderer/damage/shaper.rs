use std::mem;

use crate::utils::{Physical, Rectangle, Size};

/// The recommended minimum tile side.
const DEFAULT_MIN_TILE_SIDE: i32 = 16;

/// The maximum ratio of the largest damage rectangle to the current damage bbox.
const MAX_DAMAGE_TO_DAMAGE_BBOX_RATIO: f32 = 0.9;

/// The gap between tiles damage to merge tiles into one.
const TILE_DAMAGE_GAP_FRACTION: i32 = 4;

/// State of the damage shaping.
#[derive(Debug, Default)]
pub struct DamageShaper<const MIN_TILE_SIDE: i32 = DEFAULT_MIN_TILE_SIDE> {
    /// Cache for tiles to avoid re-allocation.
    tiles_cache: Vec<Tile>,
    /// The damage accumulated during shaping.
    out_damage: Vec<Rectangle<i32, Physical>>,
}

impl<const MIN_TILE_SIDE: i32> DamageShaper<MIN_TILE_SIDE> {
    /// Shape damage rectangles.
    #[profiling::function]
    pub fn shape_damage(&mut self, in_damage: &mut Vec<Rectangle<i32, Physical>>) {
        self.out_damage.clear();
        self.tiles_cache.clear();

        // Call the implementation without direction.
        self.shape_damage_impl(in_damage, None, false);

        // The shaped damage is inside of `out_damage`, so swap it with `in_damage` since
        // it's irrelevant.
        mem::swap(&mut self.out_damage, in_damage);
    }

    // A divide and conquer hybrid damage shaping algorithm.
    //
    // The key idea is to split rectangles by non-overlapping segments on dominating axis (largest
    // side of the damage bounding box).
    //
    // When damage is overlapping through the entire `in_damage` span we shape damage using the
    // _tile_ based shaper, where the damage bounding box is split into tiles and damage is being
    // computed for each tile individually by walking all the current damage rectangles.
    #[inline(never)]
    fn shape_damage_impl(
        &mut self,
        in_damage: &mut [Rectangle<i32, Physical>],
        last_direction: Option<DamageSplitAxis>,
        invert_direction: bool,
    ) {
        // Optimize small damage input.
        if in_damage.is_empty() {
            return;
        } else if in_damage.len() == 1 {
            self.out_damage.push(in_damage[0]);
            return;
        }

        // Compute the effective damage bounding box and the maximum damaged area rectangle.

        let (x_min, y_min, x_max, y_max, max_damage_area) =
            in_damage
                .iter()
                .fold((i32::MAX, i32::MAX, i32::MIN, i32::MIN, i32::MIN), |acc, rect| {
                    let area = rect.size.w * rect.size.h;
                    (
                        acc.0.min(rect.loc.x),
                        acc.1.min(rect.loc.y),
                        acc.2.max(rect.loc.x + rect.size.w),
                        acc.3.max(rect.loc.y + rect.size.h),
                        acc.4.max(area),
                    )
                });

        let bbox_w = x_max - x_min;
        let bbox_h = y_max - y_min;

        let damage_bbox = Rectangle::<i32, Physical>::from_loc_and_size((x_min, y_min), (bbox_w, bbox_h));

        // Damage the current bounding box when there's a damage rect covering near all the area.
        if max_damage_area as f32 / (damage_bbox.size.w * damage_bbox.size.h) as f32
            > MAX_DAMAGE_TO_DAMAGE_BBOX_RATIO
        {
            self.out_damage.push(damage_bbox);
            return;
        }

        // Now we try to split bounding box to process non-overlapping damage rects separately.
        //
        // The whole approach is recursive and splits viewport if and only if we have a gap
        // in the current segment or rectangles touch each other, since the renderer excludes
        // borders.
        //
        // Examples:
        //      [0, 3], [1, 2] [2, 3] [3, 4]
        //      will have a split at ^
        //      [0, 3], [1, 2] [2, 3] [6, 10]
        //      will have a split at ^
        //
        //  Resulting in recursively trying to shape damage before and after
        //  split point.

        let mut direction = if bbox_w >= bbox_h {
            DamageSplitAxis::X
        } else {
            DamageSplitAxis::Y
        };

        if invert_direction {
            direction = direction.invert();
        }

        // The coordinate where the first rectangle ends and where the potential overlap
        // will end.
        let mut overlap_end = match direction {
            DamageSplitAxis::X => {
                if Some(direction) != last_direction {
                    in_damage.sort_unstable_by(|lhs, rhs| {
                        // Sort ascending by X and then descending by width, so when multiple
                        // rectangles with the same X are present the first will overlap the most.
                        lhs.loc.x.cmp(&rhs.loc.x).then(rhs.size.w.cmp(&lhs.size.w))
                    });
                }
                in_damage[0].loc.x + in_damage[0].size.w
            }
            DamageSplitAxis::Y => {
                if Some(direction) != last_direction {
                    in_damage.sort_unstable_by(|lhs, rhs| {
                        // Sort ascending by Y and then descending by height, so when multiple
                        // rectangles with the same Y are present the first will overlap the most.
                        lhs.loc.y.cmp(&rhs.loc.y).then(rhs.size.h.cmp(&lhs.size.h))
                    });
                }
                in_damage[0].loc.y + in_damage[0].size.h
            }
        };

        // The start of overlap.
        let mut overlap_start_idx = 0;
        for idx in overlap_start_idx + 1..in_damage.len() {
            let rect = in_damage[idx];
            let (rect_start, rect_end) = match direction {
                DamageSplitAxis::X => (rect.loc.x, rect.loc.x + rect.size.w),
                DamageSplitAxis::Y => (rect.loc.y, rect.loc.y + rect.size.h),
            };

            // NOTE the renderer excludes the boundary, otherwise we need `>`.
            if rect_start >= overlap_end {
                self.shape_damage_impl(&mut in_damage[overlap_start_idx..idx], Some(direction), false);

                // Advance the overlap.
                overlap_start_idx = idx;
                overlap_end = rect_end;
            } else {
                overlap_end = overlap_end.max(rect_end);
            }
        }

        // When rectangle covers the entire bounding box and we've tried different direction of
        // splitting perform the tiled based shaping.
        if overlap_start_idx == 0 && invert_direction {
            // We pick more steps for edges which don't have full overlap.
            const NUM_TILES: i32 = 4;
            // NOTE we need to revert direction back to use larger side preferences.
            let (tile_w, tile_h) = match direction.invert() {
                DamageSplitAxis::X => (bbox_w / NUM_TILES, bbox_h / (NUM_TILES * 2)),
                DamageSplitAxis::Y => (bbox_w / (NUM_TILES * 2), bbox_h / NUM_TILES),
            };
            let tile_size = (tile_w.max(MIN_TILE_SIDE), tile_h.max(MIN_TILE_SIDE)).into();

            self.shape_damage_tiled(in_damage, damage_bbox, tile_size);
        } else {
            self.shape_damage_impl(
                &mut in_damage[overlap_start_idx..],
                Some(direction),
                overlap_start_idx == 0,
            );
        }
    }

    #[inline]
    fn shape_damage_tiled(
        &mut self,
        in_damage: &[Rectangle<i32, Physical>],
        bbox: Rectangle<i32, Physical>,
        tile_size: Size<i32, Physical>,
    ) {
        self.tiles_cache.clear();
        let tile_gap: Size<i32, Physical> = From::from((
            tile_size.w / TILE_DAMAGE_GAP_FRACTION,
            tile_size.h / TILE_DAMAGE_GAP_FRACTION,
        ));

        for x in (bbox.loc.x..bbox.loc.x + bbox.size.w).step_by(tile_size.w as usize) {
            let mut tiles_in_column = 0;
            for y in (bbox.loc.y..bbox.loc.y + bbox.size.h).step_by(tile_size.h as usize) {
                // NOTE the in_damage is constrained to the `bbox`, so it can't go outside
                // the tile, even though some tiles could go outside the `bbox`.
                let bbox = Rectangle::<i32, Physical>::from_loc_and_size((x, y), tile_size);
                let mut tile = Tile {
                    bbox,
                    damage: None,
                    #[cfg(test)]
                    merged_tiles: 0,
                };

                // Intersect damage regions with the given tile bounding box.
                for damage_rect in in_damage.iter() {
                    if let Some(intersection) = tile.bbox.intersection(*damage_rect) {
                        tile.damage = if let Some(tile_damage) = tile.damage {
                            Some(intersection.merge(tile_damage))
                        } else {
                            Some(intersection)
                        };
                    }
                }

                tiles_in_column += 1;
                self.tiles_cache.push(tile);
            }

            // Try to reduce amount of damage by merging adjacent tiles.
            let num_tiles = self.tiles_cache.len();
            for idx in num_tiles - tiles_in_column..num_tiles - 1 {
                let (damage, adjacent_damage) =
                    match (self.tiles_cache[idx].damage, self.tiles_cache[idx + 1].damage) {
                        (Some(damage), Some(adjacent_damage)) => (damage, adjacent_damage),
                        _ => continue,
                    };

                if damage.loc.y + damage.size.h + tile_gap.h >= adjacent_damage.loc.y
                    && (damage.size.w - adjacent_damage.size.w).abs() < tile_gap.w
                {
                    self.tiles_cache[idx].damage = None;
                    self.tiles_cache[idx + 1].damage = Some(damage.merge(adjacent_damage));
                    #[cfg(test)]
                    {
                        self.tiles_cache[idx + 1].merged_tiles = self.tiles_cache[idx].merged_tiles + 1;
                    }
                }
            }
        }

        self.out_damage
            .extend(self.tiles_cache.iter().filter_map(|tile| tile.damage));
    }
}

/// Tile with the damage tracking information.
#[derive(Debug)]
struct Tile {
    /// Bounding box for the given tile.
    bbox: Rectangle<i32, Physical>,
    /// The accumulated damage in the tile.
    damage: Option<Rectangle<i32, Physical>>,
    // To ensure that damage is constrained in tests.
    #[cfg(test)]
    merged_tiles: i32,
}

/// Direction to split damage bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DamageSplitAxis {
    /// Split damage bounding box on x axis.
    X,
    /// Split damage bounding box on y axis.
    Y,
}

impl DamageSplitAxis {
    /// Invert the split direction.
    fn invert(self) -> Self {
        match self {
            DamageSplitAxis::X => DamageSplitAxis::Y,
            DamageSplitAxis::Y => DamageSplitAxis::X,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shaper() -> DamageShaper {
        DamageShaper::default()
    }

    // Check that tiles align and not overlap.
    fn check_tiles(tiles: &[Tile]) {
        let mut xs: Vec<i32> = tiles.iter().map(|tile| tile.bbox.loc.x).collect();
        let rows = xs.iter().filter(|&&x| tiles[0].bbox.loc.x == x).count();
        xs.dedup();
        let cols = xs.len();

        let mut bbox = tiles[0].bbox;
        for col in 0..cols {
            for row in 0..rows {
                let tile = &tiles[col * rows + row];
                assert_eq!(bbox, tile.bbox);
                if let Some(damage) = tile.damage {
                    // Ensure that all tile damage is within them.
                    assert!(tile.bbox.loc.x <= damage.loc.x && damage.size.w <= tile.bbox.size.w);
                    assert!(
                        tile.bbox.loc.y - tile.bbox.size.h * tile.merged_tiles <= damage.loc.y
                            && damage.size.h <= tile.bbox.loc.y + tile.bbox.size.h
                    );
                }
                bbox.loc.y += bbox.size.h;
            }
            bbox.loc.y = tiles[0].bbox.loc.y;
            bbox.loc.x += bbox.size.w;
        }
    }

    fn damage_area(damage: &[Rectangle<i32, Physical>]) -> i32 {
        let mut area = 0;
        for rect in damage {
            area += rect.size.w * rect.size.h;
        }
        area
    }

    #[test]
    fn tile_shaping() {
        let mut damage = vec![
            Rectangle::<i32, Physical>::from_loc_and_size((98, 406), (36, 48)),
            Rectangle::<i32, Physical>::from_loc_and_size((158, 502), (828, 168)),
            Rectangle::<i32, Physical>::from_loc_and_size((122, 694), (744, 528)),
            Rectangle::<i32, Physical>::from_loc_and_size((194, 1318), (420, 72)),
            Rectangle::<i32, Physical>::from_loc_and_size((146, 1414), (312, 48)),
            Rectangle::<i32, Physical>::from_loc_and_size((32, 406), (108, 1152)),
        ];

        let mut shaper = shaper();
        let mut to_shape = damage.clone();
        shaper.shape_damage(&mut to_shape);
        check_tiles(&shaper.tiles_cache);

        // Re-shaping shouldn't trigger tile based algorithm and it should result in the same
        // damage, given that end damage is not overlapping.
        let mut to_shape_again = to_shape.clone();
        shaper.shape_damage(&mut to_shape_again);
        to_shape_again.sort_by_key(|rect| rect.loc.x);
        to_shape.sort_by_key(|rect| rect.loc.x);
        assert_eq!(to_shape, to_shape_again);
        assert!(shaper.tiles_cache.is_empty());

        // Having big chunk of damage shouldn't trigger tiling.
        let bbox = Rectangle::<i32, Physical>::from_loc_and_size((0, 0), (3840, 2160));
        damage.push(bbox);
        shaper.shape_damage(&mut damage);
        assert_eq!(damage[0], bbox);
        assert!(shaper.tiles_cache.is_empty());
    }

    #[test]
    fn small_damage() {
        let mut shaper = shaper();
        let mut damage = vec![];
        shaper.shape_damage(&mut damage);
        assert!(damage.is_empty());

        let rect = Rectangle::<i32, Physical>::from_loc_and_size((0, 0), (5, 5));
        damage.push(rect);
        shaper.shape_damage(&mut damage);
        assert!(damage.len() == 1);
        assert_eq!(damage[0], rect);
    }

    #[test]
    fn shape_pixels() {
        let mut shaper = shaper();
        let mut damage = vec![];
        shaper.shape_damage(&mut damage);
        assert!(damage.is_empty());

        let w = 384;
        let h = 216;

        for x in 0..w {
            for y in 0..h {
                let rect = Rectangle::<i32, Physical>::from_loc_and_size((x, y), (1, 1));
                damage.push(rect);
            }
        }

        let mut to_shape = damage.clone();
        shaper.shape_damage(&mut to_shape);
        assert!(shaper.tiles_cache.is_empty());
        assert_eq!(damage_area(&to_shape), w * h);
        to_shape.sort_by_key(|rect| rect.loc.x);
        assert_eq!(damage, to_shape);

        let w1 = 216;
        let h1 = 144;
        let overlap1 = Rectangle::<i32, Physical>::from_loc_and_size((0, 0), (w1, h1));
        let overlap2 = Rectangle::<i32, Physical>::from_loc_and_size((w1, h1), (w - w1, h - h1));
        damage.push(overlap1);
        damage.push(overlap2);

        shaper.shape_damage(&mut damage);
        assert!(shaper.tiles_cache.is_empty());
        assert_eq!(damage_area(&damage), w * h);
        assert!(damage.contains(&overlap1));
        assert!(damage.contains(&overlap2));
    }
}
