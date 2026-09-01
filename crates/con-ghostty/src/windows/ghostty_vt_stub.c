/*
 * Stub C implementations of the libghostty-vt symbols con-ghostty's
 * `src/windows/vt.rs` binds. Used when `CON_STUB_GHOSTTY_VT=1` so a
 * cargo build can link without a working Zig/libghostty-vt toolchain.
 *
 * Signatures mirror `include/ghostty/vt/{terminal,render,allocator}.h`
 * at GHOSTTY_REV `5f5b988c5236facfe8d2439203d9ee9d5b636cf8` — keep in
 * sync with vt.rs on upstream bumps.
 *
 * All calls return empty / false / zero so downstream code degrades
 * gracefully to "empty terminal grid, clear-color render". The rest
 * of the Windows backend (GPUI host view, ConPTY spawn, D3D11
 * swapchain, atlas setup) still exercises fully.
 */

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef void* GhosttyTerminal;
typedef void* GhosttyRenderState;
typedef void* GhosttyRowIterator;
typedef void* GhosttyRowCells;
typedef void* GhosttyKeyEncoder;
typedef void* GhosttyKeyEvent;
typedef void* GhosttySelectionGesture;
typedef void* GhosttySelectionGestureEvent;
typedef void* GhosttyKittyGraphics;
typedef void* GhosttyKittyGraphicsImage;
typedef void* GhosttyKittyGraphicsPlacementIterator;
typedef int   GhosttyResult;

struct GhosttyPointCoordinate {
    uint16_t x;
    uint32_t y;
};

union GhosttyPointValue {
    struct GhosttyPointCoordinate coordinate;
    uint64_t _padding[2];
};

struct GhosttyPoint {
    int tag;
    union GhosttyPointValue value;
};

struct GhosttyGridRef {
    size_t size;
    void* node;
    uint16_t x;
    uint16_t y;
};

struct GhosttySelection {
    size_t size;
    struct GhosttyGridRef start;
    struct GhosttyGridRef end;
    bool rectangle;
};

struct GhosttyTerminalSelectionFormatOptions {
    size_t size;
    int emit;
    bool unwrap;
    bool trim;
    const struct GhosttySelection* selection;
};

union GhosttyTerminalScrollViewportValue {
    intptr_t delta;
    uint64_t _padding[2];
};

struct GhosttyTerminalScrollViewport {
    int tag;
    union GhosttyTerminalScrollViewportValue value;
};

/* The stub has no real ABI manifest. Returning NULL keeps ordinary stub
 * builds linkable while making manifest-aware tests fail explicitly rather
 * than validating against invented metadata. */
const char* ghostty_type_json(void) { return NULL; }

/* ── Allocator and process-global services ─────────────────────── */

uint8_t* ghostty_alloc(const void* allocator, size_t len) {
    (void)allocator; (void)len;
    return NULL;
}

void ghostty_free(const void* allocator, uint8_t* ptr, size_t len) {
    (void)allocator; (void)ptr; (void)len;
}

GhosttyResult ghostty_sys_set(int option, const void* value) {
    (void)option; (void)value;
    return 0;
}

/* ── Terminal lifecycle ─────────────────────────────────────────── */

GhosttyResult ghostty_terminal_new(
    const void* allocator,
    GhosttyTerminal* out_terminal,
    uint16_t cols,
    uint16_t rows
) {
    (void)allocator; (void)cols; (void)rows;
    if (out_terminal) { *out_terminal = (void*)(uintptr_t)1; }
    return 0;
}

void ghostty_terminal_free(GhosttyTerminal terminal) { (void)terminal; }

GhosttyResult ghostty_terminal_resize(
    GhosttyTerminal terminal, uint16_t cols, uint16_t rows,
    uint32_t cell_width_px, uint32_t cell_height_px
) {
    (void)terminal; (void)cols; (void)rows;
    (void)cell_width_px; (void)cell_height_px;
    return 0;
}

void ghostty_terminal_vt_write(GhosttyTerminal terminal, const uint8_t* data, size_t len) {
    (void)terminal; (void)data; (void)len;
}

void ghostty_terminal_scroll_viewport(
    GhosttyTerminal terminal,
    struct GhosttyTerminalScrollViewport behavior
) {
    (void)terminal; (void)behavior;
}

GhosttyResult ghostty_terminal_set(
    GhosttyTerminal terminal, int option, const void* value
) {
    (void)terminal; (void)option; (void)value;
    return 0;
}

