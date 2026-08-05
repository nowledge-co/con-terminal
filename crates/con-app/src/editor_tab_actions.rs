//! Reusable editor tab index finder — pure function, no GPUI dependencies.
//!
//! Given the stable ids of the current tabs, a slice of which tabs are
//! editor-only tabs, the active tab index, and the last-activated editor tab id
//! (maintained by workspace), determines which editor tab to reuse for an
//! "open in editor tab" action.
//!
//! Semantics:
//! - `last_editor_tab_id` still points to an editor-only tab → return it
//! - Else if `active_tab` is itself an editor tab → return `active_tab`
//! - Else if any editor-only tab exists → return the first editor tab at or
//!   after the active tab, wrapping around
//! - Else → `None` (caller should create a new editor tab)

/// Returns the index of the editor tab to reuse, or `None` if a new tab is needed.
///
/// - `last_editor_tab_id` still resolves to an editor tab → returns it
/// - Otherwise `active_tab` is editor → returns `active_tab`
/// - Otherwise the first editor tab at/after the active tab, wrapping around
/// - Otherwise → `None`
pub fn reusable_editor_tab_index(
    tab_ids: &[u64],
    is_editor_tab: &[bool],
    active_tab: usize,
    last_editor_tab_id: Option<u64>,
) -> Option<usize> {
    if tab_ids.len() != is_editor_tab.len() {
        return None;
    }

    // Track the last editor by stable tab identity, not by index. Indices shift
    // whenever the user closes or reorders tabs.
    if let Some(tab_id) = last_editor_tab_id {
        if let Some(idx) = tab_ids.iter().position(|id| *id == tab_id)
            && is_editor_tab[idx]
        {
            return Some(idx);
        }
    }

    // Fall back to active_tab if it is an editor tab
    if active_tab < is_editor_tab.len() && is_editor_tab[active_tab] {
        return Some(active_tab);
    }

    if is_editor_tab.is_empty() {
        return None;
    }

    let start = active_tab % is_editor_tab.len();
    for offset in 0..is_editor_tab.len() {
        let idx = (start + offset) % is_editor_tab.len();
        if is_editor_tab[idx] {
            return Some(idx);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_editor_tab_id_valid_returns_it() {
        let is_editor_tab = [false, true, false, false];
        let tab_ids = [10, 11, 12, 13];
        let active_tab = 0;
        let last_editor_tab_id = Some(11);
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            Some(1)
        );
    }

    #[test]
    fn stale_last_editor_tab_id_falls_back_to_active_editor() {
        let is_editor_tab = [false, true, false];
        let tab_ids = [10, 11, 12];
        let active_tab = 1; // active is editor
        let last_editor_tab_id = Some(99); // no longer present
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            Some(1)
        );
    }

    #[test]
    fn stale_last_editor_tab_id_falls_back_to_existing_editor() {
        let is_editor_tab = [false, false, true];
        let tab_ids = [10, 11, 12];
        let active_tab = 0; // not editor
        let last_editor_tab_id = Some(99);
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            Some(2)
        );
    }

    #[test]
    fn last_editor_tab_id_now_non_editor_falls_back() {
        let is_editor_tab = [true, false, true]; // index 1 is not editor
        let tab_ids = [10, 11, 12];
        let active_tab = 0; // editor
        let last_editor_tab_id = Some(11);
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            Some(0)
        );
    }

    #[test]
    fn last_editor_tab_id_now_non_editor_falls_back_to_existing_editor() {
        let is_editor_tab = [false, false, true];
        let tab_ids = [10, 11, 12];
        let active_tab = 0; // not editor
        let last_editor_tab_id = Some(11); // not editor
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            Some(2)
        );
    }

    #[test]
    fn active_tab_is_editor_returns_active() {
        let is_editor_tab = [false, true, false];
        let tab_ids = [10, 11, 12];
        let active_tab = 1;
        let last_editor_tab_id = None;
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            Some(1)
        );
    }

    #[test]
    fn active_tab_is_editor_last_editor_also_valid_returns_last() {
        let is_editor_tab = [true, true, false];
        let tab_ids = [10, 11, 12];
        let active_tab = 0;
        let last_editor_tab_id = Some(11);
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            Some(1)
        );
    }

    #[test]
    fn no_editor_tabs_returns_none() {
        let is_editor_tab = [false, false, false];
        let tab_ids = [10, 11, 12];
        let active_tab = 1;
        let last_editor_tab_id = Some(10);
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            None
        );
    }

    #[test]
    fn empty_array_returns_none() {
        let is_editor_tab: [bool; 0] = [];
        let tab_ids: [u64; 0] = [];
        let active_tab = 0;
        let last_editor_tab_id = Some(10);
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            None
        );
    }

    #[test]
    fn no_last_active_not_editor_returns_existing_editor() {
        let is_editor_tab = [false, false, true];
        let tab_ids = [10, 11, 12];
        let active_tab = 0;
        let last_editor_tab_id = None;
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            Some(2)
        );
    }

    #[test]
    fn fallback_wraps_from_active_tab() {
        let is_editor_tab = [true, false, false];
        let tab_ids = [10, 11, 12];
        let active_tab = 2;
        let last_editor_tab_id = None;
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, active_tab, last_editor_tab_id),
            Some(0)
        );
    }

    #[test]
    fn fallback_tolerates_out_of_bounds_active_tab() {
        let is_editor_tab = [false, true, false];
        let tab_ids = [10, 11, 12];
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, 99, None),
            Some(1)
        );
    }

    #[test]
    fn mismatched_metadata_returns_none() {
        let is_editor_tab = [true, false, false];
        let tab_ids = [10, 11];
        assert_eq!(
            reusable_editor_tab_index(&tab_ids, &is_editor_tab, 0, Some(10)),
            None
        );
    }
}
