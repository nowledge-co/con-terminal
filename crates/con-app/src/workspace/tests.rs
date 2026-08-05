use super::chrome::agent_panel_motion_target_for_agent_request;
use super::{
    ActivationFocusTarget, ActivePaneFocusTarget, EditorFileCloseOutcome, EditorLineBoundary,
    FileTreeFocusSource, WorkspaceCloseIntent, activation_focus_target_for_reassertion,
    active_pane_focus_target, editor_file_close_outcome, editor_line_boundary_for_key,
    file_tree_root_for_focus, focused_editor_pane_key_fallback_allowed,
    resize_drag_should_continue, should_reveal_vertical_tab_rail_for_new_tab,
    should_show_activity_bar, workspace_close_intent, workspace_close_intent_for_close_tab,
};
use super::{
    ConWorkspace, SPLIT_PREVIEW_SEAM_THICKNESS, SplitDirection, SplitPlacement,
    TabRenameStateSnapshot, centered_drag_preview_origin, clamp_preview_origin_to_tab_bar,
    clear_pane_tab_promotion_drag_state, horizontal_tab_slot_from_drag,
    horizontal_tab_slot_from_position, is_dragged_tab_source, is_tab_strip_preview_active,
    normalize_tab_user_label, pane_drag_floating_preview_origin,
    pane_split_drop_target_from_position, pane_title_drag_tab_slot, pane_title_drag_to_tab_active,
    rebase_active_tab_for_insert, remap_drop_slot_for_current_order,
    remap_tab_rename_state_after_close, remap_tab_rename_state_after_reorder,
    split_preview_local_rect, split_preview_regions, tab_drag_overlay_probe_position,
    tab_drag_preview_origin, tab_like_drag_preview_size, tab_rename_commit_label,
    tab_rename_initial_label, trailing_drop_slot_from_position,
};
use crate::activity_bar::ActivitySlot;
use crate::sidebar::{
    DraggedTabPreviewConstraint, constrained_drag_preview_x_shift, constrained_drag_preview_y_shift,
};
use gpui::{Bounds, MouseButton, Point, Size, px};
use std::path::{Path, PathBuf};

#[test]
fn file_tree_root_for_terminal_focus_uses_terminal_cwd() {
    let root = file_tree_root_for_focus(
        FileTreeFocusSource::Terminal {
            cwd: Some("/tmp/project"),
        },
        Some(Path::new("/tmp/old")),
    );
    assert_eq!(root, Some(PathBuf::from("/tmp/project")));
}

#[test]
fn file_tree_root_for_editor_focus_uses_file_parent_dir_without_existing_root() {
    let root = file_tree_root_for_focus(
        FileTreeFocusSource::Editor {
            file_path: Some(Path::new("/tmp/project/src/main.rs")),
        },
        None,
    );
    assert_eq!(root, Some(PathBuf::from("/tmp/project/src")));
}

#[test]
fn file_tree_root_for_editor_focus_preserves_existing_root_that_contains_file() {
    let root = file_tree_root_for_focus(
        FileTreeFocusSource::Editor {
            file_path: Some(Path::new("/a/b/c/d/e.txt")),
        },
        Some(Path::new("/a/b")),
    );
    assert_eq!(root, Some(PathBuf::from("/a/b")));
}

#[test]
fn workspace_cmd_w_closes_editor_file_before_pane_tab_or_window() {
    assert_eq!(
        workspace_close_intent(2, Some(2), 4),
        WorkspaceCloseIntent::CloseEditorFile
    );
    assert_eq!(
        workspace_close_intent(1, Some(1), 1),
        WorkspaceCloseIntent::CloseEditorFile
    );
    assert_eq!(
        workspace_close_intent(2, Some(0), 4),
        WorkspaceCloseIntent::ClosePane
    );
    assert_eq!(
        workspace_close_intent(1, None, 4),
        WorkspaceCloseIntent::CloseTab
    );
    assert_eq!(
        workspace_close_intent(1, Some(0), 1),
        WorkspaceCloseIntent::CloseWindow
    );
}