GhosttyResult ghostty_terminal_get(
    GhosttyTerminal terminal, int key, void* out
) {
    (void)terminal; (void)key; (void)out;
    return 1;
}

GhosttyResult ghostty_terminal_paste(
    GhosttyTerminal terminal, const void* paste, bool* out_written
) {
    (void)terminal; (void)paste;
    if (out_written) { *out_written = false; }
    return 0;
}

GhosttyResult ghostty_terminal_grid_ref(
    GhosttyTerminal terminal, struct GhosttyPoint point,
    struct GhosttyGridRef* out_ref
) {
    (void)terminal; (void)point; (void)out_ref;
    return -4;
}

/* ── Selection ─────────────────────────────────────────────────── */

GhosttyResult ghostty_selection_gesture_new(
    const void* allocator, GhosttySelectionGesture* out_gesture
) {
    (void)allocator;
    if (out_gesture) { *out_gesture = (void*)(uintptr_t)8; }
    return 0;
}

void ghostty_selection_gesture_free(
    GhosttySelectionGesture gesture, GhosttyTerminal terminal
) {
    (void)gesture; (void)terminal;
}

void ghostty_selection_gesture_reset(
    GhosttySelectionGesture gesture, GhosttyTerminal terminal
) {
    (void)gesture; (void)terminal;
}

GhosttyResult ghostty_selection_gesture_get(
    GhosttySelectionGesture gesture, GhosttyTerminal terminal,
    int data, void* out
) {
    (void)gesture; (void)terminal;
    if (out) {
        if (data == 0) { *(uint8_t*)out = 0; }
        else { *(int*)out = 0; }
    }
    return 0;
}

GhosttyResult ghostty_selection_gesture_event_new(
    const void* allocator, GhosttySelectionGestureEvent* out_event,
    int event_type
) {
    (void)allocator; (void)event_type;
    if (out_event) { *out_event = (void*)(uintptr_t)9; }
    return 0;
}

void ghostty_selection_gesture_event_free(GhosttySelectionGestureEvent event) {
    (void)event;
}

GhosttyResult ghostty_selection_gesture_event_set(
    GhosttySelectionGestureEvent event, int option, const void* value
) {
    (void)event; (void)option; (void)value;
    return 0;
}

GhosttyResult ghostty_selection_gesture_event(
    GhosttySelectionGesture gesture, GhosttyTerminal terminal,
    GhosttySelectionGestureEvent event, struct GhosttySelection* out_selection
) {
    (void)gesture; (void)terminal; (void)event; (void)out_selection;
    return -4;
}

GhosttyResult ghostty_terminal_selection_equal(
    GhosttyTerminal terminal, const struct GhosttySelection* a,
    const struct GhosttySelection* b, bool* out_equal
) {
    (void)terminal; (void)a; (void)b;
    if (out_equal) { *out_equal = true; }
    return 0;
}

GhosttyResult ghostty_terminal_selection_format_alloc(
    GhosttyTerminal terminal, const void* allocator,
    struct GhosttyTerminalSelectionFormatOptions options,
    uint8_t** out_ptr, size_t* out_len
) {
    (void)terminal; (void)allocator; (void)options;
    if (out_ptr) { *out_ptr = NULL; }
    if (out_len) { *out_len = 0; }
    return -4;
}

/* ── Kitty graphics ────────────────────────────────────────────── */

GhosttyResult ghostty_kitty_graphics_get(
    GhosttyKittyGraphics graphics, int data, void* out
) {
    (void)graphics; (void)data; (void)out;
    return 1;
}

GhosttyKittyGraphicsImage ghostty_kitty_graphics_image(
    GhosttyKittyGraphics graphics, uint32_t image_id
) {
    (void)graphics; (void)image_id;
    return NULL;
}

GhosttyResult ghostty_kitty_graphics_image_get(
    GhosttyKittyGraphicsImage image, int data, void* out
) {
    (void)image; (void)data; (void)out;
    return 1;
}

GhosttyResult ghostty_kitty_graphics_image_get_multi(
    GhosttyKittyGraphicsImage image, size_t count,
    const int* keys, void** values, size_t* out_written
) {
    (void)image; (void)count; (void)keys; (void)values; (void)out_written;
    return 1;
}

GhosttyResult ghostty_kitty_graphics_placement_iterator_new(
    const void* allocator, GhosttyKittyGraphicsPlacementIterator* out_iterator
) {
    (void)allocator;
    if (out_iterator) { *out_iterator = (void*)(uintptr_t)7; }
    return 0;
}

