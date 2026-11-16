//! Drag-time toolbar orientation from a stable, unclamped horizontal footprint.
//!
//! The host proposes the same horizontal rectangle from pointer position and a
//! fixed drag grab reference in both orientations. Feeding the actual rotated
//! or clamped HWND bounds back into this policy would create layout feedback.
//! The host owns monitor selection, positioning, sizing and persistence.

use crate::ui::ToolbarLayout;

pub const ENTER_DISTANCE_DIP: f64 = 0.0;
pub const RIGHT_OVERHANG_DIP: f64 = 32.0;
pub const RELEASE_DISTANCE_DIP: f64 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
}

/// Physical screen-pixel bounds, including negative monitor coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    fn is_valid(self) -> bool {
        self.left < self.right && self.top < self.bottom
    }
}

/// Selects the side for a docking transition, independently of actual animated
/// window bounds. A crossed edge takes precedence over an inside gap; when
/// both signed gaps are equal, the left edge wins deterministically. Invalid
/// rectangles return `None`, allowing the host to retain its current edge.
pub fn nearest_edge(proposed_horizontal: Bounds, monitor_work_area: Bounds) -> Option<Edge> {
    let (from_left, from_right) = side_gaps(proposed_horizontal, monitor_work_area)?;
    Some(if from_left <= from_right {
        Edge::Left
    } else {
        Edge::Right
    })
}

fn side_gaps(proposed: Bounds, work: Bounds) -> Option<(i64, i64)> {
    if !proposed.is_valid() || !work.is_valid() {
        return None;
    }
    Some((
        i64::from(proposed.left) - i64::from(work.left),
        i64::from(work.right) - i64::from(proposed.right),
    ))
}

