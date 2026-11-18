//! Deliberate cursor-location motion and its short, timer-driven animation.
//!
//! The host supplies physical screen samples and owns presentation. Detection
//! allocates no memory, works during mouse capture, and does not alter input.

use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::POINT;

const MIN_SAMPLE_TIME: Duration = Duration::from_millis(4);
const MAX_SAMPLE_TIME: Duration = Duration::from_millis(80);
const SHAKE_WINDOW: Duration = Duration::from_millis(300);
const SHAKE_PAUSE: Duration = Duration::from_millis(90);
const FAST_SPEED: f64 = 5_000.0;
const EXTREME_SPEED: f64 = 14_000.0;
const MAX_PLAUSIBLE_SPEED: f64 = 60_000.0;
const MAX_SAMPLE_DISTANCE: f64 = 480.0;
const MIN_LEG_DISTANCE: f64 = 56.0;
const MIN_EXTREME_DISTANCE: f64 = 240.0;
const MIN_EXTREME_TIME: Duration = Duration::from_millis(16);
const GROW: Duration = Duration::from_millis(90);
const HOLD: Duration = Duration::from_millis(85);
const SHRINK: Duration = Duration::from_millis(240);
const COOLDOWN: Duration = Duration::from_millis(250);

/// `amount` is an enlargement envelope, not a cursor size: zero is normal and
/// one is the host's chosen maximum. No polling is necessary when inactive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorFrame {
    pub amount: f32,
    pub active: bool,
}

#[derive(Clone, Copy, Debug)]
struct Sample {
    position: [f64; 2],
    scale: f64,
    at: Instant,
}

#[derive(Clone, Copy, Debug)]
struct Shake {
    direction: [f64; 2],
    leg_distance: f64,
    reversals: u8,
    started: Instant,
    last_fast: Instant,
}

#[derive(Clone, Copy, Debug)]
struct Extreme {
    started: Instant,
    distance: f64,
    samples: u8,
}

/// All thresholds use device-independent pixels and seconds. A shake needs
/// three substantial, fast legs with two reversals; a very fast sweep needs
/// several samples. A single jump, stale sample, or DPI change cannot trigger.
#[derive(Debug, Default)]
pub struct CursorMotion {
    sample: Option<Sample>,
    shake: Option<Shake>,
    extreme: Option<Extreme>,
    started: Option<Instant>,
    cooldown_until: Option<Instant>,
}

