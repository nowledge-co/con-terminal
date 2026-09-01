pub(crate) struct MouseButtonSequence<M> {
    payload: Option<M>,
}

impl<M> Default for MouseButtonSequence<M> {
    fn default() -> Self {
        Self { payload: None }
    }
}

impl<M> MouseButtonSequence<M> {
    pub(crate) fn is_active(&self) -> bool {
        self.payload.is_some()
    }

    #[cfg(any(target_os = "windows", target_os = "linux"))]
    pub(crate) fn payload(&self) -> Option<&M> {
        self.payload.as_ref()
    }

    pub(crate) fn begin(&mut self, payload: M) {
        debug_assert!(self.payload.is_none());
        self.payload = Some(payload);
    }

    pub(crate) fn finish(&mut self) -> Option<M> {
        self.payload.take()
    }
}

#[cfg(test)]
mod tests {
    use super::MouseButtonSequence;

    #[test]
    fn button_sequences_finish_independently_and_once() {
        let mut left = MouseButtonSequence::default();
        let mut right = MouseButtonSequence::default();

        left.begin(1);
        right.begin(2);

        assert_eq!(left.finish(), Some(1));
        assert!(!left.is_active());
        assert!(right.is_active());
        assert_eq!(left.finish(), None);
        assert_eq!(right.finish(), Some(2));
        assert_eq!(right.finish(), None);
    }
}