#[test]
fn workspace_cmd_w_uses_focused_editor_pane_when_keyboard_focus_is_elsewhere() {
    assert_eq!(
        workspace_close_intent_for_close_tab(1, None, Some(3), 1),
        WorkspaceCloseIntent::CloseEditorFile
    );
    assert_eq!(
        workspace_close_intent_for_close_tab(1, None, None, 3),
        WorkspaceCloseIntent::CloseTab
    );
}

#[test]
fn ctrl_a_and_ctrl_e_have_workspace_fallback_for_focused_editor_pane() {
    assert_eq!(
        editor_line_boundary_for_key("a", true, false, false, false),
        Some(EditorLineBoundary::Start)
    );
    assert_eq!(
        editor_line_boundary_for_key("e", true, false, false, false),
        Some(EditorLineBoundary::End)
    );
    assert_eq!(
        editor_line_boundary_for_key("a", false, false, false, false),
        None
    );
}

#[test]
fn focused_editor_pane_key_fallback_does_not_steal_input_focus() {
    assert!(focused_editor_pane_key_fallback_allowed(false));
    assert!(!focused_editor_pane_key_fallback_allowed(true));
}

#[test]
fn activity_bar_visibility_tracks_left_sidebar_visibility() {
    assert!(!should_show_activity_bar(false, ActivitySlot::Files));
    assert!(!should_show_activity_bar(false, ActivitySlot::Search));
    assert!(should_show_activity_bar(true, ActivitySlot::Files));
    assert!(should_show_activity_bar(true, ActivitySlot::Search));
}

#[test]
fn resize_drag_stops_when_left_button_is_no_longer_pressed() {
    assert!(resize_drag_should_continue(Some(MouseButton::Left)));
    assert!(!resize_drag_should_continue(None));
    assert!(!resize_drag_should_continue(Some(MouseButton::Right)));
}

#[test]
fn editor_only_tabs_focus_editor_when_no_terminal_exists() {
    assert_eq!(
        active_pane_focus_target(true, true),
        ActivePaneFocusTarget::Terminal
    );
    assert_eq!(
        active_pane_focus_target(false, true),
        ActivePaneFocusTarget::Editor
    );
    assert_eq!(
        active_pane_focus_target(false, false),
        ActivePaneFocusTarget::Workspace
    );
}

#[test]
fn window_activation_focus_prefers_input_bar_then_agent_then_editor_then_terminal() {
    assert_eq!(
        activation_focus_target_for_reassertion(true, true, true, true),
        ActivationFocusTarget::InputBar
    );
    assert_eq!(
        activation_focus_target_for_reassertion(false, true, true, true),
        ActivationFocusTarget::AgentInlineInput
    );
    assert_eq!(
        activation_focus_target_for_reassertion(false, false, true, false),
        ActivationFocusTarget::ActiveEditor
    );
    assert_eq!(
        activation_focus_target_for_reassertion(false, false, false, true),
        ActivationFocusTarget::ActiveEditor
    );
    assert_eq!(
        activation_focus_target_for_reassertion(false, false, false, false),
        ActivationFocusTarget::Terminal
    );
}

#[test]
fn editor_file_close_outcome_closes_single_empty_editor_pane_container() {
    assert_eq!(
        editor_file_close_outcome(1, false),
        EditorFileCloseOutcome::KeepEditorPane
    );
    assert_eq!(
        editor_file_close_outcome(2, true),
        EditorFileCloseOutcome::CloseEditorPane
    );
    assert_eq!(
        editor_file_close_outcome(1, true),
        EditorFileCloseOutcome::ReplaceEditorPaneWithTerminal
    );
}

#[test]
fn agent_request_opening_panel_drives_panel_motion_to_visible() {
    // Always drives to 1.0 regardless of current open state, so a stale
    // agent_panel_open flag (set without motion) is corrected on next request.
    assert_eq!(
        agent_panel_motion_target_for_agent_request(false),
        Some(1.0)
    );
    assert_eq!(agent_panel_motion_target_for_agent_request(true), Some(1.0));
}

#[test]
fn tab_drag_preview_origin_is_clamped_to_tab_bar_height() {
    let preview_size = Size {
        width: px(180.0),
        height: px(28.0),
    };
    let origin_below = Point {
        x: px(120.0),
        y: px(100.0),
    };

    assert_eq!(
        clamp_preview_origin_to_tab_bar(origin_below, preview_size, px(28.0), px(36.0)),
        Point {
            x: px(120.0),
            y: px(36.0)
        }
    );
}

