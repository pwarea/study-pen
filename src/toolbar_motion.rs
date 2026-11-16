//! Two-stage, horizontal-to-vertical toolbar docking in physical pixels.
//!
//! Layout changes only while the toolbar is fully outside the work area. The
//! host clips every presented frame to that area and owns drag cancellation.

use crate::{
    docking::{Bounds, Edge},
    ui::ToolbarLayout,
};
use std::time::{Duration, Instant};

pub const OUT_DURATION: Duration = Duration::from_millis(48);
pub const IN_DURATION: Duration = Duration::from_millis(48);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotionFrame {
    pub layout: ToolbarLayout,
    pub bounds: Bounds,
    pub finished: bool,
}

#[derive(Clone, Debug)]
pub struct DockAnimation {
    source: Bounds,
    target: Bounds,
    work: Bounds,
    edge: Edge,
    started: Instant,
}

impl DockAnimation {
    pub fn new(source: Bounds, target: Bounds, work: Bounds, edge: Edge, started: Instant) -> Self {
        Self {
            source,
            target,
            work,
            edge,
            started,
        }
    }

    /// Follows the current vertical destination without restarting either stage
    /// or feeding animated bounds back into the original horizontal movement.
    pub fn retarget(&mut self, target: Bounds) {
        self.target = target;
    }

    pub fn target(&self) -> Bounds {
        self.target
    }

    pub fn work(&self) -> Bounds {
        self.work
    }

    pub fn edge(&self) -> Edge {
        self.edge
    }

    pub fn frame(&self, now: Instant) -> MotionFrame {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed < OUT_DURATION {
            MotionFrame {
                layout: ToolbarLayout::Horizontal,
                bounds: interpolate_x(
                    self.source,
                    i64::from(self.source.left),
                    self.outside_left(self.source),
                    elapsed.as_secs_f64() / OUT_DURATION.as_secs_f64(),
                ),
                finished: false,
            }
        } else if elapsed < OUT_DURATION + IN_DURATION {
            MotionFrame {
                layout: ToolbarLayout::Vertical,
                bounds: interpolate_x(
                    self.target,
                    self.outside_left(self.target),
                    i64::from(self.target.left),
                    (elapsed - OUT_DURATION).as_secs_f64() / IN_DURATION.as_secs_f64(),
                ),
                finished: false,
            }
        } else {
            MotionFrame {
                layout: ToolbarLayout::Vertical,
                bounds: self.target,
                finished: true,
            }
        }
    }

    fn outside_left(&self, bounds: Bounds) -> i64 {
        match self.edge {
            Edge::Left => {
                i64::from(self.work.left) - (i64::from(bounds.right) - i64::from(bounds.left))
            }
            Edge::Right => i64::from(self.work.right),
        }
    }
}

