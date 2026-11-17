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
    Shape,
    Fill,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShapeKind {
    #[default]
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Triangle,
}

impl ShapeKind {
    pub const ALL: [Self; 5] = [
        Self::Rectangle,
        Self::Ellipse,
        Self::Line,
        Self::Arrow,
        Self::Triangle,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Rectangle => "Rectangle",
            Self::Ellipse => "Ellipse",
            Self::Line => "Line",
            Self::Arrow => "Arrow",
            Self::Triangle => "Triangle",
        }
    }
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
    /// Closed shape outlines also carry their fill and local eraser cutouts.
    pub closed: bool,
    pub fill: Option<Color>,
    pub erasures: Vec<EraseSweep>,
    bounds: Bounds,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EraseSweep {
    pub from: Point,
    pub to: Point,
    pub radius: f32,
}

impl EraseSweep {
    fn contains(self, point: Point) -> bool {
        point_segment_distance_squared(point, self.from, self.to) <= self.radius * self.radius
    }

    fn contains_sweep(self, from: Point, to: Point, radius: f32) -> bool {
        let remaining = self.radius - radius;
        remaining >= 0.0
            && point_segment_distance_squared(from, self.from, self.to) <= remaining * remaining
            && point_segment_distance_squared(to, self.from, self.to) <= remaining * remaining
    }
}

