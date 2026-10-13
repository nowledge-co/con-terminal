use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::motion::MotionValue;
use gpui::Hsla;

const ARRIVAL: Duration = Duration::from_millis(220);
const DEPARTURE: Duration = Duration::from_millis(160);

pub(super) struct DepartingTab {
    pub id: u64,
    pub title: String,
    pub icon: &'static str,
    pub colors: Option<(Hsla, Hsla)>,
    following: Vec<u64>,
    anchor: Option<u64>,
    motion: MotionValue,
    pub weight: f32,
}

#[derive(Default)]
pub(super) struct TabSlotMotion {
    initialized: bool,
    live: Vec<u64>,
    order: Vec<u64>,
    weights: HashMap<u64, MotionValue>,
    colors: HashMap<u64, (Hsla, Hsla)>,
    pub departing: Vec<DepartingTab>,
}

impl TabSlotMotion {
    pub fn sync(&mut self, ids: impl Iterator<Item = u64> + Clone, animate: bool, now: Instant) {
        if self.initialized
            && self.live.iter().copied().eq(ids.clone())
            && (animate
                || (self.departing.is_empty()
                    && !self.weights.values().any(MotionValue::is_animating)))
        {
            return;
        }
        let next: Vec<_> = ids.collect();
        // A restored/imported workspace is a new scene, not a burst of new tabs.
        let survivor_order_matches = self
            .live
            .iter()
            .filter(|id| next.contains(id))
            .eq(next.iter().filter(|id| self.live.contains(id)));
        let animate = animate
            && survivor_order_matches
            && (!self.initialized || next.iter().any(|id| self.live.contains(id)));
        self.weights.retain(|id, _| next.contains(id));
        self.colors.retain(|id, _| next.contains(id));
        if !animate {
            self.departing.clear();
        }
        if self.departing.is_empty() {
            self.order = next.clone();
        } else {
            self.order
                .retain(|id| next.contains(id) || self.departing.iter().any(|exit| exit.id == *id));
            for (index, id) in next.iter().enumerate() {
                if !self.order.contains(id) {
                    let insert_at = next[index + 1..]
                        .iter()
                        .find_map(|following| {
                            self.order
                                .iter()
                                .position(|candidate| candidate == following)
                        })
                        .unwrap_or(self.order.len());
                    self.order.insert(insert_at, *id);
                }
            }
            self.departing
                .sort_by_key(|exit| self.order.iter().position(|id| *id == exit.id));
        }
        for exit in &mut self.departing {
            exit.anchor = exit.following.iter().copied().find(|id| next.contains(id));
        }
        for &id in &next {
            let weight = self.weights.entry(id).or_insert_with(|| {
                MotionValue::new(if self.initialized && animate {
                    0.0
                } else {
                    1.0
                })
            });
            if !animate {
                *weight = MotionValue::new(1.0);
            } else if !self.live.contains(&id) {
                weight.set_target_at(1.0, ARRIVAL, now);
            }
        }
        self.live = next;
        self.initialized = true;
    }

    // Only inert chrome survives logical close; no terminal/entity/process is retained.
    pub fn depart(&mut self, id: u64, title: String, icon: &'static str, now: Instant) {
        let Some(index) = self.live.iter().position(|candidate| *candidate == id) else {
            return;
        };
        let Some(mut motion) = self.weights.remove(&id) else {
            return;
        };
        let weight = motion.current_at(now);
        motion.set_target_at(0.0, DEPARTURE, now);
        self.departing.push(DepartingTab {
            id,
            title,
            icon,
            colors: self.colors.remove(&id),
            following: self.live[index + 1..].to_vec(),
            anchor: self.live.get(index + 1).copied(),
            motion,
            weight,
        });
    }

    pub fn advance(&mut self, now: Instant, reduce_motion: bool) -> (f32, bool) {
        let mut needs_frame = false;
        let mut units = 0.0;
        for motion in self.weights.values_mut() {
            units += motion.update_at(now, reduce_motion, || needs_frame = true);
        }
        for exit in &mut self.departing {
            exit.weight = exit
                .motion
                .update_at(now, reduce_motion, || needs_frame = true);
            units += exit.weight;
        }
        let before = self.departing.len();
        self.departing.retain(|exit| exit.motion.is_animating());
        if self.departing.len() != before {
            self.order.retain(|id| {
                self.live.contains(id) || self.departing.iter().any(|exit| exit.id == *id)
            });
        }
        (units, needs_frame)
    }

    pub fn weight(&self, id: u64, now: Instant) -> f32 {
        self.weights
            .get(&id)
            .map_or(1.0, |motion| motion.current_at(now))
    }

