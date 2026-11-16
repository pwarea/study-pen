//! Vector ink, continuous erasing, and gesture-sized undo transactions.
//!
//! This module is independent of windowing and rendering. Coordinates and widths
//! are in the same units; the host converts between screen pixels and logical
//! pixels when appropriate.

use std::collections::VecDeque;

const MAX_UNDO_BYTES: usize = 8 * 1024 * 1024;
const MAX_UNDO_STEPS: usize = 64;
const EPSILON: f32 = 0.0001;
const PARAMETER_EPSILON: f32 = f32::EPSILON;
const POINT_SPACING: f32 = 0.35;

pub const MIN_ERASER_DIAMETER: f32 = 6.0;
pub const MAX_ERASER_DIAMETER: f32 = 240.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    fn distance_squared(self, other: Self) -> f32 {
        (self.x - other.x).powi(2) + (self.y - other.y).powi(2)
    }

    fn interpolate(self, other: Self, t: f32) -> Self {
        Self::new(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Accepts RGB or RRGGBB with an optional leading hash.
    pub fn from_hex(value: &str) -> Option<Self> {
        let text = value.trim().strip_prefix('#').unwrap_or(value.trim());
        if !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        match text.len() {
            3 => {
                let value = u16::from_str_radix(text, 16).ok()?;
                Some(Self::rgb(
                    (((value >> 8) & 0xf) * 17) as u8,
                    (((value >> 4) & 0xf) * 17) as u8,
                    ((value & 0xf) * 17) as u8,
                ))
            }
            6 => {
                let value = u32::from_str_radix(text, 16).ok()?;
                Some(Self::rgb(
                    (value >> 16) as u8,
                    (value >> 8) as u8,
                    value as u8,
                ))
            }
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Pen,
    Eraser,
    ObjectEraser,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl Bounds {
    fn at(point: Point) -> Self {
        Self {
            min_x: point.x,
            min_y: point.y,
            max_x: point.x,
            max_y: point.y,
        }
    }

    fn include(&mut self, point: Point) {
        self.min_x = self.min_x.min(point.x);
        self.min_y = self.min_y.min(point.y);
        self.max_x = self.max_x.max(point.x);
        self.max_y = self.max_y.max(point.y);
    }

    fn expanded(self, radius: f32) -> Self {
        Self {
            min_x: self.min_x - radius,
            min_y: self.min_y - radius,
            max_x: self.max_x + radius,
            max_y: self.max_y + radius,
        }
    }

    fn overlaps(self, other: Self) -> bool {
        self.min_x <= other.max_x
            && self.max_x >= other.min_x
            && self.min_y <= other.max_y
            && self.max_y >= other.min_y
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub points: Vec<Point>,
    pub color: Color,
    pub width: f32,
    bounds: Bounds,
}

impl Stroke {
    fn new(point: Point, color: Color, width: f32) -> Self {
        Self {
            points: vec![point],
            color,
            width,
            bounds: Bounds::at(point),
        }
    }

    fn from_points(points: Vec<Point>, color: Color, width: f32) -> Self {
        debug_assert!(!points.is_empty());
        let mut bounds = Bounds::at(points[0]);
        for point in &points[1..] {
            bounds.include(*point);
        }
        Self {
            points,
            color,
            width,
            bounds,
        }
    }

    pub fn bounds(&self) -> Bounds {
        self.bounds.expanded(self.width * 0.5)
    }

    fn append(&mut self, point: Point) {
        self.points.push(point);
        self.bounds.include(point);
    }

    fn memory_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.points.capacity() * std::mem::size_of::<Point>()
    }
}

#[derive(Debug)]
enum Undo {
    RemoveLast,
    Restore(Vec<Stroke>),
}

impl Undo {
    fn memory_bytes(&self) -> usize {
        match self {
            Self::RemoveLast => std::mem::size_of::<Self>(),
            Self::Restore(strokes) => scene_bytes(strokes),
        }
    }
}

#[derive(Debug)]
enum Gesture {
    Pen(Stroke),
    Erase {
        tool: Tool,
        previous: Point,
        radius: f32,
        before: Option<Vec<Stroke>>,
    },
}

/// A scene and its undo history. One pointer gesture always produces one undo
/// step. Undo history is bounded by both step count and allocated point memory.
#[derive(Debug, Default)]
pub struct Ink {
    strokes: Vec<Stroke>,
    gesture: Option<Gesture>,
    undo: VecDeque<Undo>,
    undo_bytes: usize,
}

impl Ink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn strokes(&self) -> &[Stroke] {
        &self.strokes
    }

    pub fn active_stroke(&self) -> Option<&Stroke> {
        match &self.gesture {
            Some(Gesture::Pen(stroke)) => Some(stroke),
            _ => None,
        }
    }

    pub fn is_drawing(&self) -> bool {
        self.gesture.is_some()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.gesture.is_some()
    }

    /// Starts a stroke or eraser gesture. Eraser width is its diameter. A pen
    /// tap remains a visible dot. A new gesture commits the previous gesture.
    pub fn begin(&mut self, point: Point, tool: Tool, color: Color, width: f32) {
        self.end();
        if !point.is_finite() || !width.is_finite() {
            return;
        }
        let width = width.clamp(0.5, 1024.0);
        if tool == Tool::Pen {
            self.gesture = Some(Gesture::Pen(Stroke::new(point, color, width)));
        } else {
            let before = self.erase(point, point, width * 0.5, tool, false);
            self.gesture = Some(Gesture::Erase {
                tool,
                previous: point,
                radius: width * 0.5,
                before,
            });
        }
    }

    pub fn extend(&mut self, point: Point) {
        if !point.is_finite() {
            return;
        }
        let Some(mut gesture) = self.gesture.take() else {
            return;
        };
        match &mut gesture {
            Gesture::Pen(stroke) => {
                let previous = *stroke.points.last().expect("A stroke has a first point");
                if previous.distance_squared(point) >= POINT_SPACING.powi(2) {
                    stroke.append(point);
                }
            }
            Gesture::Erase {
                tool,
                previous,
                radius,
                before,
            } => {
                if previous.distance_squared(point) >= EPSILON {
                    let snapshot = self.erase(*previous, point, *radius, *tool, before.is_some());
                    if before.is_none() {
                        *before = snapshot;
                    }
                    *previous = point;
                }
            }
        }
        self.gesture = Some(gesture);
    }

    pub fn end(&mut self) {
        match self.gesture.take() {
            Some(Gesture::Pen(mut stroke)) => {
                stroke.points.shrink_to_fit();
                self.strokes.push(stroke);
                self.push_undo(Undo::RemoveLast);
            }
            Some(Gesture::Erase {
                before: Some(before),
                ..
            }) => {
                self.push_undo(Undo::Restore(before));
            }
            _ => {}
        }
    }

    /// Cancels an unfinished gesture, restoring any ink it erased.
    pub fn cancel(&mut self) {
        if let Some(Gesture::Erase {
            before: Some(before),
            ..
        }) = self.gesture.take()
        {
            self.strokes = before;
        }
    }

    pub fn undo(&mut self) -> bool {
        if self.gesture.is_some() {
            self.cancel();
            return true;
        }
        let Some(action) = self.undo.pop_back() else {
            return false;
        };
        self.undo_bytes = self.undo_bytes.saturating_sub(action.memory_bytes());
        match action {
            Undo::RemoveLast => {
                self.strokes.pop();
            }
            Undo::Restore(strokes) => self.strokes = strokes,
        }
        true
    }

    pub fn clear(&mut self) {
        self.end();
        if !self.strokes.is_empty() {
            let before = std::mem::take(&mut self.strokes);
            self.push_undo(Undo::Restore(before));
        }
    }

    fn push_undo(&mut self, action: Undo) {
        let bytes = action.memory_bytes();
        if bytes > MAX_UNDO_BYTES {
            // Once an oversized operation cannot be retained, earlier actions
            // would no longer apply to the current scene. Drop the whole chain.
            self.undo.clear();
            self.undo_bytes = 0;
            return;
        }
        while self.undo.len() >= MAX_UNDO_STEPS || self.undo_bytes + bytes > MAX_UNDO_BYTES {
            if let Some(old) = self.undo.pop_front() {
                self.undo_bytes = self.undo_bytes.saturating_sub(old.memory_bytes());
            } else {
                break;
            }
        }
        self.undo_bytes += bytes;
        self.undo.push_back(action);
    }

    /// Takes a lazy snapshot only if this gesture actually intersects ink.
    fn erase(
        &mut self,
        from: Point,
        to: Point,
        radius: f32,
        tool: Tool,
        already_snapshotted: bool,
    ) -> Option<Vec<Stroke>> {
        let mut sweep_bounds = Bounds::at(from);
        sweep_bounds.include(to);
        let sweep_bounds = sweep_bounds.expanded(radius);
        let mut changed = Vec::new();
        for (index, stroke) in self.strokes.iter().enumerate() {
            if !stroke.bounds().overlaps(sweep_bounds) {
                continue;
            }
            // Round cap allowance ensures a removed region is not painted back
            // by the newly exposed ends of the surviving vector fragments.
            let cut_radius = radius + stroke.width * 0.5;
            if tool == Tool::ObjectEraser {
                if stroke_hits_capsule(stroke, from, to, cut_radius) {
                    changed.push((index, Vec::new()));
                }
            } else if let Some(fragments) = clip_stroke(stroke, from, to, cut_radius) {
                changed.push((index, fragments));
            }
        }
        if changed.is_empty() {
            return None;
        }
        let before = if already_snapshotted {
            None
        } else {
            Some(self.strokes.clone())
        };
        // Reverse order keeps every remaining original index valid.
        for (index, fragments) in changed.into_iter().rev() {
            self.strokes.splice(index..=index, fragments);
        }
        before
    }
}

fn scene_bytes(strokes: &Vec<Stroke>) -> usize {
    std::mem::size_of::<Vec<Stroke>>()
        + strokes.capacity() * std::mem::size_of::<Stroke>()
        + strokes
            .iter()
            .map(|stroke| stroke.memory_bytes() - std::mem::size_of::<Stroke>())
            .sum::<usize>()
}

/// Exponential resizing feels consistent at small and large diameters. The
/// first four logical pixels are a dead zone; upward movement is negative.
pub fn eraser_diameter(initial: f32, vertical_delta: f32) -> f32 {
    if !initial.is_finite() || !vertical_delta.is_finite() {
        return MIN_ERASER_DIAMETER;
    }
    let initial = initial.clamp(MIN_ERASER_DIAMETER, MAX_ERASER_DIAMETER);
    let travel = (vertical_delta.abs() - 4.0).max(0.0) * vertical_delta.signum();
    (initial * (-travel / 100.0).exp()).clamp(MIN_ERASER_DIAMETER, MAX_ERASER_DIAMETER)
}

fn stroke_hits_capsule(stroke: &Stroke, from: Point, to: Point, radius: f32) -> bool {
    if stroke.points.len() == 1 {
        return point_segment_distance_squared(stroke.points[0], from, to) <= radius * radius;
    }
    stroke
        .points
        .windows(2)
        .any(|pair| capsule_interval(pair[0], pair[1], from, to, radius).is_some())
}

/// Returns fragments only when some positive-length part of the stroke is cut.
fn clip_stroke(stroke: &Stroke, from: Point, to: Point, radius: f32) -> Option<Vec<Stroke>> {
    if stroke.points.len() == 1 {
        return (point_segment_distance_squared(stroke.points[0], from, to) <= radius * radius)
            .then(Vec::new);
    }
    let mut fragments: Vec<Vec<Point>> = Vec::new();
    let mut current = Vec::new();
    let mut changed = false;
    for pair in stroke.points.windows(2) {
        let (start, finish) = (pair[0], pair[1]);
        let cut = capsule_interval(start, finish, from, to, radius)
            .filter(|(low, high)| high - low > PARAMETER_EPSILON);
        if let Some((low, high)) = cut {
            changed = true;
            if low > PARAMETER_EPSILON {
                append_distinct(&mut current, start);
                append_distinct(&mut current, start.interpolate(finish, low));
            }
            if !current.is_empty() {
                fragments.push(std::mem::take(&mut current));
            }
            if high < 1.0 - PARAMETER_EPSILON {
                append_distinct(&mut current, start.interpolate(finish, high));
                append_distinct(&mut current, finish);
            }
        } else {
            append_distinct(&mut current, start);
            append_distinct(&mut current, finish);
        }
    }
    if !changed {
        return None;
    }
    if !current.is_empty() {
        fragments.push(current);
    }
    Some(
        fragments
            .into_iter()
            .map(|points| Stroke::from_points(points, stroke.color, stroke.width))
            .collect(),
    )
}

fn append_distinct(points: &mut Vec<Point>, point: Point) {
    if points
        .last()
        .is_none_or(|last| last.distance_squared(point) > EPSILON * EPSILON)
    {
        points.push(point);
    }
}

fn point_segment_distance_squared(point: Point, start: Point, finish: Point) -> f32 {
    let dx = finish.x - start.x;
    let dy = finish.y - start.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared < EPSILON * EPSILON {
        return point.distance_squared(start);
    }
    let t =
        (((point.x - start.x) * dx + (point.y - start.y) * dy) / length_squared).clamp(0.0, 1.0);
    point.distance_squared(start.interpolate(finish, t))
}

/// Clips a line segment against a convex capsule. Intersections are continuous
/// even when pointer samples or stroke endpoints are far apart.
fn capsule_interval(
    start: Point,
    finish: Point,
    from: Point,
    to: Point,
    radius: f32,
) -> Option<(f32, f32)> {
    let vx = to.x - from.x;
    let vy = to.y - from.y;
    let length_squared = vx * vx + vy * vy;
    let mut result = circle_interval(start, finish, from, radius);
    if length_squared < EPSILON * EPSILON {
        return result;
    }
    merge_interval(&mut result, circle_interval(start, finish, to, radius));

    let dx = finish.x - start.x;
    let dy = finish.y - start.y;
    let sx = start.x - from.x;
    let sy = start.y - from.y;
    let mut low = 0.0;
    let mut high = 1.0;
    let band = radius * length_squared.sqrt();
    // The rectangular middle of the capsule is the intersection of a parallel
    // strip and the two perpendicular lines through its endpoint centers.
    if clip_linear(
        sx * vy - sy * vx,
        dx * vy - dy * vx,
        -band,
        band,
        &mut low,
        &mut high,
    ) && clip_linear(
        sx * vx + sy * vy,
        dx * vx + dy * vy,
        0.0,
        length_squared,
        &mut low,
        &mut high,
    ) {
        merge_interval(&mut result, Some((low, high)));
    }
    result
}

fn merge_interval(target: &mut Option<(f32, f32)>, interval: Option<(f32, f32)>) {
    if let Some((low, high)) = interval {
        *target = Some(match *target {
            Some((old_low, old_high)) => (old_low.min(low), old_high.max(high)),
            None => (low, high),
        });
    }
}

fn circle_interval(start: Point, finish: Point, center: Point, radius: f32) -> Option<(f32, f32)> {
    // Wider intermediates avoid subtracting nearly equal f32 values for a
    // small eraser in the middle of a long segment on a multi-monitor desktop.
    let dx = f64::from(finish.x) - f64::from(start.x);
    let dy = f64::from(finish.y) - f64::from(start.y);
    let sx = f64::from(start.x) - f64::from(center.x);
    let sy = f64::from(start.y) - f64::from(center.y);
    let radius = f64::from(radius);
    let a = dx * dx + dy * dy;
    let c = sx * sx + sy * sy - radius * radius;
    if a < f64::from(EPSILON * EPSILON) {
        return (c <= 0.0).then_some((0.0, 1.0));
    }
    let b = 2.0 * (sx * dx + sy * dy);
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    let low = ((-b - root) / (2.0 * a)).max(0.0);
    let high = ((-b + root) / (2.0 * a)).min(1.0);
    (low <= high).then_some((low as f32, high as f32))
}

fn clip_linear(
    value: f32,
    slope: f32,
    minimum: f32,
    maximum: f32,
    low: &mut f32,
    high: &mut f32,
) -> bool {
    if slope.abs() < EPSILON {
        return value >= minimum && value <= maximum;
    }
    let first = (minimum - value) / slope;
    let second = (maximum - value) / slope;
    *low = low.max(first.min(second));
    *high = high.min(first.max(second));
    *low <= *high
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Color = Color::rgb(255, 20, 40);

    fn line(ink: &mut Ink, start: Point, finish: Point) {
        ink.begin(start, Tool::Pen, RED, 4.0);
        ink.extend(finish);
        ink.end();
    }

    #[test]
    fn colors_round_trip_and_reject_invalid_values() {
        assert_eq!(Color::from_hex(" #aBc123 ").unwrap().to_hex(), "#ABC123");
        assert_eq!(Color::from_hex("09F"), Some(Color::rgb(0, 153, 255)));
        for invalid in ["", "#ABCD", "#GGGGGG", "#1234567", "-12345", "+12345"] {
            assert_eq!(Color::from_hex(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn a_tap_is_a_dot_and_one_undo_removes_one_gesture() {
        let mut ink = Ink::new();
        ink.begin(Point::new(2.0, 3.0), Tool::Pen, RED, 4.0);
        assert_eq!(ink.active_stroke().unwrap().points.len(), 1);
        ink.end();
        assert_eq!(ink.strokes().len(), 1);
        assert_eq!(ink.strokes()[0].points, vec![Point::new(2.0, 3.0)]);
        assert!(ink.undo());
        assert!(!ink.undo());
    }

    #[test]
    fn object_eraser_hits_between_sparse_samples() {
        let mut ink = Ink::new();
        line(&mut ink, Point::new(0.0, 50.0), Point::new(100.0, 50.0));
        line(&mut ink, Point::new(0.0, 80.0), Point::new(100.0, 80.0));
        ink.begin(Point::new(50.0, 10.0), Tool::ObjectEraser, RED, 10.0);
        ink.extend(Point::new(50.0, 65.0));
        ink.end();
        assert_eq!(ink.strokes().len(), 1);
        assert_eq!(ink.strokes()[0].points[0].y, 80.0);
        assert!(ink.undo());
        assert_eq!(ink.strokes().len(), 2);
    }

    #[test]
    fn pixel_eraser_splits_a_long_segment_and_undo_restores_it() {
        let mut ink = Ink::new();
        line(&mut ink, Point::new(0.0, 50.0), Point::new(100.0, 50.0));
        let original = ink.strokes().to_vec();
        ink.begin(Point::new(50.0, 50.0), Tool::Eraser, RED, 16.0);
        ink.end();
        assert_eq!(ink.strokes().len(), 2);
        let left = ink.strokes()[0].points.last().unwrap();
        let right = ink.strokes()[1].points[0];
        assert!((left.x - 40.0).abs() < 0.001);
        assert!((right.x - 60.0).abs() < 0.001);
        assert!(ink.undo());
        assert_eq!(ink.strokes(), original);
    }

    #[test]
    fn moving_eraser_cuts_the_whole_sweep_in_a_single_undo() {
        let mut ink = Ink::new();
        line(&mut ink, Point::new(0.0, 50.0), Point::new(200.0, 50.0));
        let original = ink.strokes().to_vec();
        ink.begin(Point::new(40.0, 50.0), Tool::Eraser, RED, 16.0);
        ink.extend(Point::new(100.0, 50.0));
        ink.extend(Point::new(160.0, 50.0));
        ink.end();
        assert_eq!(ink.strokes().len(), 2);
        assert!((ink.strokes()[0].points.last().unwrap().x - 30.0).abs() < 0.001);
        assert!((ink.strokes()[1].points[0].x - 170.0).abs() < 0.001);
        assert!(ink.undo());
        assert_eq!(ink.strokes(), original);
        assert!(ink.undo());
        assert!(ink.strokes().is_empty());
    }

    #[test]
    fn eraser_clear_and_cancel_preserve_undo_order() {
        let mut ink = Ink::new();
        line(&mut ink, Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        let original = ink.strokes().to_vec();
        ink.begin(Point::new(50.0, 0.0), Tool::ObjectEraser, RED, 8.0);
        assert!(ink.strokes().is_empty());
        ink.cancel();
        assert_eq!(ink.strokes(), original);
        ink.clear();
        assert!(ink.strokes().is_empty());
        assert!(ink.undo());
        assert_eq!(ink.strokes(), original);
        assert!(ink.undo());
        assert!(ink.strokes().is_empty());
    }

    #[test]
    fn missing_ink_does_not_add_an_undo_step() {
        let mut ink = Ink::new();
        line(&mut ink, Point::new(0.0, 0.0), Point::new(100.0, 0.0));
        ink.begin(Point::new(50.0, 100.0), Tool::Eraser, RED, 8.0);
        ink.end();
        assert!(ink.undo());
        assert!(ink.strokes().is_empty());
    }

    #[test]
    fn diagonal_capsule_cuts_without_endpoint_hits() {
        let interval = capsule_interval(
            Point::new(0.0, 100.0),
            Point::new(100.0, 0.0),
            Point::new(0.0, 0.0),
            Point::new(100.0, 100.0),
            5.0,
        )
        .unwrap();
        assert!((interval.0 - 0.46464467).abs() < 0.0001);
        assert!((interval.1 - 0.5353553).abs() < 0.0001);
    }

    #[test]
    fn small_eraser_remains_accurate_across_a_wide_virtual_desktop() {
        let mut ink = Ink::new();
        line(
            &mut ink,
            Point::new(-20000.0, 50.0),
            Point::new(20000.0, 50.0),
        );
        ink.begin(Point::new(0.0, 50.0), Tool::Eraser, RED, 6.0);
        ink.end();
        assert_eq!(ink.strokes().len(), 2);
        assert!((ink.strokes()[0].points.last().unwrap().x + 5.0).abs() < 0.01);
        assert!((ink.strokes()[1].points[0].x - 5.0).abs() < 0.01);
    }

    #[test]
    fn undo_history_is_bounded() {
        let mut ink = Ink::new();
        for index in 0..100 {
            ink.begin(Point::new(index as f32, 0.0), Tool::Pen, RED, 4.0);
            ink.end();
        }
        let mut undone = 0;
        while ink.undo() {
            undone += 1;
        }
        assert_eq!(undone, MAX_UNDO_STEPS);
        assert_eq!(ink.strokes().len(), 100 - MAX_UNDO_STEPS);
    }

    #[test]
    fn eraser_resize_has_a_deadzone_and_stable_limits() {
        assert_eq!(eraser_diameter(30.0, -3.0), 30.0);
        assert!(eraser_diameter(30.0, -50.0) > 30.0);
        assert!(eraser_diameter(30.0, 50.0) < 30.0);
        assert_eq!(eraser_diameter(30.0, -10000.0), MAX_ERASER_DIAMETER);
        assert_eq!(eraser_diameter(30.0, 10000.0), MIN_ERASER_DIAMETER);
        assert_eq!(eraser_diameter(f32::NAN, 0.0), MIN_ERASER_DIAMETER);
    }

    #[test]
    fn invalid_coordinates_are_ignored() {
        let mut ink = Ink::new();
        ink.begin(Point::new(f32::NAN, 0.0), Tool::Pen, RED, 4.0);
        assert!(!ink.is_drawing());
        ink.begin(Point::new(2.0, 3.0), Tool::Pen, RED, 4.0);
        ink.extend(Point::new(4.0, f32::INFINITY));
        ink.end();
        assert_eq!(ink.strokes()[0].points.len(), 1);
    }
}
