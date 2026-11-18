//! Pointer-delta toolbar dragging with a shared virtual handle anchor.
//!
//! A docking animation can move the toolbar far from the actual pointer. A
//! rebase attaches the logical drag to that new toolbar position, preserving
//! the original normalized grab fractions and the now-unbounded pointer offset.
//! Subsequent pointer deltas move the toolbar, rather than snapping it back.

use crate::{
    docking::{Bounds, Edge},
    ui::{ToolbarGeometry, ToolbarLayout},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragGeometry {
    toolbar: ToolbarGeometry,
    pointer_anchor: Point,
    handle_anchor: (f64, f64),
    grab_fraction: (f64, f64),
    left_handle_width: Option<f64>,
}

impl DragGeometry {
    /// Starts a drag from the toolbar's actual screen rectangle. Fractions are
    /// clamped only here; rebasing never derives them from an off-toolbar cursor.
    pub fn begin(
        pointer: Point,
        actual: Bounds,
        toolbar: ToolbarGeometry,
        scale: f32,
    ) -> Option<Self> {
        if !valid_bounds(actual) {
            return None;
        }
        let metrics = Metrics::scaled(toolbar, scale)?;
        let grab_fraction = (
            ((f64::from(pointer.x) - f64::from(actual.left)) / metrics.handle_width)
                .clamp(0.0, 1.0),
            ((f64::from(pointer.y) - f64::from(actual.top)) / metrics.handle_height)
                .clamp(0.0, 1.0),
        );
        Some(Self {
            toolbar,
            pointer_anchor: pointer,
            handle_anchor: (
                f64::from(actual.left) + grab_fraction.0 * metrics.handle_width,
                f64::from(actual.top) + grab_fraction.1 * metrics.handle_height,
            ),
            grab_fraction,
            left_handle_width: None,
        })
    }

    /// Returns the translated virtual grab location in physical screen pixels.
    /// Select a monitor using this point, rather than the potentially distant
    /// actual pointer after a docking animation. It is independent of the
    /// orientation and scale being projected and does not change the drag.
    pub fn handle_position(&self, pointer: Point) -> Option<Point> {
        let dx = (i64::from(pointer.x) - i64::from(self.pointer_anchor.x)) as f64;
        let dy = (i64::from(pointer.y) - i64::from(self.pointer_anchor.y)) as f64;
        let x = (self.handle_anchor.0 + dx).round();
        let y = (self.handle_anchor.1 + dy).round();
        if [x, y].into_iter().any(|value| {
            !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX)
        }) {
            return None;
        }
        Some(Point::new(x as i32, y as i32))
    }

    /// Proposes a rectangle without clamping it to a monitor. Both orientations
    /// use the same translated handle anchor. In particular, ask for Horizontal
    /// to decide whether the complete horizontal footprint fits while dragging
    /// a vertical toolbar; the real pointer may be far from either rectangle.
    pub fn proposed_bounds(
        &self,
        pointer: Point,
        layout: ToolbarLayout,
        scale: f32,
    ) -> Option<Bounds> {
        let metrics = Metrics::scaled(self.toolbar.with_layout(layout), scale)?;
        let dx = (i64::from(pointer.x) - i64::from(self.pointer_anchor.x)) as f64;
        let dy = (i64::from(pointer.y) - i64::from(self.pointer_anchor.y)) as f64;
        let handle_width = if layout == ToolbarLayout::Horizontal {
            self.left_handle_width.unwrap_or(metrics.handle_width)
        } else {
            metrics.handle_width
        };
        let left = (self.handle_anchor.0 + dx - self.grab_fraction.0 * handle_width).round();
        let top =
            (self.handle_anchor.1 + dy - self.grab_fraction.1 * metrics.handle_height).round();
        let right = left + f64::from(metrics.width);
        let bottom = top + f64::from(metrics.height);
        if [left, top, right, bottom].into_iter().any(|value| {
            !value.is_finite() || value < f64::from(i32::MIN) || value > f64::from(i32::MAX)
        }) {
            return None;
        }
        Some(Bounds::new(
            left as i32,
            top as i32,
            right as i32,
            bottom as i32,
        ))
    }

    /// Reattaches the drag to actual or intended layout bounds at this pointer
    /// sample. Use at animation start to follow its logical destination, and at
    /// completion with the actual final bounds to avoid applying movement twice.
    /// Also use after an intentional clamp or DPI resize. Invalid input leaves
    /// the existing geometry unchanged.
    pub fn rebase(
        &mut self,
        pointer: Point,
        actual: Bounds,
        layout: ToolbarLayout,
        scale: f32,
    ) -> bool {
        if !valid_bounds(actual) {
            return false;
        }
        let Some(metrics) = Metrics::scaled(self.toolbar.with_layout(layout), scale) else {
            return false;
        };
        self.pointer_anchor = pointer;
        self.handle_anchor = (
            f64::from(actual.left) + self.grab_fraction.0 * metrics.handle_width,
            f64::from(actual.top) + self.grab_fraction.1 * metrics.handle_height,
        );
        self.left_handle_width = match (layout, self.left_handle_width) {
            (ToolbarLayout::Vertical, Some(_)) => Some(metrics.handle_width),
            _ => None,
        };
        true
    }

    /// Align the left edge of the hypothetical horizontal footprint with a
    /// left-docked column. Different handle widths must not count as a pull.
    pub fn rebase_docked(
        &mut self,
        pointer: Point,
        actual: Bounds,
        scale: f32,
        edge: Edge,
    ) -> bool {
        if !self.rebase(pointer, actual, ToolbarLayout::Vertical, scale) {
            return false;
        }
        self.left_handle_width = if edge == Edge::Left {
            Metrics::scaled(self.toolbar.with_layout(ToolbarLayout::Vertical), scale)
                .map(|metrics| metrics.handle_width)
        } else {
            None
        };
        true
    }
}