void ghostty_kitty_graphics_placement_iterator_free(
    GhosttyKittyGraphicsPlacementIterator iterator
) {
    (void)iterator;
}

bool ghostty_kitty_graphics_placement_next(
    GhosttyKittyGraphicsPlacementIterator iterator
) {
    (void)iterator;
    return false;
}

GhosttyResult ghostty_kitty_graphics_placement_get_multi(
    GhosttyKittyGraphicsPlacementIterator iterator, size_t count,
    const int* keys, void** values, size_t* out_written
) {
    (void)iterator; (void)count; (void)keys; (void)values; (void)out_written;
    return 1;
}

GhosttyResult ghostty_kitty_graphics_placement_render_info(
    GhosttyKittyGraphicsPlacementIterator iterator,
    GhosttyKittyGraphicsImage image, GhosttyTerminal terminal, void* out_info
) {
    (void)iterator; (void)image; (void)terminal; (void)out_info;
    return 1;
}

/* ── Key encoder ────────────────────────────────────────────────── */

GhosttyResult ghostty_key_encoder_new(
    const void* allocator, GhosttyKeyEncoder* out_encoder
) {
    (void)allocator;
    if (out_encoder) { *out_encoder = (void*)(uintptr_t)5; }
    return 0;
}

void ghostty_key_encoder_free(GhosttyKeyEncoder encoder) { (void)encoder; }

void ghostty_key_encoder_setopt_from_terminal(
    GhosttyKeyEncoder encoder, GhosttyTerminal terminal
) {
    (void)encoder; (void)terminal;
}

GhosttyResult ghostty_key_encoder_encode(
    GhosttyKeyEncoder encoder, GhosttyKeyEvent event,
    char* out_buf, size_t out_buf_size, size_t* out_len
) {
    (void)encoder; (void)event; (void)out_buf; (void)out_buf_size;
    if (out_len) { *out_len = 0; }
    return 0;
}

GhosttyResult ghostty_key_event_new(
    const void* allocator, GhosttyKeyEvent* out_event
) {
    (void)allocator;
    if (out_event) { *out_event = (void*)(uintptr_t)6; }
    return 0;
}

void ghostty_key_event_free(GhosttyKeyEvent event) { (void)event; }
void ghostty_key_event_set_action(GhosttyKeyEvent event, int action) {
    (void)event; (void)action;
}
void ghostty_key_event_set_key(GhosttyKeyEvent event, int key) {
    (void)event; (void)key;
}
void ghostty_key_event_set_mods(GhosttyKeyEvent event, uint16_t mods) {
    (void)event; (void)mods;
}
void ghostty_key_event_set_consumed_mods(GhosttyKeyEvent event, uint16_t mods) {
    (void)event; (void)mods;
}
void ghostty_key_event_set_composing(GhosttyKeyEvent event, bool composing) {
    (void)event; (void)composing;
}
void ghostty_key_event_set_utf8(
    GhosttyKeyEvent event, const char* utf8, size_t len
) {
    (void)event; (void)utf8; (void)len;
}
void ghostty_key_event_set_unshifted_codepoint(
    GhosttyKeyEvent event, uint32_t codepoint
) {
    (void)event; (void)codepoint;
}

/* ── Cell accessor ──────────────────────────────────────────────── */

GhosttyResult ghostty_cell_get(uint64_t cell, int key, void* out) {
    (void)cell; (void)key;
    if (out) { *(uint8_t*)out = 0; }
    return 0;
}

GhosttyResult ghostty_cell_get_multi(
    uint64_t cell, size_t count, const int* keys,
    void** values, size_t* out_written
) {
    if (!keys || !values) {
        if (out_written) { *out_written = 0; }
        return -2;
    }
    for (size_t i = 0; i < count; ++i) {
        if (!values[i]) {
            if (out_written) { *out_written = i; }
            return -2;
        }
        GhosttyResult result = ghostty_cell_get(cell, keys[i], values[i]);
        if (result != 0) {
            if (out_written) { *out_written = i; }
            return result;
        }
    }
    if (out_written) { *out_written = count; }
    return 0;
}

/* ── Render state ───────────────────────────────────────────────── */

GhosttyResult ghostty_render_state_new(
    const void* allocator, GhosttyRenderState* out_state
) {
    (void)allocator;
    if (out_state) { *out_state = (void*)(uintptr_t)2; }
    return 0;
}