#[test]
fn tab_drag_preview_origin_keeps_preview_inside_tab_bar_while_cursor_leaves() {
    assert_eq!(
        tab_drag_preview_origin(
            Point {
                x: px(300.0),
                y: px(120.0)
            },
            Size {
                width: px(180.0),
                height: px(28.0)
            },
            px(28.0),
            px(36.0),
        ),
        Point {
            x: px(210.0),
            y: px(36.0)
        }
    );
}

#[test]
fn dragged_tab_preview_y_shift_updates_with_current_mouse_position() {
    let constraint = DraggedTabPreviewConstraint {
        cursor_offset_y: px(12.0),
        top: px(28.0),
        height: px(36.0),
        preview_height: px(28.0),
        cursor_offset_x: px(0.0),
        left: px(0.0),
        bar_width: px(1000.0),
        preview_width: px(120.0),
    };

    assert_eq!(
        constrained_drag_preview_y_shift(px(120.0), constraint),
        px(-72.0)
    );
}

#[test]
fn dragged_tab_preview_y_shift_locks_to_source_tab_top() {
    let constraint = DraggedTabPreviewConstraint {
        cursor_offset_y: px(10.0),
        top: px(40.0),
        height: px(28.0),
        preview_height: px(28.0),
        cursor_offset_x: px(20.0),
        left: px(78.0),
        bar_width: px(600.0),
        preview_width: px(120.0),
    };

    // GPUI natural top at this mouse position is 190 (= 200 - 10).
    // The shift should move it back to the source tab top 40.
    assert_eq!(
        constrained_drag_preview_y_shift(px(200.0), constraint),
        px(-150.0)
    );
}

#[test]
fn dragged_tab_preview_x_shift_clamps_inside_tab_bar() {
    let constraint = DraggedTabPreviewConstraint {
        cursor_offset_y: px(10.0),
        top: px(40.0),
        height: px(28.0),
        preview_height: px(28.0),
        cursor_offset_x: px(20.0),
        left: px(78.0),
        bar_width: px(300.0),
        preview_width: px(120.0),
    };

    // Natural left 30 (= 50 - 20) is before the tab bar; shift to 78.
    assert_eq!(
        constrained_drag_preview_x_shift(px(50.0), constraint),
        px(48.0)
    );
    // Natural left 480 (= 500 - 20) is after max_left 258; shift left.
    assert_eq!(
        constrained_drag_preview_x_shift(px(500.0), constraint),
        px(-222.0)
    );
}

#[test]
fn horizontal_tab_reorder_probe_uses_locked_overlay_center_not_mouse_y() {
    let preview = super::TabDragPreviewState {
        title: "tab".into(),
        icon: "phosphor/terminal.svg",
        source_top: px(6.0),
        cursor_offset_x: px(40.0),
    };
    let probe = tab_drag_overlay_probe_position(
        Point {
            x: px(300.0),
            y: px(240.0),
        },
        &preview,
        Size {
            width: px(180.0),
            height: px(28.0),
        },
        px(78.0),
        px(1020.0),
    );

    assert_eq!(
        probe,
        Point {
            x: px(350.0),
            y: px(20.0)
        }
    );
}

#[test]
fn pane_drag_floating_preview_origin_places_title_under_cursor() {
    let preview_size = Size {
        width: px(120.0),
        height: px(28.0),
    };
    assert_eq!(
        pane_drag_floating_preview_origin(
            Point {
                x: px(644.0),
                y: px(634.0)
            },
            preview_size
        ),
        Point {
            x: px(584.0),
            y: px(620.0)
        }
    );
    assert_eq!(
        pane_drag_floating_preview_origin(
            Point {
                x: px(1200.0),
                y: px(80.0)
            },
            preview_size
        ),
        Point {
            x: px(1140.0),
            y: px(66.0)
        }
    );
}

#[test]
fn tab_drag_preview_uses_tab_like_size() {
    assert_eq!(
        tab_like_drag_preview_size(),
        Size {
            width: px(180.0),
            height: px(28.0)
        }
    );
}