/// Chooses orientation from the proposed horizontal toolbar's side gaps.
///
/// The left side docks on overflow. The right side allows a short horizontal
/// overhang before docking. Exact contact stays horizontal. Release layout when
/// both ends fit inside with an eight-logical-pixel margin, reduced as needed
/// so a narrow work area can still fit the full horizontal toolbar. A perfectly
/// full-width fit has zero release margin and remains horizontal afterward.
/// Top and bottom proximity do not trigger docking.
///
/// `proposed_horizontal` must be the unclamped horizontal footprint even when
/// `current` is vertical. Its dimensions, grab reference and scale must not
/// depend on the current layout. Invalid geometry or scale leaves it unchanged.
pub fn choose_layout(
    current: ToolbarLayout,
    proposed_horizontal: Bounds,
    monitor_work_area: Bounds,
    scale: f32,
) -> ToolbarLayout {
    if !scale.is_finite() || scale <= 0.0 {
        return current;
    }
    let Some((from_left, from_right)) = side_gaps(proposed_horizontal, monitor_work_area) else {
        return current;
    };
    match current {
        ToolbarLayout::Horizontal => {
            let entry = ENTER_DISTANCE_DIP * f64::from(scale);
            let right_overhang = RIGHT_OVERHANG_DIP * f64::from(scale);
            if (from_left as f64) < entry || (from_right as f64) <= -right_overhang {
                ToolbarLayout::Vertical
            } else {
                ToolbarLayout::Horizontal
            }
        }
        ToolbarLayout::Vertical => {
            let work_width = i64::from(monitor_work_area.right) - i64::from(monitor_work_area.left);
            let proposed_width =
                i64::from(proposed_horizontal.right) - i64::from(proposed_horizontal.left);
            // Coordinates are whole pixels. Round the available half-margin
            // down so an odd number of spare pixels can still allow a fit.
            let available_margin = ((work_width - proposed_width).max(0) / 2) as f64;
            let margin = (RELEASE_DISTANCE_DIP * f64::from(scale)).min(available_margin);
            if from_left as f64 >= margin && from_right as f64 >= margin {
                ToolbarLayout::Horizontal
            } else {
                ToolbarLayout::Vertical
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ToolbarLayout::{Horizontal, Vertical};

    const PRIMARY: Bounds = Bounds::new(0, 0, 1920, 1040);
    const MIDDLE: Bounds = Bounds::new(600, 200, 1222, 276);

    fn horizontal_at(left: i32) -> Bounds {
        Bounds::new(left, 200, left + 622, 276)
    }

    #[test]
    fn right_entry_waits_for_visible_overhang_and_left_entry_remains_immediate() {
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(-1), PRIMARY, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(0), PRIMARY, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(1), PRIMARY, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(1299), PRIMARY, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(1298), PRIMARY, 1.0),
            Horizontal
        );
        assert_eq!(choose_layout(Horizontal, MIDDLE, PRIMARY, 1.0), Horizontal);
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(1329), PRIMARY, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(1330), PRIMARY, 1.0),
            Vertical
        );
    }

    #[test]
    fn long_toolbar_docks_after_a_short_overhang_while_the_pointer_is_still_far_away() {
        let proposed = horizontal_at(1330);
        let pointer_x = proposed.left + 64;
        assert_eq!(proposed.right - PRIMARY.right, 32);
        assert!(PRIMARY.right - pointer_x > 500);
        assert_eq!(choose_layout(Horizontal, proposed, PRIMARY, 1.0), Vertical);
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(1340), PRIMARY, 1.0),
            Vertical
        );
    }

    #[test]
    fn vertical_releases_at_the_exact_eight_pixel_margin() {
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(4), PRIMARY, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(4), PRIMARY, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(0), PRIMARY, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(-1), PRIMARY, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(7), PRIMARY, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(8), PRIMARY, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(1291), PRIMARY, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(1290), PRIMARY, 1.0),
            Horizontal
        );
    }

    #[test]
    fn stable_horizontal_footprint_prevents_rotation_and_release_feedback() {
        let crossed = horizontal_at(1330);
        let mut layout = choose_layout(Horizontal, crossed, PRIMARY, 1.0);
        assert_eq!(layout, Vertical);
        assert_eq!(choose_layout(layout, crossed, PRIMARY, 1.0), Vertical);
        let released = horizontal_at(1290);
        layout = choose_layout(layout, released, PRIMARY, 1.0);
        assert_eq!(layout, Horizontal);
        assert_eq!(choose_layout(layout, released, PRIMARY, 1.0), Horizontal);
    }

    #[test]
    fn negative_monitor_origins_support_contact_and_release_on_either_side() {
        let monitor = Bounds::new(-2560, -120, 0, 1320);
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(-2561), monitor, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(-2560), monitor, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(-621), monitor, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(-622), monitor, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(-591), monitor, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(-590), monitor, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(-630), monitor, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(-629), monitor, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(-1700), PRIMARY, 1.0),
            Vertical
        );
    }

    #[test]
    fn dpi_scales_the_right_overhang_and_the_small_release_margin() {
        let monitor = Bounds::new(-3840, 0, 0, 2160);
        let proposal = |gap: i32| Bounds::new(-gap - 1244, 400, -gap, 552);
        assert_eq!(
            choose_layout(Horizontal, proposal(-1), monitor, 2.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, proposal(0), monitor, 2.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, proposal(-63), monitor, 2.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, proposal(-64), monitor, 2.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Horizontal, proposal(-47), monitor, 1.5),
            Horizontal
        );
        assert_eq!(
            choose_layout(Horizontal, proposal(-48), monitor, 1.5),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, proposal(15), monitor, 2.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, proposal(16), monitor, 2.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Vertical, proposal(11), monitor, 1.5),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, proposal(12), monitor, 1.5),
            Horizontal
        );
    }

    #[test]
    fn adaptive_margin_preserves_narrow_and_full_width_fits_without_oscillation() {
        let narrow = Bounds::new(0, 0, 634, 1040);
        assert_eq!(
            choose_layout(Vertical, horizontal_at(5), narrow, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(6), narrow, 1.0),
            Horizontal
        );
        let full_width = Bounds::new(0, 0, 622, 1040);
        let exact = horizontal_at(0);
        let released = choose_layout(Vertical, exact, full_width, 1.0);
        assert_eq!(released, Horizontal);
        assert_eq!(choose_layout(released, exact, full_width, 1.0), Horizontal);
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(-1), full_width, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(-1), full_width, 1.0),
            Vertical
        );
        let too_narrow = Bounds::new(0, 0, 621, 1040);
        assert_eq!(choose_layout(Vertical, exact, too_narrow, 1.0), Vertical);
    }

    #[test]
    fn odd_spare_pixel_counts_still_allow_a_complete_horizontal_fit() {
        let work = Bounds::new(0, 0, 637, 1040);
        assert_eq!(
            choose_layout(Vertical, horizontal_at(7), work, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(8), work, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Vertical, horizontal_at(6), work, 1.0),
            Vertical
        );
    }

    #[test]
    fn top_and_bottom_contact_do_not_trigger_vertical_layout() {
        for proposal in [
            Bounds::new(600, -40, 1222, 36),
            Bounds::new(600, 1020, 1222, 1096),
        ] {
            assert_eq!(
                choose_layout(Horizontal, proposal, PRIMARY, 1.0),
                Horizontal
            );
            assert_eq!(choose_layout(Vertical, proposal, PRIMARY, 1.0), Horizontal);
        }
    }

    #[test]
    fn overshoots_and_extreme_coordinates_do_not_overflow() {
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(-500), PRIMARY, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(Horizontal, horizontal_at(2500), PRIMARY, 1.0),
            Vertical
        );
        assert_eq!(
            choose_layout(
                Horizontal,
                Bounds::new(i32::MIN, 0, i32::MIN + 622, 76),
                PRIMARY,
                1.0
            ),
            Vertical
        );
        assert_eq!(
            choose_layout(
                Horizontal,
                Bounds::new(i32::MAX - 622, 0, i32::MAX, 76),
                PRIMARY,
                1.0
            ),
            Vertical
        );
        let entire_range = Bounds::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        assert_eq!(
            choose_layout(Horizontal, MIDDLE, entire_range, 1.0),
            Horizontal
        );
        assert_eq!(
            choose_layout(Vertical, MIDDLE, entire_range, 1.0),
            Horizontal
        );
    }

    #[test]
    fn invalid_geometry_or_dpi_keeps_the_existing_layout() {
        for invalid in [
            Bounds::new(5, 0, 5, 10),
            Bounds::new(10, 0, 5, 10),
            Bounds::new(0, 8, 10, 8),
        ] {
            assert_eq!(choose_layout(Horizontal, MIDDLE, invalid, 1.0), Horizontal);
            assert_eq!(choose_layout(Vertical, MIDDLE, invalid, 1.0), Vertical);
            assert_eq!(choose_layout(Horizontal, invalid, PRIMARY, 1.0), Horizontal);
            assert_eq!(choose_layout(Vertical, invalid, PRIMARY, 1.0), Vertical);
            assert_eq!(nearest_edge(MIDDLE, invalid), None);
            assert_eq!(nearest_edge(invalid, PRIMARY), None);
        }
        for invalid_scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                choose_layout(Horizontal, horizontal_at(0), PRIMARY, invalid_scale),
                Horizontal
            );
            assert_eq!(
                choose_layout(Vertical, MIDDLE, PRIMARY, invalid_scale),
                Vertical
            );
        }
    }

    #[test]
    fn transition_side_matches_contact_overshoot_and_negative_monitors() {
        assert_eq!(nearest_edge(horizontal_at(0), PRIMARY), Some(Edge::Left));
        assert_eq!(
            nearest_edge(horizontal_at(1298), PRIMARY),
            Some(Edge::Right)
        );
        assert_eq!(nearest_edge(horizontal_at(-50), PRIMARY), Some(Edge::Left));
        assert_eq!(
            nearest_edge(horizontal_at(1350), PRIMARY),
            Some(Edge::Right)
        );
        assert_eq!(nearest_edge(horizontal_at(649), PRIMARY), Some(Edge::Left));
        let negative_monitor = Bounds::new(-2560, -120, 0, 1320);
        assert_eq!(
            nearest_edge(horizontal_at(-2540), negative_monitor),
            Some(Edge::Left)
        );
        assert_eq!(
            nearest_edge(horizontal_at(-640), negative_monitor),
            Some(Edge::Right)
        );
        assert_eq!(
            nearest_edge(Bounds::new(i32::MIN, 0, i32::MIN + 622, 76), PRIMARY),
            Some(Edge::Left)
        );
        assert_eq!(
            nearest_edge(Bounds::new(i32::MAX - 622, 0, i32::MAX, 76), PRIMARY),
            Some(Edge::Right)
        );
    }
}