impl CursorMotion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a physical screen position. Returns true only on a fresh
    /// trigger. Repeated movement never restarts an active animation.
    pub fn record(&mut self, point: POINT, scale: f32, now: Instant) -> bool {
        if !scale.is_finite() || scale <= 0.0 {
            self.clear_detection();
            self.sample = None;
            return false;
        }
        let current = Sample {
            position: [f64::from(point.x), f64::from(point.y)],
            scale: f64::from(scale),
            at: now,
        };
        let Some(previous) = self.sample else {
            self.sample = Some(current);
            return false;
        };
        if now <= previous.at {
            return false;
        }
        let elapsed = now.duration_since(previous.at);
        // Accumulate very frequent packets against the same anchor, so their
        // individual quantized pixels do not create implausible velocities.
        if elapsed < MIN_SAMPLE_TIME {
            return false;
        }
        self.sample = Some(current);
        if elapsed > MAX_SAMPLE_TIME || (current.scale - previous.scale).abs() > 0.01 {
            self.clear_detection();
            return false;
        }
        if self.cooldown_until.is_some_and(|until| now < until) {
            self.clear_detection();
            return false;
        }
        let delta = [
            (current.position[0] - previous.position[0]) / current.scale,
            (current.position[1] - previous.position[1]) / current.scale,
        ];
        let distance = delta[0].hypot(delta[1]);
        let speed = distance / elapsed.as_secs_f64();
        if distance > MAX_SAMPLE_DISTANCE || speed > MAX_PLAUSIBLE_SPEED {
            self.clear_detection();
            return false;
        }

        let extreme = if speed >= EXTREME_SPEED {
            let mut run = self.extreme.unwrap_or(Extreme {
                started: previous.at,
                distance: 0.0,
                samples: 0,
            });
            run.distance += distance;
            run.samples = run.samples.saturating_add(1);
            self.extreme = Some(run);
            run.samples >= 3
                && run.distance >= MIN_EXTREME_DISTANCE
                && now.duration_since(run.started) >= MIN_EXTREME_TIME
        } else {
            self.extreme = None;
            false
        };

        let shaken = if speed >= FAST_SPEED {
            let direction = [delta[0] / distance, delta[1] / distance];
            let mut shake = self.shake.filter(|shake| {
                now.duration_since(shake.started) <= SHAKE_WINDOW
                    && now.duration_since(shake.last_fast) <= SHAKE_PAUSE
            });
            if let Some(leg) = &mut shake {
                let alignment = direction[0] * leg.direction[0] + direction[1] * leg.direction[1];
                if alignment >= 0.4 {
                    leg.leg_distance += distance;
                } else if alignment <= -0.65 && leg.leg_distance >= MIN_LEG_DISTANCE {
                    leg.direction = direction;
                    leg.leg_distance = distance;
                    leg.reversals += 1;
                } else {
                    shake = None;
                }
            }
            let shake = shake.unwrap_or(Shake {
                direction,
                leg_distance: distance,
                reversals: 0,
                started: previous.at,
                last_fast: now,
            });
            self.shake = Some(Shake {
                last_fast: now,
                ..shake
            });
            shake.reversals >= 2 && shake.leg_distance >= MIN_LEG_DISTANCE
        } else {
            if self
                .shake
                .is_some_and(|shake| now.duration_since(shake.last_fast) > SHAKE_PAUSE)
            {
                self.shake = None;
            }
            false
        };
        if !extreme && !shaken {
            return false;
        }
        self.started = Some(now);
        self.cooldown_until = Some(now + GROW + HOLD + SHRINK + COOLDOWN);
        self.clear_detection();
        true
    }

    /// Evaluates a cubic ease-in-out envelope without changing state. Both
    /// ends have zero slope, avoiding a size jump on the first timer frame.
    pub fn frame(&self, now: Instant) -> CursorFrame {
        let Some(started) = self.started else {
            return CursorFrame {
                amount: 0.0,
                active: false,
            };
        };
        let elapsed = now.saturating_duration_since(started);
        let amount = if elapsed < GROW {
            smoothstep(elapsed.as_secs_f64() / GROW.as_secs_f64())
        } else if elapsed < GROW + HOLD {
            1.0
        } else if elapsed < GROW + HOLD + SHRINK {
            1.0 - smoothstep((elapsed - GROW - HOLD).as_secs_f64() / SHRINK.as_secs_f64())
        } else {
            return CursorFrame {
                amount: 0.0,
                active: false,
            };
        };
        CursorFrame {
            amount: amount as f32,
            active: true,
        }
    }

    /// Clears both the input history and animation, for a tool/context change.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    fn clear_detection(&mut self) {
        self.shake = None;
        self.extreme = None;
    }
}