#[test]
fn centered_drag_preview_origin_places_cursor_at_preview_center() {
    assert_eq!(
        centered_drag_preview_origin(
            Point {
                x: px(300.0),
                y: px(200.0)
            },
            Size {
                width: px(180.0),
                height: px(28.0)
            },
        ),
        Point {
            x: px(210.0),
            y: px(186.0)
        }
    );
}

#[test]
fn pane_title_drag_to_tab_preview_activates_tab_strip_without_gpui_drag() {
    let drag = super::PaneTitleDragState {
        title: "pane".into(),
        current_pos: Point {
            x: px(20.0),
            y: px(32.0),
        },
        active: true,
        target: Some(super::PaneDropTarget::NewTab { slot: 1 }),
    };

    assert!(pane_title_drag_to_tab_active(Some(&drag)));

    // Also active when dragging over a pane split target — tab strip
    // stays visible so the user can drag back up into it.
    let drag_split = super::PaneTitleDragState {
        title: "pane".into(),
        current_pos: Point {
            x: px(20.0),
            y: px(200.0),
        },
        active: true,
        target: None,
    };
    assert!(pane_title_drag_to_tab_active(Some(&drag_split)));

    // Not active when drag hasn't started yet.
    let drag_inactive = super::PaneTitleDragState {
        title: "pane".into(),
        current_pos: Point {
            x: px(2.0),
            y: px(2.0),
        },
        active: false,
        target: None,
    };
    assert!(!pane_title_drag_to_tab_active(Some(&drag_inactive)));
}

#[test]
fn pane_title_drag_to_tab_keeps_tab_strip_preview_active_without_gpui_tab_drag() {
    assert!(is_tab_strip_preview_active(false, true));
    assert!(is_tab_strip_preview_active(true, false));
    assert!(!is_tab_strip_preview_active(false, false));
}

#[test]
fn clearing_pane_tab_promotion_drop_removes_all_preview_state() {
    let mut pane_title_drag = Some(super::PaneTitleDragState {
        title: "pane".into(),
        current_pos: Point {
            x: px(160.0),
            y: px(44.0),
        },
        active: true,
        target: Some(super::PaneDropTarget::NewTab { slot: 2 }),
    });
    let mut tab_strip_drop_slot = Some(2);
    let mut tab_drag_target = Some(super::TabDragTarget::Reorder { slot: 2 });

    clear_pane_tab_promotion_drag_state(
        &mut pane_title_drag,
        &mut tab_strip_drop_slot,
        &mut tab_drag_target,
    );

    assert!(pane_title_drag.is_none());
    assert_eq!(tab_strip_drop_slot, None);
    assert_eq!(tab_drag_target, None);
}

#[test]
fn pane_tab_promotion_rebases_active_tab_before_insert() {
    assert_eq!(rebase_active_tab_for_insert(2, 0), 3);
    assert_eq!(rebase_active_tab_for_insert(2, 2), 3);
    assert_eq!(rebase_active_tab_for_insert(2, 3), 2);
}

#[test]
fn tab_drag_source_is_hidden_only_for_active_dragged_session() {
    assert!(is_dragged_tab_source(Some(42), 42));
    assert!(!is_dragged_tab_source(Some(42), 7));
    assert!(!is_dragged_tab_source(None, 42));
}

#[test]
fn normalize_tab_user_label_trims_and_clears_blank_values() {
    assert_eq!(normalize_tab_user_label(""), None);
    assert_eq!(normalize_tab_user_label("   \t  \n"), None);
    assert_eq!(
        normalize_tab_user_label("  hello  "),
        Some("hello".to_string())
    );
}

#[test]
fn tab_rename_commit_label_only_persists_user_edits() {
    assert_eq!(tab_rename_commit_label("Deploy", false), None);
    assert_eq!(
        tab_rename_commit_label("  Deploy  ", true),
        Some(Some("Deploy".to_string()))
    );
    assert_eq!(tab_rename_commit_label("   ", true), Some(None));
}

#[test]
fn remap_tab_rename_state_after_close_drops_closed_tab_and_preserves_cancelled_id() {
    assert_eq!(
        remap_tab_rename_state_after_close(
            Some(TabRenameStateSnapshot {
                tab_id: 30,
                tab_index: 2,
            }),
            Some(40),
            2,
        ),
        (None, Some(40))
    );
    assert_eq!(
        remap_tab_rename_state_after_close(
            Some(TabRenameStateSnapshot {
                tab_id: 50,
                tab_index: 4,
            }),
            Some(50),
            1,
        ),
        (
            Some(TabRenameStateSnapshot {
                tab_id: 50,
                tab_index: 3,
            }),
            Some(50)
        )
    );
}

