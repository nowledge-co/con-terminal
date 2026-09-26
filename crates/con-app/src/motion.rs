use std::time::{Duration, Instant};

use gpui::{App, Window, px};

#[derive(Clone, Debug)]
pub struct MotionValue {
    current: f32,
    from: f32,
    target: f32,
    started_at: Option<Instant>,
    duration: Duration,
}

impl MotionValue {
    pub fn new(value: f32) -> Self {
        Self {
            current: value,
            from: value,
            target: value,
            started_at: None,
            duration: Duration::from_millis(180),
        }
    }

    pub fn is_animating(&self) -> bool {
        self.started_at.is_some()
    }

    pub fn set_target(&mut self, target: f32, duration: Duration) {
        let current = self.current();
        self.current = current;
        self.from = current;
        self.target = target;
        self.duration = duration;
        self.started_at = if (current - target).abs() > 0.001 {
            Some(Instant::now())
        } else {
            self.current = target;
            None
        };
    }

    pub fn restart(&mut self, from: f32, target: f32, duration: Duration) {
        self.current = from;
        self.from = from;
        self.target = target;
        self.duration = duration;
        self.started_at = if (from - target).abs() > 0.001 {
            Some(Instant::now())
        } else {
            None
        };
    }

    pub fn current(&self) -> f32 {
        let Some(started_at) = self.started_at else {
            return self.target;
        };

        let elapsed = started_at.elapsed();
        if elapsed >= self.duration || self.duration.is_zero() {
            return self.target;
        }

        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let eased = ease_out_quint(t.clamp(0.0, 1.0));
        self.from + ((self.target - self.from) * eased)
    }

    pub fn value(&mut self, window: &mut Window, cx: &App) -> f32 {
        self.update(cx.reduce_motion(), || window.request_animation_frame())
    }

    fn update(&mut self, reduce_motion: bool, request_frame: impl FnOnce()) -> f32 {
        if reduce_motion {
            self.current = self.target;
            self.from = self.target;
            self.started_at = None;
            return self.target;
        }

        let value = self.current();
        if let Some(started_at) = self.started_at {
            if started_at.elapsed() >= self.duration || self.duration.is_zero() {
                self.current = self.target;
                self.from = self.target;
                self.started_at = None;
            } else {
                self.current = value;
                request_frame();
            }
        } else {
            self.current = self.target;
        }
        value
    }
}

pub fn vertical_reveal_offset(progress: f32, distance: f32) -> gpui::Pixels {
    px((1.0 - progress).clamp(0.0, 1.0) * distance)
}

fn ease_out_quint(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(5)
}

#[cfg(test)]
mod tests {
    use super::MotionValue;
    use std::{cell::Cell, time::Duration};

    #[test]
    fn enabling_reduced_motion_during_motion_snaps_without_another_frame() {
        let mut motion = MotionValue::new(0.0);
        motion.restart(0.0, 1.0, Duration::from_secs(10));
        let requested = Cell::new(false);

        assert_eq!(motion.update(true, || requested.set(true)), 1.0);
        assert!(!motion.is_animating());
        assert!(!requested.get());
        assert_eq!(motion.current(), 1.0);
    }

    #[test]
    fn reduced_motion_keeps_an_already_settled_value_settled() {
        let mut motion = MotionValue::new(0.25);
        motion.set_target(0.25, Duration::from_secs(1));
        let requested = Cell::new(false);

        assert_eq!(motion.update(true, || requested.set(true)), 0.25);
        assert!(!motion.is_animating());
        assert!(!requested.get());
    }
}
