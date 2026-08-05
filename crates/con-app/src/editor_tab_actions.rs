//! Reusable editor tab index finder — pure function, no GPUI dependencies.
//!
//! Given a slice of which tabs are editor tabs, the active tab index, and the
//! last-activated editor tab index (maintained by workspace), determines which
//! editor tab to reuse for an "open in editor tab" action.
//!
//! Semantics:
//! - `last_editor_tab` is valid (in bounds + `is_editor_tab[i] == true`) → return it
//! - Else if `active_tab` is itself an editor tab → return `active_tab`
//! - Else → `None` (caller should create a new editor tab)

/// Returns the index of the editor tab to reuse, or `None` if a new tab is needed.
///
/// - `last_editor_tab` valid (in range + `is_editor_tab[i] == true`) → returns it
/// - Otherwise `active_tab` is editor → returns `active_tab`
/// - Otherwise → `None`
pub fn reusable_editor_tab_index(
    is_editor_tab: &[bool],
    active_tab: usize,
    last_editor_tab: Option<usize>,
) -> Option<usize> {
    // Check if last_editor_tab is valid and points to an editor tab
    if let Some(idx) = last_editor_tab {
        if idx < is_editor_tab.len() && is_editor_tab[idx] {
            return Some(idx);
        }
    }

    // Fall back to active_tab if it is an editor tab
    if active_tab < is_editor_tab.len() && is_editor_tab[active_tab] {
        return Some(active_tab);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_last_editor_tab_valid_returns_it() {
        // last_editor_tab points to a valid editor tab → returns it even if active_tab is not editor
        let is_editor_tab = [false, true, false, false];
        let active_tab = 0;
        let last_editor_tab = Some(1);
        assert_eq!(reusable_editor_tab_index(&is_editor_tab, active_tab, last_editor_tab), Some(1));
    }

    #[test]
    fn test_last_editor_tab_out_of_bounds_falls_back() {
        // last_editor_tab is out of bounds → falls back to active_tab
        let is_editor_tab = [false, true, false];
        let active_tab = 1; // active is editor
        let last_editor_tab = Some(99); // out of bounds
        assert_eq!(reusable_editor_tab_index(&is_editor_tab, active_tab, last_editor_tab), Some(1));
    }

    #[test]
    fn test_last_editor_tab_out_of_bounds_no_fallback_returns_none() {
        // last_editor_tab out of bounds AND active_tab is not editor → None
        let is_editor_tab = [false, false, false];
        let active_tab = 0; // not editor
        let last_editor_tab = Some(99);
        assert_eq!(reusable_editor_tab_index(&is_editor_tab, active_tab, last_editor_tab), None);
    }

    #[test]
    fn test_last_editor_tab_not_editor_falls_back() {
        // last_editor_tab points to non-editor → falls back to active_tab
        let is_editor_tab = [true, false, true]; // index 1 is not editor
        let active_tab = 0; // editor
        let last_editor_tab = Some(1); // points to terminal tab
        assert_eq!(reusable_editor_tab_index(&is_editor_tab, active_tab, last_editor_tab), Some(0));
    }

    #[test]
    fn test_last_editor_tab_not_editor_falls_back_to_non_editor_returns_none() {
        // last_editor_tab points to non-editor AND active_tab is not editor → None
        let is_editor_tab = [false, false, true];
        let active_tab = 0; // not editor
        let last_editor_tab = Some(1); // not editor
        assert_eq!(reusable_editor_tab_index(&is_editor_tab, active_tab, last_editor_tab), None);
    }

    #[test]
    fn test_active_tab_is_editor_returns_active() {
        // No valid last_editor_tab, but active_tab itself is editor → returns active_tab
        let is_editor_tab = [false, true, false];
        let active_tab = 1;
        let last_editor_tab = None;
        assert_eq!(reusable_editor_tab_index(&is_editor_tab, active_tab, last_editor_tab), Some(1));
    }

    #[test]
    fn test_active_tab_is_editor_last_editor_also_valid_returns_last() {
        // Both last_editor_tab and active_tab are valid → prefer last_editor_tab
        let is_editor_tab = [true, true, false];
        let active_tab = 0;
        let last_editor_tab = Some(1);
        assert_eq!(reusable_editor_tab_index(&is_editor_tab, active_tab, last_editor_tab), Some(1));
    }

    #[test]
    fn test_no_editor_tabs_returns_none() {
        // No editor tabs at all → None
        let is_editor_tab = [false, false, false];
        let active_tab = 1;
        let last_editor_tab = Some(0);
        assert_eq!(reusable_editor_tab_index(&is_editor_tab, active_tab, last_editor_tab), None);
    }

    #[test]
    fn test_empty_array_returns_none() {
        // Empty array → None regardless of active_tab or last_editor_tab
        let is_editor_tab: [bool; 0] = [];
        let active_tab = 0;
        let last_editor_tab = Some(0);
        assert_eq!(reusable_editor_tab_index(&is_editor_tab, active_tab, last_editor_tab), None);
    }

    #[test]
    fn test_last_editor_tab_none_active_not_editor_returns_none() {
        // last_editor_tab = None, active_tab is not editor → None
        let is_editor_tab = [false, false, true];
        let active_tab = 0;
        let last_editor_tab = None;
        assert_eq!(reusable_editor_tab_index(&is_editor_tab, active_tab, last_editor_tab), None);
    }
}