fn interpolate_x(shape: Bounds, from: i64, to: i64, progress: f64) -> Bounds {
    let remaining = 1.0 - progress.clamp(0.0, 1.0);
    let eased = 1.0 - remaining * remaining * remaining;
    let left = (from as f64 + (to - from) as f64 * eased).round() as i64;
    let right = left + (i64::from(shape.right) - i64::from(shape.left));
    // Wide intermediates avoid overflow when a negative-origin monitor's edge
    // is crossed. Saturation covers unrepresentable desktop-extreme coordinates.
    let physical = |value: i64| value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    Bounds::new(physical(left), shape.top, physical(right), shape.bottom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ToolbarLayout::{Horizontal, Vertical};

    const WORK: Bounds = Bounds::new(0, 0, 1920, 1040);
    const SOURCE: Bounds = Bounds::new(1400, 300, 1880, 360);
    const TARGET: Bounds = Bounds::new(1860, 100, 1920, 580);

    fn at(started: Instant, milliseconds: u64) -> Instant {
        started + Duration::from_millis(milliseconds)
    }

    #[test]
    fn right_overhang_continues_into_the_slide_without_snapping_back_inside() {
        let started = Instant::now();
        let source = Bounds::new(1472, 300, 1952, 360);
        let animation = DockAnimation::new(source, TARGET, WORK, Edge::Right, started);
        assert_eq!(animation.frame(started).bounds, source);
        let next = animation.frame(at(started, 12));
        assert_eq!(next.layout, Horizontal);
        assert!(next.bounds.left > source.left);
        assert_eq!(animation.frame(at(started, 48)).layout, Vertical);
        assert_eq!(animation.frame(at(started, 96)).bounds, TARGET);
    }

    #[test]
    fn right_docking_changes_layout_fully_outside_and_finishes_at_the_exact_target() {
        let started = Instant::now();
        let animation = DockAnimation::new(SOURCE, TARGET, WORK, Edge::Right, started);
        assert_eq!(
            animation.frame(started),
            MotionFrame {
                layout: Horizontal,
                bounds: SOURCE,
                finished: false,
            }
        );
        assert_eq!(
            animation.frame(at(started, 48)),
            MotionFrame {
                layout: Vertical,
                bounds: Bounds::new(1920, 100, 1980, 580),
                finished: false,
            }
        );
        for milliseconds in [96, 97, 500] {
            assert_eq!(
                animation.frame(at(started, milliseconds)),
                MotionFrame {
                    layout: Vertical,
                    bounds: TARGET,
                    finished: true,
                }
            );
        }
        assert_eq!(animation.target(), TARGET);
        assert_eq!(animation.work(), WORK);
        assert_eq!(animation.edge(), Edge::Right);
    }

    #[test]
    fn cubic_ease_out_moves_only_x_and_preserves_each_stages_shape() {
        let started = Instant::now();
        let animation = DockAnimation::new(SOURCE, TARGET, WORK, Edge::Right, started);
        let outgoing = animation.frame(at(started, 24));
        assert_eq!(outgoing.layout, Horizontal);
        assert_eq!(outgoing.bounds, Bounds::new(1855, 300, 2335, 360));
        let incoming = animation.frame(at(started, 72));
        assert_eq!(incoming.layout, Vertical);
        assert_eq!(incoming.bounds, Bounds::new(1868, 100, 1928, 580));
        assert!(!outgoing.finished && !incoming.finished);
    }

    #[test]
    fn negative_left_edge_docking_is_monotonic_and_never_changes_stage_y_or_size() {
        let started = Instant::now();
        let work = Bounds::new(-2560, -120, 0, 1320);
        let source = Bounds::new(-2540, 250, -2060, 310);
        let target = Bounds::new(-2560, -80, -2500, 400);
        let animation = DockAnimation::new(source, target, work, Edge::Left, started);
        let mut previous = source.left;
        for milliseconds in 0..48 {
            let frame = animation.frame(at(started, milliseconds));
            assert_eq!(frame.layout, Horizontal);
            assert!(frame.bounds.left <= previous);
            assert_eq!(frame.bounds.top, source.top);
            assert_eq!(frame.bounds.bottom, source.bottom);
            assert_eq!(frame.bounds.right - frame.bounds.left, 480);
            previous = frame.bounds.left;
        }
        let outside = animation.frame(at(started, 48));
        assert_eq!(outside.bounds, Bounds::new(-2620, -80, -2560, 400));
        assert_eq!(outside.layout, Vertical);
        previous = outside.bounds.left;
        for milliseconds in 49..=96 {
            let frame = animation.frame(at(started, milliseconds));
            assert_eq!(frame.layout, Vertical);
            assert!(frame.bounds.left >= previous);
            assert_eq!(frame.bounds.top, target.top);
            assert_eq!(frame.bounds.bottom, target.bottom);
            assert_eq!(frame.bounds.right - frame.bounds.left, 60);
            previous = frame.bounds.left;
        }
        assert_eq!(animation.frame(at(started, 96)).bounds, target);
        assert_eq!(animation.edge(), Edge::Left);
    }

    #[test]
    fn retargeting_preserves_the_outgoing_path_and_original_completion_time() {
        let started = Instant::now();
        let mut animation = DockAnimation::new(SOURCE, TARGET, WORK, Edge::Right, started);
        let original_outgoing = animation.frame(at(started, 24));
        let updated = Bounds::new(1860, 400, 1920, 880);
        animation.retarget(updated);
        assert_eq!(animation.frame(started).bounds, SOURCE);
        assert_eq!(animation.frame(at(started, 24)), original_outgoing);
        assert_eq!(
            animation.frame(at(started, 48)).bounds,
            Bounds::new(1920, 400, 1980, 880)
        );
        let final_target = Bounds::new(1840, 200, 1900, 680);
        animation.retarget(final_target);
        assert_eq!(animation.target(), final_target);
        assert_eq!(
            animation.frame(at(started, 72)).bounds.top,
            final_target.top
        );
        assert_eq!(
            animation.frame(at(started, 96)),
            MotionFrame {
                layout: Vertical,
                bounds: final_target,
                finished: true,
            }
        );
        assert_eq!(animation.work(), WORK);
        assert_eq!(animation.edge(), Edge::Right);
    }

    #[test]
    fn time_before_start_keeps_the_exact_source_frame() {
        let now = Instant::now();
        let started = now + Duration::from_millis(10);
        let animation = DockAnimation::new(SOURCE, TARGET, WORK, Edge::Right, started);
        assert_eq!(animation.frame(now), animation.frame(started));
    }
}