#[test]
fn remap_tab_rename_state_after_reorder_tracks_tab_identity() {
    let old_order = vec![10_u64, 20, 30, 40];
    let new_order = vec![30_u64, 10, 20, 40];
    assert_eq!(
        remap_tab_rename_state_after_reorder(
            Some(TabRenameStateSnapshot {
                tab_id: 30,
                tab_index: 2,
            }),
            Some(30),
            &old_order,
            &new_order,
        ),
        (
            Some(TabRenameStateSnapshot {
                tab_id: 30,
                tab_index: 0,
            }),
            Some(30)
        )
    );
}

#[test]
fn tab_rename_initial_label_prefers_rendered_presentation_over_raw_title() {
    assert_eq!(
        tab_rename_initial_label(
            None,
            None,
            None,
            Some("prod-1.example.com"),
            Some("ssh prod-1.example.com"),
            Some("/Users/sundy/src/con-terminal"),
            0,
            false,
        ),
        "prod-1.example.com"
    );

    assert_eq!(
        tab_rename_initial_label(
            None,
            Some("Deploy"),
            Some("phosphor/rocket.svg"),
            None,
            Some("zsh"),
            Some("/Users/sundy/src/con-terminal"),
            0,
            false,
        ),
        "Deploy"
    );
}

#[test]
fn split_preview_regions_vertical_before_splits_top_and_bottom() {
    let bounds = Bounds {
        origin: Point {
            x: px(10.0),
            y: px(20.0),
        },
        size: Size {
            width: px(300.0),
            height: px(200.0),
        },
    };

    let regions = split_preview_regions(bounds, SplitDirection::Vertical, SplitPlacement::Before);

    assert_eq!(
        regions.incoming.origin,
        Point {
            x: px(10.0),
            y: px(20.0)
        }
    );
    assert_eq!(
        regions.incoming.size,
        Size {
            width: px(300.0),
            height: px(100.0)
        }
    );
    assert_eq!(
        regions.existing.origin,
        Point {
            x: px(10.0),
            y: px(120.0)
        }
    );
    assert_eq!(
        regions.existing.size,
        Size {
            width: px(300.0),
            height: px(100.0)
        }
    );
    assert_eq!(regions.seam.size.width, px(300.0));
    assert_eq!(regions.seam.size.height, px(SPLIT_PREVIEW_SEAM_THICKNESS));
}

#[test]
fn split_preview_regions_horizontal_after_splits_left_and_right() {
    let bounds = Bounds {
        origin: Point {
            x: px(10.0),
            y: px(20.0),
        },
        size: Size {
            width: px(300.0),
            height: px(200.0),
        },
    };

    let regions = split_preview_regions(bounds, SplitDirection::Horizontal, SplitPlacement::After);

    assert_eq!(
        regions.existing.origin,
        Point {
            x: px(10.0),
            y: px(20.0)
        }
    );
    assert_eq!(
        regions.existing.size,
        Size {
            width: px(150.0),
            height: px(200.0)
        }
    );
    assert_eq!(
        regions.incoming.origin,
        Point {
            x: px(160.0),
            y: px(20.0)
        }
    );
    assert_eq!(
        regions.incoming.size,
        Size {
            width: px(150.0),
            height: px(200.0)
        }
    );
    assert_eq!(regions.seam.size.width, px(SPLIT_PREVIEW_SEAM_THICKNESS));
    assert_eq!(regions.seam.size.height, px(200.0));
}

#[test]
fn split_preview_local_rect_maps_absolute_bounds_to_content_space() {
    let content_bounds = Bounds {
        origin: Point {
            x: px(100.0),
            y: px(50.0),
        },
        size: Size {
            width: px(600.0),
            height: px(400.0),
        },
    };
    let absolute = Bounds {
        origin: Point {
            x: px(250.0),
            y: px(140.0),
        },
        size: Size {
            width: px(300.0),
            height: px(120.0),
        },
    };

    assert_eq!(
        split_preview_local_rect(absolute, content_bounds),
        (150.0, 90.0, 300.0, 120.0)
    );
}