impl Stroke {
    fn new(point: Point, color: Color, width: f32) -> Self {
        Self {
            points: vec![point],
            color,
            width,
            closed: false,
            fill: None,
            erasures: Vec::new(),
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
            closed: false,
            fill: None,
            erasures: Vec::new(),
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
        std::mem::size_of::<Self>()
            + self.points.capacity() * std::mem::size_of::<Point>()
            + self.erasures.capacity() * std::mem::size_of::<EraseSweep>()
    }

    fn visible_at(&self, point: Point) -> bool {
        !self.erasures.iter().any(|cut| cut.contains(point))
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
    Shape {
        origin: Point,
        kind: ShapeKind,
        stroke: Stroke,
    },
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
            Some(Gesture::Pen(stroke) | Gesture::Shape { stroke, .. }) => Some(stroke),
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
        match tool {
            Tool::Pen => self.gesture = Some(Gesture::Pen(Stroke::new(point, color, width))),
            Tool::Shape => self.begin_shape(point, ShapeKind::Rectangle, color, width),
            Tool::Fill => {
                self.fill_at(point, color);
            }
            Tool::Eraser | Tool::ObjectEraser => {
                let before = self.erase(point, point, width * 0.5, tool, false);
                self.gesture = Some(Gesture::Erase {
                    tool,
                    previous: point,
                    radius: width * 0.5,
                    before,
                });
            }
        }
    }

    /// Starts a shape preview. Dragging changes its geometry, not the scene;
    /// release commits exactly one object and cancellation discards the preview.
    pub fn begin_shape(&mut self, point: Point, kind: ShapeKind, color: Color, width: f32) {
        self.end();
        if !point.is_finite() || !width.is_finite() {
            return;
        }
        self.gesture = Some(Gesture::Shape {
            origin: point,
            kind,
            stroke: shape_stroke(point, point, kind, color, width.clamp(0.5, 1024.0)),
        });
    }

    /// Fills the topmost closed shape beneath the pointer. An already matching
    /// fill or a click in an erased hole is a no-op and creates no undo entry.
    pub fn fill_at(&mut self, point: Point, color: Color) -> Option<Bounds> {
        self.end();
        if !point.is_finite() {
            return None;
        }
        let index = self.strokes.iter().rposition(|stroke| {
            stroke.closed && polygon_contains(&stroke.points, point) && stroke.visible_at(point)
        })?;
        if self.strokes[index].fill == Some(color) {
            return None;
        }
        let before = self.strokes.clone();
        let stroke = &mut self.strokes[index];
        stroke.fill = Some(color);
        let bounds = stroke.bounds();
        self.push_undo(Undo::Restore(before));
        Some(bounds)
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
            Gesture::Shape {
                origin,
                kind,
                stroke,
            } => {
                *stroke = shape_stroke(*origin, point, *kind, stroke.color, stroke.width);
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
            Some(Gesture::Shape { mut stroke, .. }) => {
                // A stationary shape gesture is not an accidental pen dot.
                if stroke.points.len() > 1 {
                    stroke.points.shrink_to_fit();
                    self.strokes.push(stroke);
                    self.push_undo(Undo::RemoveLast);
                }
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
                if stroke_hits_capsule(stroke, from, to, radius) {
                    changed.push((index, Vec::new()));
                }
            } else if stroke.closed {
                if stroke_hits_capsule(stroke, from, to, radius)
                    && !stroke
                        .erasures
                        .iter()
                        .any(|cut| cut.contains_sweep(from, to, radius))
                {
                    let mut erased = stroke.clone();
                    append_eraser_sweep(&mut erased.erasures, EraseSweep { from, to, radius });
                    changed.push((index, vec![erased]));
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

fn shape_stroke(origin: Point, end: Point, kind: ShapeKind, color: Color, width: f32) -> Stroke {
    if origin.distance_squared(end) < POINT_SPACING * POINT_SPACING {
        return Stroke::new(origin, color, width);
    }
    let left = origin.x.min(end.x);
    let right = origin.x.max(end.x);
    let top = origin.y.min(end.y);
    let bottom = origin.y.max(end.y);
    let center = Point::new(left * 0.5 + right * 0.5, top * 0.5 + bottom * 0.5);
    let mut closed = false;
    let points = match kind {
        ShapeKind::Rectangle | ShapeKind::Ellipse | ShapeKind::Triangle
            if right - left < POINT_SPACING || bottom - top < POINT_SPACING =>
        {
            vec![origin, end]
        }
        ShapeKind::Rectangle => {
            closed = true;
            vec![
                Point::new(left, top),
                Point::new(right, top),
                Point::new(right, bottom),
                Point::new(left, bottom),
                Point::new(left, top),
            ]
        }
        ShapeKind::Ellipse => {
            closed = true;
            let rx = right * 0.5 - left * 0.5;
            let ry = bottom * 0.5 - top * 0.5;
            // A quarter-pixel chord error keeps large ellipses smooth without
            // allocating points proportional to their screen-sized perimeter.
            let segments = (std::f32::consts::PI * (rx.max(ry) / 0.5).sqrt())
                .ceil()
                .clamp(24.0, 512.0) as usize;
            let mut points = Vec::with_capacity(segments + 1);
            for index in 0..segments {
                let angle = index as f32 * std::f32::consts::TAU / segments as f32;
                points.push(Point::new(
                    center.x + rx * angle.cos(),
                    center.y + ry * angle.sin(),
                ));
            }
            points.push(points[0]);
            points
        }
        ShapeKind::Triangle => {
            closed = true;
            vec![
                Point::new(center.x, top),
                Point::new(right, bottom),
                Point::new(left, bottom),
                Point::new(center.x, top),
            ]
        }
        ShapeKind::Line => vec![origin, end],
        ShapeKind::Arrow => {
            let dx = end.x - origin.x;
            let dy = end.y - origin.y;
            let length = dx.hypot(dy);
            let head = (width * 3.0).clamp(10.0, 36.0).min(length * 0.45);
            let ux = dx / length;
            let uy = dy / length;
            let base = Point::new(end.x - ux * head, end.y - uy * head);
            vec![
                origin,
                end,
                Point::new(base.x - uy * head * 0.55, base.y + ux * head * 0.55),
                end,
                Point::new(base.x + uy * head * 0.55, base.y - ux * head * 0.55),
            ]
        }
    };
    if !points.iter().all(|point| point.is_finite()) {
        return Stroke::new(origin, color, width);
    }
    let mut stroke = Stroke::from_points(points, color, width);
    stroke.closed = closed;
    stroke
}

fn polygon_contains(points: &[Point], point: Point) -> bool {
    let mut inside = false;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if point_segment_distance_squared(point, a, b) <= EPSILON * EPSILON {
            return true;
        }
        if (a.y > point.y) != (b.y > point.y)
            && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x
        {
            inside = !inside;
        }
    }
    inside
}

fn append_eraser_sweep(cuts: &mut Vec<EraseSweep>, next: EraseSweep) {
    if let Some(last) = cuts.last_mut()
        && last.radius == next.radius
        && last.to == next.from
    {
        let ax = last.to.x - last.from.x;
        let ay = last.to.y - last.from.y;
        let bx = next.to.x - next.from.x;
        let by = next.to.y - next.from.y;
        if (ax * by - ay * bx).abs() <= EPSILON && ax * bx + ay * by >= 0.0 {
            last.to = next.to;
            return;
        }
    }
    cuts.push(next);
}

fn stroke_hits_capsule(stroke: &Stroke, from: Point, to: Point, radius: f32) -> bool {
    if stroke
        .erasures
        .iter()
        .any(|cut| cut.contains_sweep(from, to, radius))
    {
        return false;
    }
    if stroke.fill.is_some() {
        // The centerline test detects a sweep inside the shape, where no
        // outline segment is touched. Boundary tests below handle crossings.
        for point in [from, to, from.interpolate(to, 0.5)] {
            if polygon_contains(&stroke.points, point) && stroke.visible_at(point) {
                return true;
            }
            // A larger eraser can reach visible fill around an existing hole
            // while its center remains inside that hole.
            for index in 0..16 {
                let angle = index as f32 * std::f32::consts::TAU / 16.0;
                let edge = Point::new(
                    point.x + radius * angle.cos(),
                    point.y + radius * angle.sin(),
                );
                if polygon_contains(&stroke.points, edge) && stroke.visible_at(edge) {
                    return true;
                }
            }
        }
    }
    // Fill and existing holes use the actual eraser footprint. Only an outline
    // needs its own half-width allowance when checking its centerline segments.
    let outline_radius = radius + stroke.width * 0.5;
    if stroke.points.len() == 1 {
        return point_segment_distance_squared(stroke.points[0], from, to)
            <= outline_radius * outline_radius;
    }
    stroke.points.windows(2).any(|pair| {
        capsule_interval(pair[0], pair[1], from, to, outline_radius).is_some_and(|(low, high)| {
            [low, (low + high) * 0.5, high]
                .into_iter()
                .any(|t| stroke.visible_at(pair[0].interpolate(pair[1], t)))
        })
    })
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

    fn rectangle(ink: &mut Ink, start: Point, finish: Point) {
        ink.begin_shape(start, ShapeKind::Rectangle, RED, 4.0);
        ink.extend(finish);
        ink.end();
    }

    #[test]
    fn shape_drag_replaces_its_preview_and_commits_one_object() {
        let mut ink = Ink::new();
        ink.begin_shape(Point::new(100.0, 90.0), ShapeKind::Rectangle, RED, 4.0);
        ink.extend(Point::new(20.0, 10.0));
        let preview = ink.active_stroke().unwrap();
        assert!(preview.closed);
        assert_eq!(preview.points.len(), 5);
        assert_eq!(
            preview.bounds(),
            Bounds {
                min_x: 18.0,
                min_y: 8.0,
                max_x: 102.0,
                max_y: 92.0
            }
        );
        ink.extend(Point::new(0.0, 0.0));
        assert_eq!(ink.active_stroke().unwrap().points.len(), 5);
        assert!(ink.strokes().is_empty());
        ink.end();
        assert_eq!(ink.strokes().len(), 1);
        assert!(ink.undo());
        assert!(!ink.undo());

        ink.begin_shape(Point::new(4.0, 5.0), ShapeKind::Ellipse, RED, 4.0);
        ink.end();
        assert!(
            ink.strokes().is_empty(),
            "A shape tap must not leave an accidental dot"
        );
    }

    #[test]
    fn fill_targets_topmost_closed_shape_and_is_transactional() {
        let blue = Color::rgb(20, 80, 255);
        let mut ink = Ink::new();
        rectangle(&mut ink, Point::new(0.0, 0.0), Point::new(120.0, 100.0));
        rectangle(&mut ink, Point::new(20.0, 20.0), Point::new(80.0, 80.0));
        let original = ink.strokes().to_vec();
        assert_eq!(
            ink.fill_at(Point::new(40.0, 40.0), blue),
            Some(ink.strokes()[1].bounds())
        );
        assert_eq!(ink.strokes()[0].fill, None);
        assert_eq!(ink.strokes()[1].fill, Some(blue));
        assert!(ink.fill_at(Point::new(40.0, 40.0), blue).is_none());
        assert!(ink.fill_at(Point::new(200.0, 40.0), blue).is_none());
        assert!(ink.undo());
        assert_eq!(ink.strokes(), original);
        assert!(ink.undo());
        assert_eq!(ink.strokes().len(), 1);
    }

    #[test]
    fn pixel_eraser_cuts_filled_shape_interior_and_undo_restores_fill() {
        let mut ink = Ink::new();
        rectangle(&mut ink, Point::new(0.0, 0.0), Point::new(200.0, 160.0));
        ink.fill_at(Point::new(80.0, 80.0), RED).unwrap();
        let original = ink.strokes().to_vec();
        ink.begin(Point::new(50.0, 80.0), Tool::Eraser, RED, 24.0);
        ink.extend(Point::new(150.0, 80.0));
        ink.end();
        assert_eq!(ink.strokes().len(), 1);
        assert_eq!(ink.strokes()[0].fill, Some(RED));
        assert_eq!(
            ink.strokes()[0].erasures.len(),
            1,
            "A straight sweep uses one compact capsule"
        );
        assert!(!ink.strokes()[0].visible_at(Point::new(100.0, 80.0)));
        assert!(ink.strokes()[0].visible_at(Point::new(100.0, 110.0)));
        assert!(
            ink.fill_at(Point::new(100.0, 80.0), Color::default())
                .is_none()
        );
        assert!(ink.undo());
        assert_eq!(ink.strokes(), original);
    }

    #[test]
    fn object_eraser_removes_filled_interior_but_respects_an_erased_hole() {
        let mut ink = Ink::new();
        rectangle(&mut ink, Point::new(0.0, 0.0), Point::new(200.0, 160.0));
        ink.fill_at(Point::new(80.0, 80.0), RED).unwrap();
        ink.begin(Point::new(100.0, 80.0), Tool::Eraser, RED, 40.0);
        ink.end();
        let cut = ink.strokes().to_vec();
        ink.begin(Point::new(100.0, 80.0), Tool::ObjectEraser, RED, 8.0);
        ink.end();
        assert_eq!(ink.strokes(), cut);
        ink.begin(Point::new(130.0, 80.0), Tool::ObjectEraser, RED, 8.0);
        ink.end();
        assert!(ink.strokes().is_empty());
        assert!(ink.undo());
        assert_eq!(ink.strokes(), cut);
    }

    #[test]
    fn thick_outline_does_not_expand_object_eraser_inside_a_filled_hole() {
        let mut ink = Ink::new();
        ink.begin_shape(Point::new(0.0, 0.0), ShapeKind::Rectangle, RED, 40.0);
        ink.extend(Point::new(300.0, 250.0));
        ink.end();
        let center = Point::new(150.0, 125.0);
        ink.fill_at(center, RED).unwrap();
        ink.begin(center, Tool::Eraser, RED, 80.0);
        ink.end();
        let cut = ink.strokes().to_vec();

        // A 35-pixel-radius eraser fits wholly inside the 40-pixel hole. The
        // outline's extra 20 pixels must not turn this into a false collision.
        ink.begin(center, Tool::ObjectEraser, RED, 70.0);
        ink.end();
        assert_eq!(ink.strokes(), cut);
        ink.begin(center, Tool::ObjectEraser, RED, 82.0);
        ink.end();
        assert!(
            ink.strokes().is_empty(),
            "A larger footprint reaches visible fill"
        );
        assert!(ink.undo());
        assert_eq!(ink.strokes(), cut);
        assert!(ink.undo());
        assert!(
            ink.strokes()[0].erasures.is_empty(),
            "The missed eraser adds no undo action"
        );
    }

    #[test]
    fn shape_cancel_and_clear_preserve_undo_order() {
        let mut ink = Ink::new();
        rectangle(&mut ink, Point::new(0.0, 0.0), Point::new(80.0, 60.0));
        ink.fill_at(Point::new(20.0, 20.0), RED).unwrap();
        let original = ink.strokes().to_vec();
        ink.begin(Point::new(30.0, 20.0), Tool::Eraser, RED, 10.0);
        ink.cancel();
        assert_eq!(ink.strokes(), original);
        ink.begin_shape(Point::new(10.0, 10.0), ShapeKind::Triangle, RED, 3.0);
        ink.extend(Point::new(200.0, 100.0));
        ink.cancel();
        assert_eq!(ink.strokes(), original);
        ink.clear();
        assert!(ink.undo());
        assert_eq!(ink.strokes(), original);
        assert!(ink.undo());
        assert_eq!(ink.strokes()[0].fill, None);
    }

    #[test]
    fn ellipse_and_triangle_fill_but_lines_and_arrows_remain_open() {
        for kind in ShapeKind::ALL {
            let mut ink = Ink::new();
            ink.begin_shape(Point::new(-100.0, -80.0), kind, RED, 3.0);
            ink.extend(Point::new(100.0, 80.0));
            ink.end();
            let closed = matches!(
                kind,
                ShapeKind::Rectangle | ShapeKind::Ellipse | ShapeKind::Triangle
            );
            assert_eq!(ink.strokes()[0].closed, closed, "{}", kind.label());
            assert_eq!(
                ink.fill_at(Point::new(0.0, 0.0), RED).is_some(),
                closed,
                "{}",
                kind.label()
            );
            assert!(
                ink.strokes()[0]
                    .points
                    .iter()
                    .all(|point| point.is_finite())
            );
        }
    }
}