fn smoothstep(progress: f64) -> f64 {
    let t = progress.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(motion: &mut CursorMotion, start: Instant, scale: f32, samples: &[(u64, i32)]) -> bool {
        samples.iter().fold(false, |triggered, &(millis, x)| {
            motion.record(
                POINT {
                    x: (x as f32 * scale).round() as i32,
                    y: -200,
                },
                scale,
                start + Duration::from_millis(millis),
            ) || triggered
        })
    }

    #[test]
    fn deliberate_fast_shake_triggers_at_common_scales_and_negative_origins() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut motion = CursorMotion::new();
            let start = Instant::now();
            assert!(feed(
                &mut motion,
                start,
                scale,
                &[(0, -800), (16, -704), (32, -800), (48, -704)]
            ));
            assert!(motion.frame(start + Duration::from_millis(49)).active);
        }
    }

    #[test]
    fn ordinary_drawing_one_reversal_and_short_jitter_do_not_trigger() {
        for samples in [
            vec![(0, 0), (16, 32), (32, 0), (48, 32), (64, 0)],
            vec![(0, 0), (16, 96), (32, 0), (48, -96), (64, -192)],
            vec![(0, 0), (4, 24), (8, 0), (12, 24), (16, 0), (20, 24)],
        ] {
            let mut motion = CursorMotion::new();
            assert!(!feed(&mut motion, Instant::now(), 1.0, &samples));
        }
    }

    #[test]
    fn extreme_sweep_needs_multiple_plausible_samples() {
        let mut motion = CursorMotion::new();
        let start = Instant::now();
        assert!(!feed(
            &mut motion,
            start,
            1.0,
            &[(0, 0), (8, 128), (16, 256)]
        ));
        assert!(feed(&mut motion, start, 1.0, &[(24, 384)]));
    }

    #[test]
    fn isolated_warps_long_gaps_and_dpi_changes_reset_detection() {
        for samples in [
            vec![(0, 0), (16, 2000), (32, 0), (48, 2000)],
            vec![(0, 0), (4, 400), (8, 0), (12, 400)],
            vec![(0, 0), (16, 96), (150, 0), (166, 96)],
        ] {
            let mut motion = CursorMotion::new();
            assert!(!feed(&mut motion, Instant::now(), 1.0, &samples));
        }
        let mut motion = CursorMotion::new();
        let start = Instant::now();
        assert!(!feed(&mut motion, start, 1.0, &[(0, 0), (16, 96)]));
        assert!(!feed(&mut motion, start, 2.0, &[(32, 0), (48, 96)]));
        assert!(!motion.frame(start + Duration::from_millis(48)).active);
    }

    #[test]
    fn animation_has_exact_boundaries_and_soft_growth_and_shrink() {
        let mut motion = CursorMotion::new();
        let start = Instant::now();
        assert!(!motion.frame(start).active);
        assert!(feed(
            &mut motion,
            start,
            1.0,
            &[(0, 0), (16, 96), (32, 0), (48, 96)]
        ));
        let triggered = start + Duration::from_millis(48);
        assert_eq!(
            motion.frame(triggered),
            CursorFrame {
                amount: 0.0,
                active: true
            }
        );
        assert!(motion.frame(triggered + Duration::from_millis(1)).amount < 0.001);
        assert!((motion.frame(triggered + GROW / 2).amount - 0.5).abs() < 0.00001);
        assert_eq!(motion.frame(triggered + GROW).amount, 1.0);
        assert_eq!(motion.frame(triggered + GROW + HOLD).amount, 1.0);
        assert!((motion.frame(triggered + GROW + HOLD + SHRINK / 2).amount - 0.5).abs() < 0.00001);
        let deadline = triggered + GROW + HOLD + SHRINK;
        assert!(motion.frame(deadline - Duration::from_millis(1)).amount < 0.001);
        assert_eq!(
            motion.frame(deadline),
            CursorFrame {
                amount: 0.0,
                active: false
            }
        );
    }

    #[test]
    fn continued_motion_cannot_restart_animation_and_reset_clears_everything() {
        let mut motion = CursorMotion::new();
        let start = Instant::now();
        assert!(feed(
            &mut motion,
            start,
            1.0,
            &[(0, 0), (16, 96), (32, 0), (48, 96)]
        ));
        let triggered = motion.started;
        assert!(!feed(
            &mut motion,
            start,
            1.0,
            &[(64, 0), (80, 96), (96, 0), (112, 96)]
        ));
        assert_eq!(motion.started, triggered);
        motion.reset();
        assert_eq!(motion.started, None);
        assert!(!motion.frame(start).active);
    }

    #[test]
    fn frequent_packets_accumulate_and_out_of_order_samples_are_ignored() {
        let mut motion = CursorMotion::new();
        let start = Instant::now();
        assert!(!feed(
            &mut motion,
            start,
            1.0,
            &[(0, 0), (1, 8), (2, 16), (3, 24)]
        ));
        assert_eq!(motion.sample.unwrap().position[0], 0.0);
        assert!(!feed(&mut motion, start, 1.0, &[(4, 32), (3, 1000)]));
        assert_eq!(motion.sample.unwrap().position[0], 32.0);
        assert!(!motion.record(
            POINT { x: 0, y: 0 },
            f32::NAN,
            start + Duration::from_millis(8)
        ));
        assert!(motion.sample.is_none());
    }
}