#[test]
fn pane_split_drop_target_uses_whole_pane_quadrants() {
    let panes = [(
        1,
        Bounds {
            origin: Point {
                x: px(100.0),
                y: px(200.0),
            },
            size: Size {
                width: px(400.0),
                height: px(300.0),
            },
        },
    )];

    let top = pane_split_drop_target_from_position(
        0,
        Point {
            x: px(300.0),
            y: px(320.0),
        },
        &panes,
    )
    .expect("top interior target");
    assert_eq!(top.target_pane_id, 1);
    assert_eq!(top.direction, SplitDirection::Vertical);
    assert_eq!(top.placement, SplitPlacement::Before);

    let bottom = pane_split_drop_target_from_position(
        0,
        Point {
            x: px(300.0),
            y: px(380.0),
        },
        &panes,
    )
    .expect("bottom interior target");
    assert_eq!(bottom.direction, SplitDirection::Vertical);
    assert_eq!(bottom.placement, SplitPlacement::After);

    let left = pane_split_drop_target_from_position(
        0,
        Point {
            x: px(260.0),
            y: px(350.0),
        },
        &panes,
    )
    .expect("left interior target");
    assert_eq!(left.direction, SplitDirection::Horizontal);
    assert_eq!(left.placement, SplitPlacement::Before);

    let right = pane_split_drop_target_from_position(
        0,
        Point {
            x: px(340.0),
            y: px(350.0),
        },
        &panes,
    )
    .expect("right interior target");
    assert_eq!(right.direction, SplitDirection::Horizontal);
    assert_eq!(right.placement, SplitPlacement::After);
}

#[test]
fn pane_split_drop_target_ignores_source_pane() {
    let panes = [(
        1,
        Bounds {
            origin: Point {
                x: px(100.0),
                y: px(200.0),
            },
            size: Size {
                width: px(400.0),
                height: px(300.0),
            },
        },
    )];

    assert!(
        pane_split_drop_target_from_position(
            1,
            Point {
                x: px(120.0),
                y: px(350.0)
            },
            &panes,
        )
        .is_none()
    );
}

#[test]
fn trailing_drop_slot_uses_real_bounds() {
    let bounds = Bounds {
        origin: Point {
            x: px(340.0),
            y: px(20.0),
        },
        size: Size {
            width: px(88.0),
            height: px(30.0),
        },
    };

    assert_eq!(
        trailing_drop_slot_from_position(
            Point {
                x: px(400.0),
                y: px(25.0)
            },
            bounds,
            5,
        ),
        Some(5)
    );
    assert_eq!(
        trailing_drop_slot_from_position(
            Point {
                x: px(429.0),
                y: px(25.0)
            },
            bounds,
            5,
        ),
        None
    );
}

#[test]
fn horizontal_tab_slot_ignores_cursor_outside_bounds() {
    let bounds = Bounds {
        origin: Point {
            x: px(100.0),
            y: px(20.0),
        },
        size: Size {
            width: px(80.0),
            height: px(30.0),
        },
    };
    assert_eq!(
        horizontal_tab_slot_from_position(
            Point {
                x: px(90.0),
                y: px(25.0)
            },
            bounds,
            3
        ),
        None
    );
    assert_eq!(
        horizontal_tab_slot_from_position(
            Point {
                x: px(181.0),
                y: px(25.0)
            },
            bounds,
            3
        ),
        None
    );
    assert_eq!(
        horizontal_tab_slot_from_position(
            Point {
                x: px(120.0),
                y: px(10.0)
            },
            bounds,
            3
        ),
        None
    );
    assert_eq!(
        horizontal_tab_slot_from_position(
            Point {
                x: px(120.0),
                y: px(51.0)
            },
            bounds,
            3
        ),
        None
    );
}

#[test]
fn horizontal_tab_slot_uses_left_and_right_halves() {
    let bounds = Bounds {
        origin: Point {
            x: px(100.0),
            y: px(20.0),
        },
        size: Size {
            width: px(80.0),
            height: px(30.0),
        },
    };
    assert_eq!(
        horizontal_tab_slot_from_position(
            Point {
                x: px(110.0),
                y: px(25.0)
            },
            bounds,
            3
        ),
        Some(3)
    );
    assert_eq!(
        horizontal_tab_slot_from_position(
            Point {
                x: px(150.0),
                y: px(25.0)
            },
            bounds,
            3
        ),
        Some(4)
    );
}

