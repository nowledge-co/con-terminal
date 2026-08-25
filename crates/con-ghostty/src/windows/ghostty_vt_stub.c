/*
 * Stub C implementations of the libghostty-vt symbols con-ghostty's
 * `src/windows/vt.rs` binds. Used when `CON_STUB_GHOSTTY_VT=1` so a
 * cargo build can link without a working Zig/libghostty-vt toolchain.
 *
 * Signatures mirror `include/ghostty/vt/{terminal,render,allocator}.h`
 * at GHOSTTY_REV `8867c37c55b578b9eb4cfaba41cb9023e557176d` — keep in
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
typedef int   GhosttyResult;

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

GhosttyResult ghostty_render_state_get(
    GhosttyRenderState state, int key, void* out
) {
    (void)state; (void)key;
    if (out) { *(uint8_t*)out = 0; }
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

GhosttyResult ghostty_render_state_row_get(
    GhosttyRowIterator iter, int key, void* out
) {
    (void)iter; (void)key;
    if (out) { *(uint8_t*)out = 0; }
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