fn valid_bounds(bounds: Bounds) -> bool {
    bounds.left < bounds.right && bounds.top < bounds.bottom
}

struct Metrics {
    width: i32,
    height: i32,
    handle_width: f64,
    handle_height: f64,
}

impl Metrics {
    fn scaled(layout: ToolbarGeometry, scale: f32) -> Option<Self> {
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let (width, height) = layout.size();
        let (handle_width, handle_height) = layout.handle_size();
        let scale = f64::from(scale);
        let width = (f64::from(width) * scale).round();
        let height = (f64::from(height) * scale).round();
        if !(1.0..=f64::from(i32::MAX)).contains(&width)
            || !(1.0..=f64::from(i32::MAX)).contains(&height)
        {
            return None;
        }
        Some(Self {
            width: width as i32,
            height: height as i32,
            handle_width: f64::from(handle_width) * scale,
            handle_height: f64::from(handle_height) * scale,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docking::choose_layout;
    use ToolbarLayout::{Horizontal, Vertical};

    const WORK: Bounds = Bounds::new(0, 0, 1920, 1040);
    const HORIZONTAL: Bounds = Bounds::new(600, 200, 1080, 260);
    const INITIAL_POINTER: Point = Point::new(618, 230);
    const DOCK_POINTER: Point = Point::new(1364, 230);
    const RIGHT_COLUMN: Bounds = Bounds::new(1860, 100, 1920, 580);

    #[test]
    fn left_docking_requires_a_real_pull_independent_of_handle_width_and_dpi() {
        for labels in [false, true] {
            let toolbar = ToolbarGeometry::new(
                Horizontal,
                &crate::toolbar_preferences::ToolbarButton::DEFAULT,
                labels,
                true,
            );
            for scale in [1.0_f32, 1.25, 1.5, 2.0] {
                let horizontal = Metrics::scaled(toolbar, scale).unwrap();
                let vertical = Metrics::scaled(toolbar.with_layout(Vertical), scale).unwrap();
                let work = Bounds::new(-1920, 0, 0, 2160);
                for grab in [0.0, 0.577_777_8, 0.977_777_8] {
                    let actual = Bounds::new(
                        -1000,
                        100,
                        -1000 + horizontal.width,
                        100 + horizontal.height,
                    );
                    let start = Point::new(
                        actual.left + (horizontal.handle_width * grab).round() as i32,
                        120,
                    );
                    let mut drag = DragGeometry::begin(start, actual, toolbar, scale).unwrap();
                    let pointer = Point::new(work.left, 150);
                    let column = Bounds::new(
                        work.left,
                        100,
                        work.left + vertical.width,
                        100 + vertical.height,
                    );
                    assert!(drag.rebase_docked(pointer, column, scale, Edge::Left));
                    let same = drag.proposed_bounds(pointer, Horizontal, scale).unwrap();
                    assert_eq!(same.left, work.left);
                    assert_eq!(choose_layout(Vertical, same, work, scale), Vertical);
                    assert_eq!(drag.proposed_bounds(pointer, Vertical, scale), Some(column));

                    let down = Point::new(pointer.x, pointer.y + 30);
                    let moved = drag.proposed_bounds(down, Horizontal, scale).unwrap();
                    assert_eq!(moved.left, work.left);
                    assert_eq!(moved.top, same.top + 30);
                    assert_eq!(choose_layout(Vertical, moved, work, scale), Vertical);

                    let release =
                        (crate::docking::RELEASE_DISTANCE_DIP * f64::from(scale)).ceil() as i32;
                    let before = drag
                        .proposed_bounds(
                            Point::new(pointer.x + release - 1, pointer.y),
                            Horizontal,
                            scale,
                        )
                        .unwrap();
                    assert_eq!(choose_layout(Vertical, before, work, scale), Vertical);
                    let after = drag
                        .proposed_bounds(
                            Point::new(pointer.x + release, pointer.y),
                            Horizontal,
                            scale,
                        )
                        .unwrap();
                    assert_eq!(choose_layout(Vertical, after, work, scale), Horizontal);
                }
            }
        }
    }

    #[test]
    fn a_new_drag_on_the_left_column_keeps_the_leading_reference() {
        let toolbar = ToolbarGeometry::new(
            Vertical,
            &crate::toolbar_preferences::ToolbarButton::DEFAULT,
            true,
            true,
        );
        let metrics = Metrics::scaled(toolbar, 1.0).unwrap();
        let column = Bounds::new(0, 100, metrics.width, 100 + metrics.height);
        for x in [1, 26, 44, 80] {
            let pointer = Point::new(x, 120);
            let mut drag = DragGeometry::begin(pointer, column, toolbar, 1.0).unwrap();
            drag.rebase_docked(pointer, column, 1.0, Edge::Left);
            for dx in [0, 1, 2, 7] {
                let proposal = drag
                    .proposed_bounds(Point::new(x + dx, 140), Horizontal, 1.0)
                    .unwrap();
                assert_eq!(proposal.left, dx);
                assert_eq!(choose_layout(Vertical, proposal, WORK, 1.0), Vertical);
            }
        }
    }

    #[test]
    fn physical_left_limit_after_a_right_slide_can_seed_a_reachable_left_reference() {
        let mut drag = horizontal_drag();
        let right_pointer = Point::new(1364, 120);
        drag.rebase_docked(right_pointer, RIGHT_COLUMN, 1.0, Edge::Right);
        let physical_limit = Point::new(WORK.left, 120);
        let unreachable = drag
            .proposed_bounds(physical_limit, Horizontal, 1.0)
            .unwrap();
        assert!(unreachable.left > WORK.left);

        let left_column = Bounds::new(0, 100, 60, 580);
        drag.rebase_docked(physical_limit, left_column, 1.0, Edge::Left);
        let proposal = drag
            .proposed_bounds(physical_limit, Horizontal, 1.0)
            .unwrap();
        assert_eq!(proposal.left, WORK.left);
        assert_eq!(choose_layout(Vertical, proposal, WORK, 1.0), Vertical);
        let next = Point::new(10, 120);
        assert_eq!(drag.proposed_bounds(next, Vertical, 1.0).unwrap().left, 10);
        let horizontal = drag.proposed_bounds(next, Horizontal, 1.0).unwrap();
        drag.rebase(next, horizontal, Horizontal, 1.0);
        assert_eq!(
            drag.proposed_bounds(next, Horizontal, 1.0),
            Some(horizontal)
        );
    }

    fn horizontal_drag() -> DragGeometry {
        DragGeometry::begin(
            INITIAL_POINTER,
            HORIZONTAL,
            ToolbarGeometry::default_for(Horizontal),
            1.0,
        )
        .unwrap()
    }

    #[test]
    fn ordinary_pointer_motion_moves_the_toolbar_by_exactly_the_same_delta() {
        let drag = horizontal_drag();
        assert_eq!(
            drag.proposed_bounds(INITIAL_POINTER, Horizontal, 1.0),
            Some(HORIZONTAL)
        );
        assert_eq!(
            drag.proposed_bounds(Point::new(628, 210), Horizontal, 1.0),
            Some(Bounds::new(610, 180, 1090, 240))
        );
    }

    #[test]
    fn both_layouts_project_from_one_shared_handle_without_mutating_the_drag() {
        let drag = horizontal_drag();
        assert_eq!(
            drag.proposed_bounds(INITIAL_POINTER, Vertical, 1.0),
            Some(Bounds::new(594, 208, 654, 688))
        );
        assert_eq!(
            drag.proposed_bounds(INITIAL_POINTER, Horizontal, 1.0),
            Some(HORIZONTAL)
        );
        assert_eq!(drag, horizontal_drag());
    }

    #[test]
    fn logical_handle_stays_with_the_column_when_the_physical_pointer_crosses_a_monitor_seam() {
        let mut drag = horizontal_drag();
        assert_eq!(drag.handle_position(INITIAL_POINTER), Some(INITIAL_POINTER));
        assert_eq!(
            drag.handle_position(Point::new(628, 210)),
            Some(Point::new(628, 210))
        );
        assert!(drag.rebase(DOCK_POINTER, RIGHT_COLUMN, Vertical, 1.0));
        assert_eq!(
            drag.handle_position(DOCK_POINTER),
            Some(Point::new(1884, 123))
        );
        let physical_pointer = Point::new(-6, DOCK_POINTER.y);
        assert_eq!(
            drag.proposed_bounds(physical_pointer, Vertical, 1.0),
            Some(Bounds::new(490, 100, 550, 580))
        );
        assert_eq!(
            drag.handle_position(physical_pointer),
            Some(Point::new(514, 123))
        );
        assert_eq!(
            drag.handle_position(Point::new(-16, DOCK_POINTER.y + 20)),
            Some(Point::new(504, 143))
        );
    }

    #[test]
    fn logical_handle_rejects_non_finite_and_unrepresentable_screen_positions() {
        let mut drag = horizontal_drag();
        assert!(drag.rebase(DOCK_POINTER, RIGHT_COLUMN, Vertical, 1.0));
        assert!(drag.handle_position(Point::new(i32::MAX, 0)).is_none());
        assert_eq!(
            drag.handle_position(Point::new(i32::MIN, 0)),
            Some(Point::new(i32::MIN + 520, -108))
        );
        drag.handle_anchor = (f64::NAN, 0.0);
        assert!(drag.handle_position(DOCK_POINTER).is_none());
        drag.handle_anchor = (0.0, f64::INFINITY);
        assert!(drag.handle_position(DOCK_POINTER).is_none());
    }

    #[test]
    fn off_toolbar_pointer_rebase_has_no_jump_and_a_small_pull_moves_the_column() {
        let mut drag = horizontal_drag();
        assert!(drag.rebase(DOCK_POINTER, RIGHT_COLUMN, Vertical, 1.0));
        assert_eq!(
            drag.proposed_bounds(DOCK_POINTER, Vertical, 1.0),
            Some(RIGHT_COLUMN)
        );
        let pulled = Point::new(DOCK_POINTER.x - 10, DOCK_POINTER.y);
        assert_eq!(
            drag.proposed_bounds(pulled, Vertical, 1.0),
            Some(Bounds::new(1850, 100, 1910, 580))
        );
        let projection = drag.proposed_bounds(pulled, Horizontal, 1.0).unwrap();
        assert_eq!(projection, Bounds::new(1856, 93, 2336, 153));
        assert_eq!(choose_layout(Vertical, projection, WORK, 1.0), Vertical);
    }

    #[test]
    fn unfolding_depends_on_actual_column_travel_instead_of_the_old_pointer_anchor() {
        let mut drag = horizontal_drag();
        let small_pull = Point::new(DOCK_POINTER.x - 10, DOCK_POINTER.y);
        // This old projection reproduces the premature unfolding bug.
        let old_projection = drag.proposed_bounds(small_pull, Horizontal, 1.0).unwrap();
        assert_eq!(
            choose_layout(Vertical, old_projection, WORK, 1.0),
            Horizontal
        );
        assert!(drag.rebase(DOCK_POINTER, RIGHT_COLUMN, Vertical, 1.0));
        let still_outside = Point::new(DOCK_POINTER.x - 425, DOCK_POINTER.y);
        let projection = drag
            .proposed_bounds(still_outside, Horizontal, 1.0)
            .unwrap();
        assert_eq!(projection.right, 1921);
        assert_eq!(choose_layout(Vertical, projection, WORK, 1.0), Vertical);
        let fully_inside = Point::new(DOCK_POINTER.x - 434, DOCK_POINTER.y);
        let projection = drag.proposed_bounds(fully_inside, Horizontal, 1.0).unwrap();
        assert_eq!(projection.right, 1912);
        assert_eq!(choose_layout(Vertical, projection, WORK, 1.0), Horizontal);
        assert_eq!(
            drag.proposed_bounds(fully_inside, Vertical, 1.0)
                .unwrap()
                .left,
            RIGHT_COLUMN.left - 434
        );
    }

    #[test]
    fn animation_target_motion_and_final_rebase_do_not_apply_a_delta_twice() {
        let mut drag = horizontal_drag();
        assert!(drag.rebase(DOCK_POINTER, RIGHT_COLUMN, Vertical, 1.0));
        let during_animation = Point::new(DOCK_POINTER.x - 100, DOCK_POINTER.y + 20);
        let target = drag
            .proposed_bounds(during_animation, Vertical, 1.0)
            .unwrap();
        assert_eq!(target, Bounds::new(1760, 120, 1820, 600));
        assert_eq!(
            choose_layout(
                Vertical,
                drag.proposed_bounds(during_animation, Horizontal, 1.0)
                    .unwrap(),
                WORK,
                1.0
            ),
            Vertical
        );
        assert!(drag.rebase(during_animation, target, Vertical, 1.0));
        assert_eq!(
            drag.proposed_bounds(during_animation, Vertical, 1.0),
            Some(target)
        );
        assert_eq!(
            drag.proposed_bounds(
                Point::new(during_animation.x - 10, during_animation.y),
                Vertical,
                1.0
            ),
            Some(Bounds::new(1750, 120, 1810, 600))
        );
    }

    #[test]
    fn negative_monitor_coordinates_and_fractional_dpi_preserve_physical_deltas() {
        let actual = Bounds::new(-1500, 100, -780, 190);
        let pointer = Point::new(-1473, 145);
        let mut drag = DragGeometry::begin(
            pointer,
            actual,
            ToolbarGeometry::default_for(Horizontal),
            1.5,
        )
        .unwrap();
        assert_eq!(drag.proposed_bounds(pointer, Horizontal, 1.5), Some(actual));
        let docked = Bounds::new(-90, 0, 0, 720);
        let far_pointer = Point::new(-900, 200);
        assert!(drag.rebase(far_pointer, docked, Vertical, 1.5));
        assert_eq!(
            drag.proposed_bounds(far_pointer, Vertical, 1.5),
            Some(docked)
        );
        assert_eq!(
            drag.proposed_bounds(Point::new(-910, 220), Vertical, 1.5),
            Some(Bounds::new(-100, 20, -10, 740))
        );
        assert_eq!(
            drag.handle_position(Point::new(-910, 220)),
            Some(Point::new(-64, 54))
        );
    }

    #[test]
    fn rebasing_after_clamping_discards_blocked_travel_without_losing_the_grab_fraction() {
        let mut drag = horizontal_drag();
        let pointer = Point::new(1500, 230);
        let clamped = Bounds::new(1440, 200, 1920, 260);
        assert!(drag.rebase(pointer, clamped, Horizontal, 1.0));
        assert_eq!(
            drag.proposed_bounds(pointer, Horizontal, 1.0),
            Some(clamped)
        );
        assert_eq!(
            drag.proposed_bounds(Point::new(1490, 230), Horizontal, 1.0),
            Some(Bounds::new(1430, 200, 1910, 260))
        );
        assert_eq!(drag.grab_fraction, (0.4, 0.5));
    }

    #[test]
    fn invalid_metrics_or_unrepresentable_positions_fail_without_changing_geometry() {
        let mut drag = horizontal_drag();
        for scale in [
            0.0,
            -1.0,
            f32::NAN,
            f32::INFINITY,
            f32::MAX,
            f32::MIN_POSITIVE,
        ] {
            assert!(
                DragGeometry::begin(
                    INITIAL_POINTER,
                    HORIZONTAL,
                    ToolbarGeometry::default_for(Horizontal),
                    scale
                )
                .is_none()
            );
            assert!(
                drag.proposed_bounds(INITIAL_POINTER, Horizontal, scale)
                    .is_none()
            );
            let before = drag;
            assert!(!drag.rebase(DOCK_POINTER, RIGHT_COLUMN, Vertical, scale));
            assert_eq!(drag, before);
        }
        let invalid = Bounds::new(0, 0, 0, 1);
        assert!(
            DragGeometry::begin(
                INITIAL_POINTER,
                invalid,
                ToolbarGeometry::default_for(Horizontal),
                1.0
            )
            .is_none()
        );
        let before = drag;
        assert!(!drag.rebase(DOCK_POINTER, invalid, Vertical, 1.0));
        assert_eq!(drag, before);
        assert!(
            drag.proposed_bounds(Point::new(i32::MAX, 0), Horizontal, 1.0)
                .is_none()
        );
        assert!(
            drag.proposed_bounds(Point::new(i32::MIN, 0), Horizontal, 1.0)
                .is_none()
        );
    }
}