#[test]
fn horizontal_tab_drag_waits_until_crossing_neighbor_midpoint() {
    let bounds = Bounds {
        origin: Point {
            x: px(200.0),
            y: px(20.0),
        },
        size: Size {
            width: px(100.0),
            height: px(30.0),
        },
    };

    // Chrome-style: swap fires as soon as cursor enters the neighbour tab,
    // regardless of which half of the tab the cursor is in.
    assert_eq!(
        horizontal_tab_slot_from_drag(
            Point {
                x: px(220.0),
                y: px(25.0)
            },
            bounds,
            2,
            Some(1)
        ),
        Some(3),
        "dragging tab 1 right into tab 2 should swap immediately"
    );
    assert_eq!(
        horizontal_tab_slot_from_drag(
            Point {
                x: px(280.0),
                y: px(25.0)
            },
            bounds,
            2,
            Some(1)
        ),
        Some(3),
        "dragging tab 1 right over tab 2 right half should also swap"
    );

    let left_neighbor = Bounds {
        origin: Point {
            x: px(100.0),
            y: px(20.0),
        },
        size: Size {
            width: px(100.0),
            height: px(30.0),
        },
    };
    assert_eq!(
        horizontal_tab_slot_from_drag(
            Point {
                x: px(180.0),
                y: px(25.0)
            },
            left_neighbor,
            1,
            Some(2)
        ),
        Some(1),
        "dragging tab 2 left into tab 1 should swap immediately"
    );
    assert_eq!(
        horizontal_tab_slot_from_drag(
            Point {
                x: px(120.0),
                y: px(25.0)
            },
            left_neighbor,
            1,
            Some(2)
        ),
        Some(1),
        "dragging tab 2 left over tab 1 left half should also swap"
    );

    // Cursor on the source tab itself — no swap.
    let source_bounds = Bounds {
        origin: Point {
            x: px(300.0),
            y: px(20.0),
        },
        size: Size {
            width: px(100.0),
            height: px(30.0),
        },
    };
    assert_eq!(
        horizontal_tab_slot_from_drag(
            Point {
                x: px(350.0),
                y: px(25.0)
            },
            source_bounds,
            3,
            Some(3)
        ),
        None,
        "cursor on source tab should not trigger swap"
    );
}

#[test]
fn horizontal_tab_drag_without_source_still_uses_half_slots() {
    let bounds = Bounds {
        origin: Point {
            x: px(100.0),
            y: px(20.0),
        },
        size: Size {
            width: px(80.0),
            height: px(30.0),
        },
    };

    assert_eq!(
        horizontal_tab_slot_from_drag(
            Point {
                x: px(110.0),
                y: px(25.0)
            },
            bounds,
            3,
            None
        ),
        Some(3)
    );
    assert_eq!(
        horizontal_tab_slot_from_drag(
            Point {
                x: px(150.0),
                y: px(25.0)
            },
            bounds,
            3,
            None
        ),
        Some(4)
    );
}

#[test]
fn pane_title_drag_tab_slot_uses_midpoint_of_each_tab() {
    let tab_bounds = vec![
        gpui::Bounds {
            origin: gpui::Point {
                x: px(0.0),
                y: px(0.0),
            },
            size: gpui::Size {
                width: px(100.0),
                height: px(30.0),
            },
        },
        gpui::Bounds {
            origin: gpui::Point {
                x: px(100.0),
                y: px(0.0),
            },
            size: gpui::Size {
                width: px(100.0),
                height: px(30.0),
            },
        },
        gpui::Bounds {
            origin: gpui::Point {
                x: px(200.0),
                y: px(0.0),
            },
            size: gpui::Size {
                width: px(100.0),
                height: px(30.0),
            },
        },
    ];

    assert_eq!(
        pane_title_drag_tab_slot(
            Point {
                x: px(30.0),
                y: px(15.0)
            },
            &tab_bounds,
            3
        ),
        0
    );
    assert_eq!(
        pane_title_drag_tab_slot(
            Point {
                x: px(80.0),
                y: px(15.0)
            },
            &tab_bounds,
            3
        ),
        1
    );
    assert_eq!(
        pane_title_drag_tab_slot(
            Point {
                x: px(160.0),
                y: px(15.0)
            },
            &tab_bounds,
            3
        ),
        2
    );
    assert_eq!(
        pane_title_drag_tab_slot(
            Point {
                x: px(260.0),
                y: px(15.0)
            },
            &tab_bounds,
            3
        ),
        3
    );
}

