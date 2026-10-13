use std::time::{Duration, Instant};

use gpui::{App, SpringConfig, SpringState, Window, px};

/// Sidebar geometry uses an analytic, critically damped spring. Retargeting
/// preserves velocity; ordinary reveal/fade animations retain MotionValue.
#[derive(Clone, Debug)]
pub struct SidebarMotion {
    state: SpringState,
    target: f32,
    started_at: Option<Instant>,
}

impl SidebarMotion {
    const SPRING: SpringConfig = SpringConfig::new(625.0, 50.0, 1.0);
    const EPSILON: f32 = 0.0002;

    pub fn new(value: f32) -> Self {
        Self {
            state: SpringState {
                position: value,
                velocity: 0.0,
            },
            target: value,
            started_at: None,
        }
    }

    fn sample_at(&self, now: Instant) -> SpringState {
        self.started_at.map_or(self.state, |start| {
            Self::SPRING.step(
                self.state,
                self.target,
                now.saturating_duration_since(start).as_secs_f32(),
            )
        })
    }

    fn retarget_at(&mut self, target: f32, now: Instant) {
        if self.target == target {
            return;
        }
        self.state = self.sample_at(now);
        self.target = target;
        self.started_at = Some(now);
    }

    pub fn set_target(&mut self, target: f32) {
        self.retarget_at(target, Instant::now());
    }

    pub fn is_animating(&self) -> bool {
        self.started_at.is_some()
    }

    pub fn current(&self) -> f32 {
        self.sample_at(Instant::now()).position.clamp(0.0, 1.0)
    }

    pub fn value(&mut self, window: &mut Window, cx: &App) -> f32 {
        self.update_at(Instant::now(), cx.reduce_motion(), || {
            window.request_animation_frame()
        })
    }

    fn update_at(
        &mut self,
        now: Instant,
        reduce_motion: bool,
        request_frame: impl FnOnce(),
    ) -> f32 {
        let state = self.sample_at(now);
        if reduce_motion || Self::SPRING.is_settled(state, self.target, Self::EPSILON) {
            *self = Self::new(self.target);
            self.target
        } else {
            request_frame();
            state.position.clamp(0.0, 1.0)
        }
    }
}

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
        self.set_target_at(target, duration, Instant::now());
    }

    pub(crate) fn set_target_at(&mut self, target: f32, duration: Duration, now: Instant) {
        if duration.is_zero() {
            self.current = target;
            self.from = target;
            self.target = target;
            self.duration = duration;
            self.started_at = None;
            return;
        }
        let current = self.current_at(now);
        self.current = current;
        self.from = current;
        self.target = target;
        self.duration = duration;
        self.started_at = if (current - target).abs() > 0.001 {
            Some(now)
        } else {
            self.current = target;
            None
        };
    }

    pub fn restart(&mut self, from: f32, target: f32, duration: Duration) {
        let start = if duration.is_zero() { target } else { from };
        self.current = start;
        self.from = start;
        self.target = target;
        self.duration = duration;
        self.started_at = if !duration.is_zero() && (from - target).abs() > 0.001 {
            Some(Instant::now())
        } else {
            None
        };
    }

    pub fn current(&self) -> f32 {
        self.current_at(Instant::now())
    }

    pub(crate) fn current_at(&self, now: Instant) -> f32 {
        let Some(started_at) = self.started_at else {
            return self.target;
        };

        let elapsed = now.saturating_duration_since(started_at);
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
        self.update_at(Instant::now(), reduce_motion, request_frame)
    }

    pub(crate) fn update_at(
        &mut self,
        now: Instant,
        reduce_motion: bool,
        request_frame: impl FnOnce(),
    ) -> f32 {
        if reduce_motion {
            self.current = self.target;
            self.from = self.target;
            self.started_at = None;
            return self.target;
        }

        let value = self.current_at(now);
        if let Some(started_at) = self.started_at {
            if now.saturating_duration_since(started_at) >= self.duration || self.duration.is_zero()
            {
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
    use super::{MotionValue, SidebarMotion};
    use std::{
        cell::Cell,
        time::{Duration, Instant},
    };

    #[test]
    fn sidebar_reversal_preserves_position_and_velocity() {
        let start = Instant::now();
        let mut motion = SidebarMotion::new(0.0);
        motion.retarget_at(1.0, start);
        let reverse = start + Duration::from_millis(90);
        let before = motion.sample_at(reverse);
        assert!(before.position > 0.0 && before.velocity > 0.0);
        motion.retarget_at(0.0, reverse);
        assert_eq!(motion.sample_at(reverse), before);
        assert!(
            motion
                .sample_at(reverse + Duration::from_millis(100))
                .velocity
                < 0.0
        );
    }

    #[test]
    fn sidebar_sampling_is_frame_rate_independent_and_does_not_restart_same_target() {
        let start = Instant::now();
        let mut motion = SidebarMotion::new(0.0);
        motion.retarget_at(1.0, start);
        let original = motion.clone();
        for frame in 1..=30 {
            let now = start + Duration::from_millis(frame * 8);
            motion.update_at(now, false, || {});
            motion.retarget_at(1.0, now);
        }
        assert_eq!(
            motion.sample_at(start + Duration::from_millis(240)),
            original.sample_at(start + Duration::from_millis(240))
        );
    }

    #[test]
    fn sidebar_motion_settles_without_overshoot_or_idle_frames() {
        let start = Instant::now();
        let mut motion = SidebarMotion::new(0.0);
        motion.retarget_at(1.0, start);
        let mut last = 0.0;
        for frame in 1..=120 {
            let value = motion.update_at(start + Duration::from_millis(frame * 8), false, || {});
            assert!(value >= last && value <= 1.0);
            last = value;
        }
        assert_eq!(last, 1.0);
        assert!(!motion.is_animating());
        motion.update_at(start + Duration::from_secs(1), false, || {
            panic!("idle animation requested a frame")
        });
        motion.retarget_at(0.0, start + Duration::from_secs(1));
        assert_eq!(
            motion.update_at(start + Duration::from_millis(1010), true, || panic!(
                "reduced motion requested a frame"
            )),
            0.0
        );
        assert!(!motion.is_animating());
    }

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

    #[test]
    fn zero_duration_never_reports_a_transition_frame() {
        let mut motion = MotionValue::new(0.0);
        motion.set_target(1.0, Duration::ZERO);
        assert_eq!(motion.current(), 1.0);
        assert!(!motion.is_animating());

        motion.restart(1.0, 0.0, Duration::ZERO);
        assert_eq!(motion.current(), 0.0);
        assert!(!motion.is_animating());
    }
}
