pub(crate) struct MouseButtonSequence<M> {
    press_modifiers: Option<M>,
}

impl<M> Default for MouseButtonSequence<M> {
    fn default() -> Self {
        Self {
            press_modifiers: None,
        }
    }
}

impl<M> MouseButtonSequence<M> {
    pub(crate) fn is_active(&self) -> bool {
        self.press_modifiers.is_some()
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn press_modifiers(&self) -> Option<&M> {
        self.press_modifiers.as_ref()
    }

    pub(crate) fn press_sent(&mut self, modifiers: M) {
        debug_assert!(self.press_modifiers.is_none());
        self.press_modifiers = Some(modifiers);
    }

    pub(crate) fn finish(&mut self) -> Option<M> {
        self.press_modifiers.take()
    }
}

#[cfg(test)]
mod tests {
    use super::MouseButtonSequence;

    #[test]
    fn button_sequences_finish_independently_and_once() {
        let mut left = MouseButtonSequence::default();
        let mut right = MouseButtonSequence::default();

        left.press_sent(1);
        right.press_sent(2);

        assert_eq!(left.finish(), Some(1));
        assert!(!left.is_active());
        assert!(right.is_active());
        assert_eq!(left.finish(), None);
        assert_eq!(right.finish(), Some(2));
        assert_eq!(right.finish(), None);
    }
}
