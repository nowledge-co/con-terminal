#include <stddef.h>

#include <ghostty.h>

/*
 * Con handwrites the internal macOS Ghostty ABI in ffi.rs. Most action-tag
 * changes do not alter the enclosing union size, so Rust layout assertions
 * alone cannot detect enum drift. Keep every tag value explicit here so a
 * Ghostty revision bump fails while compiling instead of misdispatching an
 * unrelated payload at runtime.
 */
#define ASSERT_ACTION(name, value) \
    _Static_assert((name) == (value), #name " changed value")

ASSERT_ACTION(GHOSTTY_ACTION_QUIT, 0);
ASSERT_ACTION(GHOSTTY_ACTION_NEW_WINDOW, 1);
ASSERT_ACTION(GHOSTTY_ACTION_NEW_TAB, 2);
ASSERT_ACTION(GHOSTTY_ACTION_CLOSE_TAB, 3);
ASSERT_ACTION(GHOSTTY_ACTION_NEW_SPLIT, 4);
ASSERT_ACTION(GHOSTTY_ACTION_CLOSE_ALL_WINDOWS, 5);
ASSERT_ACTION(GHOSTTY_ACTION_TOGGLE_MAXIMIZE, 6);
ASSERT_ACTION(GHOSTTY_ACTION_TOGGLE_FULLSCREEN, 7);
ASSERT_ACTION(GHOSTTY_ACTION_TOGGLE_TAB_OVERVIEW, 8);
ASSERT_ACTION(GHOSTTY_ACTION_TOGGLE_WINDOW_DECORATIONS, 9);
ASSERT_ACTION(GHOSTTY_ACTION_TOGGLE_QUICK_TERMINAL, 10);
ASSERT_ACTION(GHOSTTY_ACTION_TOGGLE_COMMAND_PALETTE, 11);
ASSERT_ACTION(GHOSTTY_ACTION_TOGGLE_VISIBILITY, 12);
ASSERT_ACTION(GHOSTTY_ACTION_TOGGLE_BACKGROUND_OPACITY, 13);
ASSERT_ACTION(GHOSTTY_ACTION_MOVE_TAB, 14);
ASSERT_ACTION(GHOSTTY_ACTION_GOTO_TAB, 15);
ASSERT_ACTION(GHOSTTY_ACTION_GOTO_SPLIT, 16);
ASSERT_ACTION(GHOSTTY_ACTION_GOTO_WINDOW, 17);
ASSERT_ACTION(GHOSTTY_ACTION_RESIZE_SPLIT, 18);
ASSERT_ACTION(GHOSTTY_ACTION_EQUALIZE_SPLITS, 19);
ASSERT_ACTION(GHOSTTY_ACTION_TOGGLE_SPLIT_ZOOM, 20);
ASSERT_ACTION(GHOSTTY_ACTION_PRESENT_TERMINAL, 21);
ASSERT_ACTION(GHOSTTY_ACTION_SIZE_LIMIT, 22);
ASSERT_ACTION(GHOSTTY_ACTION_RESET_WINDOW_SIZE, 23);
ASSERT_ACTION(GHOSTTY_ACTION_INITIAL_SIZE, 24);
ASSERT_ACTION(GHOSTTY_ACTION_CELL_SIZE, 25);
ASSERT_ACTION(GHOSTTY_ACTION_SCROLLBAR, 26);
ASSERT_ACTION(GHOSTTY_ACTION_RENDER, 27);
ASSERT_ACTION(GHOSTTY_ACTION_INSPECTOR, 28);
ASSERT_ACTION(GHOSTTY_ACTION_SHOW_GTK_INSPECTOR, 29);
ASSERT_ACTION(GHOSTTY_ACTION_RENDER_INSPECTOR, 30);
ASSERT_ACTION(GHOSTTY_ACTION_EXPORT_TERMINAL_IO, 31);
ASSERT_ACTION(GHOSTTY_ACTION_DESKTOP_NOTIFICATION, 32);
ASSERT_ACTION(GHOSTTY_ACTION_SET_TITLE, 33);
ASSERT_ACTION(GHOSTTY_ACTION_SET_TAB_TITLE, 34);
ASSERT_ACTION(GHOSTTY_ACTION_SET_WINDOW_TITLE, 35);
ASSERT_ACTION(GHOSTTY_ACTION_PROMPT_TITLE, 36);
ASSERT_ACTION(GHOSTTY_ACTION_PWD, 37);
ASSERT_ACTION(GHOSTTY_ACTION_MOUSE_SHAPE, 38);
ASSERT_ACTION(GHOSTTY_ACTION_MOUSE_VISIBILITY, 39);
ASSERT_ACTION(GHOSTTY_ACTION_MOUSE_OVER_LINK, 40);
ASSERT_ACTION(GHOSTTY_ACTION_RENDERER_HEALTH, 41);
ASSERT_ACTION(GHOSTTY_ACTION_OPEN_CONFIG, 42);
ASSERT_ACTION(GHOSTTY_ACTION_QUIT_TIMER, 43);
ASSERT_ACTION(GHOSTTY_ACTION_FLOAT_WINDOW, 44);
ASSERT_ACTION(GHOSTTY_ACTION_SECURE_INPUT, 45);
ASSERT_ACTION(GHOSTTY_ACTION_KEY_SEQUENCE, 46);
ASSERT_ACTION(GHOSTTY_ACTION_KEY_TABLE, 47);
ASSERT_ACTION(GHOSTTY_ACTION_COLOR_CHANGE, 48);
ASSERT_ACTION(GHOSTTY_ACTION_RELOAD_CONFIG, 49);
ASSERT_ACTION(GHOSTTY_ACTION_CONFIG_CHANGE, 50);
ASSERT_ACTION(GHOSTTY_ACTION_CLOSE_WINDOW, 51);
ASSERT_ACTION(GHOSTTY_ACTION_RING_BELL, 52);
ASSERT_ACTION(GHOSTTY_ACTION_SELECTION_CHANGED, 53);
ASSERT_ACTION(GHOSTTY_ACTION_UNDO, 54);
ASSERT_ACTION(GHOSTTY_ACTION_REDO, 55);
ASSERT_ACTION(GHOSTTY_ACTION_CHECK_FOR_UPDATES, 56);
ASSERT_ACTION(GHOSTTY_ACTION_OPEN_URL, 57);
ASSERT_ACTION(GHOSTTY_ACTION_SHOW_CHILD_EXITED, 58);
ASSERT_ACTION(GHOSTTY_ACTION_PROGRESS_REPORT, 59);
ASSERT_ACTION(GHOSTTY_ACTION_SHOW_ON_SCREEN_KEYBOARD, 60);
ASSERT_ACTION(GHOSTTY_ACTION_COMMAND_FINISHED, 61);
ASSERT_ACTION(GHOSTTY_ACTION_START_SEARCH, 62);
ASSERT_ACTION(GHOSTTY_ACTION_END_SEARCH, 63);
ASSERT_ACTION(GHOSTTY_ACTION_SEARCH_TOTAL, 64);
ASSERT_ACTION(GHOSTTY_ACTION_SEARCH_SELECTED, 65);
ASSERT_ACTION(GHOSTTY_ACTION_READONLY, 66);
ASSERT_ACTION(GHOSTTY_ACTION_COPY_TITLE_TO_CLIPBOARD, 67);
ASSERT_ACTION(GHOSTTY_ACTION_MOVE_TAB_TO_NEW_WINDOW, 68);

#undef ASSERT_ACTION

_Static_assert(sizeof(ghostty_action_u) == 24, "ghostty_action_u changed layout");
_Static_assert(sizeof(ghostty_action_s) == 32, "ghostty_action_s changed layout");
_Static_assert(sizeof(ghostty_surface_message_childexited_s) == 16,
               "ghostty child-exited payload changed layout");
_Static_assert(offsetof(ghostty_surface_message_childexited_s, timetime_ms) == 8,
               "ghostty child-exited runtime changed offset");
_Static_assert(sizeof(ghostty_action_command_finished_s) == 16,
               "ghostty command-finished payload changed layout");
_Static_assert(offsetof(ghostty_action_command_finished_s, duration) == 8,
               "ghostty command-finished duration changed offset");
_Static_assert(sizeof(ghostty_action_start_search_s) == sizeof(void *),
               "ghostty start-search payload changed layout");
_Static_assert(sizeof(ghostty_action_search_total_s) == sizeof(ssize_t),
               "ghostty search-total payload changed layout");
_Static_assert(sizeof(ghostty_action_search_selected_s) == sizeof(ssize_t),
               "ghostty search-selected payload changed layout");

_Static_assert(GHOSTTY_CLIPBOARD_STANDARD == 0,
               "ghostty standard clipboard changed value");
_Static_assert(GHOSTTY_CLIPBOARD_SELECTION == 1,
               "ghostty selection clipboard changed value");
_Static_assert(GHOSTTY_CLIPBOARD_PRIMARY == 2,
               "ghostty primary clipboard changed value");
_Static_assert(GHOSTTY_CLIPBOARD_REQUEST_PASTE == 0,
               "ghostty paste request changed value");
_Static_assert(GHOSTTY_CLIPBOARD_REQUEST_OSC_52_READ == 1,
               "ghostty OSC 52 read request changed value");
_Static_assert(GHOSTTY_CLIPBOARD_REQUEST_OSC_52_WRITE == 2,
               "ghostty OSC 52 write request changed value");
_Static_assert(GHOSTTY_CLIPBOARD_REQUEST_KITTY_READ == 3,
               "ghostty Kitty read request changed value");
_Static_assert(GHOSTTY_CLIPBOARD_REQUEST_KITTY_WRITE == 4,
               "ghostty Kitty write request changed value");
_Static_assert(GHOSTTY_CLIPBOARD_REQUEST_LIST == 5,
               "ghostty clipboard list request changed value");
_Static_assert(GHOSTTY_CLIPBOARD_READ_STARTED == 0,
               "ghostty started result changed value");
_Static_assert(GHOSTTY_CLIPBOARD_READ_UNAVAILABLE == 1,
               "ghostty unavailable result changed value");
_Static_assert(GHOSTTY_CLIPBOARD_READ_UNSUPPORTED == 2,
               "ghostty unsupported result changed value");

_Static_assert(sizeof(ghostty_clipboard_content_s) == 24,
               "ghostty clipboard content changed layout");
_Static_assert(offsetof(ghostty_clipboard_content_s, len) == 16,
               "ghostty clipboard content length changed offset");
_Static_assert(sizeof(ghostty_clipboard_complete_s) == 40,
               "ghostty clipboard completion changed layout");
_Static_assert(offsetof(ghostty_clipboard_complete_s, available) == 16,
               "ghostty clipboard completion listing changed offset");
_Static_assert(offsetof(ghostty_clipboard_complete_s, confirmed) == 32,
               "ghostty clipboard completion confirmation changed offset");
_Static_assert(sizeof(ghostty_clipboard_confirm_s) == 48,
               "ghostty clipboard confirmation changed layout");
_Static_assert(offsetof(ghostty_clipboard_confirm_s, name) == 32,
               "ghostty clipboard confirmation name changed offset");
_Static_assert(sizeof(ghostty_runtime_config_s) == 64,
               "ghostty runtime config changed layout");
_Static_assert(offsetof(ghostty_runtime_config_s, read_clipboard_cb) == 32,
               "ghostty runtime clipboard callback changed offset");
_Static_assert(offsetof(ghostty_runtime_config_s, close_surface_cb) == 56,
               "ghostty runtime close callback changed offset");

#ifdef CON_GHOSTTY_EMBEDDED_INITIAL_OUTPUT
_Static_assert(sizeof(ghostty_surface_config_s) == 96,
               "patched ghostty surface config changed layout");
_Static_assert(offsetof(ghostty_surface_config_s, wait_after_command) == 88,
               "patched initial_output changed position");
#else
_Static_assert(sizeof(ghostty_surface_config_s) == 88,
               "ghostty surface config changed layout");
_Static_assert(offsetof(ghostty_surface_config_s, wait_after_command) == 80,
               "ghostty surface config fields changed position");
#endif