void ghostty_render_state_free(GhosttyRenderState state) { (void)state; }

GhosttyResult ghostty_render_state_update(
    GhosttyRenderState state, GhosttyTerminal terminal
) {
    (void)state; (void)terminal;
    return 0;
}

GhosttyResult ghostty_render_state_begin_update(
    GhosttyRenderState state, GhosttyTerminal terminal
) {
    (void)state; (void)terminal;
    return 0;
}

GhosttyResult ghostty_render_state_end_update(GhosttyRenderState state) {
    (void)state;
    return 0;
}

GhosttyResult ghostty_render_state_clean(GhosttyRenderState state) {
    (void)state;
    return 0;
}

GhosttyResult ghostty_render_state_get(
    GhosttyRenderState state, int key, void* out
) {
    (void)state; (void)key;
    if (out) { *(uint8_t*)out = 0; }
    return 0;
}

GhosttyResult ghostty_render_state_get_multi(
    GhosttyRenderState state, size_t count, const int* keys,
    void** values, size_t* out_written
) {
    if (!keys || !values) {
        if (out_written) { *out_written = 0; }
        return -2;
    }
    for (size_t i = 0; i < count; ++i) {
        if (!values[i]) {
            if (out_written) { *out_written = i; }
            return -2;
        }
        GhosttyResult result = ghostty_render_state_get(state, keys[i], values[i]);
        if (result != 0) {
            if (out_written) { *out_written = i; }
            return result;
        }
    }
    if (out_written) { *out_written = count; }
    return 0;
}

GhosttyResult ghostty_render_state_row_iterator_new(
    const void* allocator, GhosttyRowIterator* out_iter
) {
    (void)allocator;
    if (out_iter) { *out_iter = (void*)(uintptr_t)3; }
    return 0;
}
void ghostty_render_state_row_iterator_free(GhosttyRowIterator iter) { (void)iter; }
bool ghostty_render_state_row_iterator_next(GhosttyRowIterator iter) { (void)iter; return false; }
bool ghostty_render_state_row_iterator_next_dirty(
    GhosttyRowIterator iter, uint16_t* out_y
) {
    (void)iter; (void)out_y;
    return false;
}

GhosttyResult ghostty_render_state_row_get(
    GhosttyRowIterator iter, int key, void* out
) {
    (void)iter; (void)key;
    if (out) { *(uint8_t*)out = 0; }
    return 0;
}

GhosttyResult ghostty_render_state_row_get_multi(
    GhosttyRowIterator iter, size_t count, const int* keys,
    void** values, size_t* out_written
) {
    if (!keys || !values) {
        if (out_written) { *out_written = 0; }
        return -2;
    }
    for (size_t i = 0; i < count; ++i) {
        if (!values[i]) {
            if (out_written) { *out_written = i; }
            return -2;
        }
        GhosttyResult result = ghostty_render_state_row_get(iter, keys[i], values[i]);
        if (result != 0) {
            if (out_written) { *out_written = i; }
            return result;
        }
    }
    if (out_written) { *out_written = count; }
    return 0;
}

GhosttyResult ghostty_render_state_row_cells_new(
    const void* allocator, GhosttyRowCells* out_cells
) {
    (void)allocator;
    if (out_cells) { *out_cells = (void*)(uintptr_t)4; }
    return 0;
}
void ghostty_render_state_row_cells_free(GhosttyRowCells cells) { (void)cells; }
bool ghostty_render_state_row_cells_next(GhosttyRowCells cells) { (void)cells; return false; }

GhosttyResult ghostty_render_state_row_cells_get(
    GhosttyRowCells cells, int key, void* out
) {
    (void)cells; (void)key;
    if (out) { *(uint8_t*)out = 0; }
    return 0;
}

GhosttyResult ghostty_render_state_row_cells_get_multi(
    GhosttyRowCells cells, size_t count, const int* keys,
    void** values, size_t* out_written
) {
    if (!keys || !values) {
        if (out_written) { *out_written = 0; }
        return -2;
    }
    for (size_t i = 0; i < count; ++i) {
        if (!values[i]) {
            if (out_written) { *out_written = i; }
            return -2;
        }
        GhosttyResult result = ghostty_render_state_row_cells_get(cells, keys[i], values[i]);
        if (result != 0) {
            if (out_written) { *out_written = i; }
            return result;
        }
    }
    if (out_written) { *out_written = count; }
    return 0;
}