    pub fn remember_colors(&mut self, id: u64, background: Hsla, ink: Hsla) {
        self.colors.insert(id, (background, ink));
    }

    pub fn before(&self, exit: &DepartingTab) -> Option<u64> {
        exit.anchor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrival_expands_space_and_idle_does_not_demand_frames() {
        let start = Instant::now();
        let mut slots = TabSlotMotion::default();
        slots.sync([1, 2].into_iter(), true, start);
        assert_eq!(slots.advance(start, false), (2.0, false));
        slots.sync([1, 2, 3].into_iter(), true, start);
        assert_eq!(slots.weight(3, start), 0.0);
        let mid = start + Duration::from_millis(60);
        let (units, needs_frame) = slots.advance(mid, false);
        assert!(units > 2.0 && units < 3.0 && needs_frame);
        slots.sync([1, 2, 3].into_iter(), true, mid);
        assert_eq!(slots.advance(start + ARRIVAL, false), (3.0, false));
        assert_eq!(
            slots.advance(start + Duration::from_secs(1), false),
            (3.0, false)
        );
    }

    #[test]
    fn closing_during_arrival_retargets_from_current_space_and_discards_metadata() {
        let start = Instant::now();
        let mut slots = TabSlotMotion::default();
        slots.sync([1, 2].into_iter(), true, start);
        slots.sync([1, 2, 3].into_iter(), true, start);
        let close = start + Duration::from_millis(40);
        let before = slots.weight(3, close);
        slots.depart(3, "third".into(), "phosphor/terminal.svg", close);
        slots.sync([1, 2].into_iter(), true, close);
        assert_eq!(slots.departing[0].weight, before);
        assert_eq!(slots.advance(close, false).0, 2.0 + before);
        assert_eq!(slots.advance(close + DEPARTURE, false), (2.0, false));
        assert!(slots.departing.is_empty());
        assert_eq!(slots.order, slots.live);
    }

    #[test]
    fn multiple_middle_closures_keep_the_next_surviving_anchor() {
        let start = Instant::now();
        let mut slots = TabSlotMotion::default();
        slots.sync([1, 2, 3, 4].into_iter(), true, start);
        for id in [2, 3] {
            slots.depart(id, id.to_string(), "phosphor/terminal.svg", start);
        }
        slots.sync([1, 4].into_iter(), true, start);
        assert_eq!(slots.before(&slots.departing[0]), Some(4));
        assert_eq!(slots.before(&slots.departing[1]), Some(4));
        assert_eq!(slots.advance(start, false).0, 4.0);
    }

    #[test]
    fn reverse_order_closures_do_not_swap_departing_labels() {
        let start = Instant::now();
        let mut slots = TabSlotMotion::default();
        slots.sync([1, 2, 3, 4].into_iter(), true, start);
        slots.depart(3, "three".into(), "phosphor/terminal.svg", start);
        slots.sync([1, 2, 4].into_iter(), true, start);
        slots.depart(2, "two".into(), "phosphor/terminal.svg", start);
        slots.sync([1, 4].into_iter(), true, start);
        assert_eq!(
            slots
                .departing
                .iter()
                .map(|exit| exit.id)
                .collect::<Vec<_>>(),
            [2, 3]
        );
        slots.sync([4, 1].into_iter(), true, start);
        assert!(slots.departing.is_empty());
        assert_eq!(slots.advance(start, false), (2.0, false));
    }

    #[test]
    fn reduced_motion_and_dragging_snap_without_retaining_exits() {
        let start = Instant::now();
        let mut slots = TabSlotMotion::default();
        slots.sync([1, 2].into_iter(), true, start);
        slots.depart(1, "first".into(), "phosphor/terminal.svg", start);
        slots.sync([2, 3].into_iter(), true, start);
        assert_eq!(slots.advance(start, true), (2.0, false));
        assert!(slots.departing.is_empty());
        slots.sync([3, 2, 4].into_iter(), false, start);
        assert_eq!(slots.advance(start, false), (3.0, false));
    }

    #[test]
    fn replacing_a_workspace_does_not_collapse_the_new_scene_to_zero_width() {
        let start = Instant::now();
        let mut slots = TabSlotMotion::default();
        slots.sync([1, 2].into_iter(), true, start);
        slots.depart(1, "old".into(), "phosphor/terminal.svg", start);
        slots.sync([10, 11, 12].into_iter(), true, start);
        assert_eq!(slots.advance(start, false), (3.0, false));
        assert!(slots.departing.is_empty());
    }
}