#[test]
fn remap_drop_slot_preserves_current_order_for_live_drag_preview() {
    assert_eq!(remap_drop_slot_for_current_order(1, 2), 3);
    assert_eq!(remap_drop_slot_for_current_order(2, 1), 1);
}

#[test]
fn top_bar_clickables_explicitly_consume_left_mouse_down() {
    let source = concat!(
        include_str!("render.rs"),
        "\n",
        include_str!("render/top_bar.rs")
    );
    for control_id in [
        "tab-new",
        "toggle-left-sidebar",
        "toggle-input-bar",
        "toggle-agent-panel",
        "toggle-settings",
    ] {
        let marker = format!(".id(\"{control_id}\")");
        let start = source
            .find(&marker)
            .unwrap_or_else(|| panic!("missing top bar control {control_id}"));
        let snippet = &source[start..source.len().min(start + 1200)];
        let normalized = snippet.split_whitespace().collect::<String>();
        let has_mouse_down_handler = normalized
            .contains(".on_mouse_down(MouseButton::Left,|_,_,cx|{")
            || normalized.contains(".on_mouse_down(MouseButton::Left,cx.listener(");
        let consumes_mouse_down =
            has_mouse_down_handler && normalized.contains("cx.stop_propagation();");
        assert!(
            consumes_mouse_down,
            "top bar control {control_id} must consume left mouse-down before parent drag"
        );
    }
}

#[test]
fn surface_key_bytes_preserves_literal_character_case() {
    assert_eq!(ConWorkspace::surface_key_bytes("A").unwrap(), b"A");
    assert_eq!(ConWorkspace::surface_key_bytes("z").unwrap(), b"z");
}

#[test]
fn surface_key_bytes_matches_named_keys_case_insensitively() {
    assert_eq!(ConWorkspace::surface_key_bytes("ENTER").unwrap(), b"\n");
    assert_eq!(
        ConWorkspace::surface_key_bytes("Ctrl-C").unwrap(),
        vec![0x03]
    );
    assert_eq!(
        ConWorkspace::surface_key_bytes("Ctrl-]").unwrap(),
        vec![0x1d]
    );
    assert_eq!(
        ConWorkspace::surface_key_bytes("control-\\").unwrap(),
        vec![0x1c]
    );
    assert_eq!(ConWorkspace::surface_key_bytes("C-/").unwrap(), vec![0x1f]);
    assert_eq!(
        ConWorkspace::surface_key_bytes("ctrl-2").unwrap(),
        vec![0x00]
    );
    assert_eq!(ConWorkspace::surface_key_bytes("C-?").unwrap(), vec![0x7f]);
}

#[test]
fn new_tab_requires_immediate_ui_sync() {
    let policy = ConWorkspace::new_tab_sync_policy_for_tests();
    assert!(policy.activates_new_tab);
    assert!(policy.syncs_sidebar);
    assert!(policy.notifies_ui);
    assert!(policy.syncs_native_visibility);
    assert!(policy.reuses_shared_tab_activation_flow);
}

#[test]
fn new_tab_reveals_hidden_vertical_tab_rail_only() {
    assert!(should_reveal_vertical_tab_rail_for_new_tab(true, false));
    assert!(!should_reveal_vertical_tab_rail_for_new_tab(true, true));
    assert!(!should_reveal_vertical_tab_rail_for_new_tab(false, false));
    assert!(!should_reveal_vertical_tab_rail_for_new_tab(false, true));
}

#[test]
fn promoting_single_tab_to_tab_strip_requires_deferred_top_chrome_refresh() {
    assert!(ConWorkspace::should_defer_top_chrome_refresh_when_tab_strip_appears_for_tests());
}
