//! Incremental native search. Lock order is search -> terminal; ticking only
//! holds the search lock, so scanning copied history does not block PTY writes.
use super::*;

#[repr(i32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Status {
    Running = 0,
    FeedRequired = 1,
    #[default]
    Complete = 2,
}

#[repr(i32)]
enum OptionKey {
    Needle = 0,
    Next = 1,
    Previous = 2,
}

#[repr(i32)]
enum Data {
    Status = 0,
    Total = 2,
    SelectedIndex = 3,
    SelectedMatch = 4,
    ViewportMatches = 6,
}

#[repr(C)]
struct SelectionBuffer {
    ptr: *mut GhosttySelection,
    cap: usize,
    len: usize,
}

unsafe extern "C" {
    fn ghostty_search_new(
        allocator: *const GhosttyAllocator,
        search: *mut *mut c_void,
        terminal: GhosttyTerminal,
    ) -> GhosttyResult;
    fn ghostty_search_free(search: *mut c_void);
    fn ghostty_search_set(
        search: *mut c_void,
        key: OptionKey,
        value: *const c_void,
    ) -> GhosttyResult;
    fn ghostty_search_get(search: *mut c_void, key: Data, value: *mut c_void) -> GhosttyResult;
    fn ghostty_search_feed(search: *mut c_void) -> GhosttyResult;
    fn ghostty_search_tick(search: *mut c_void, status: *mut Status) -> GhosttyResult;
    fn ghostty_grid_ref_cell(
        reference: *const GhosttyGridRef,
        cell: *mut GhosttyCell,
    ) -> GhosttyResult;
    fn ghostty_terminal_point_from_grid_ref(
        terminal: GhosttyTerminal,
        reference: *const GhosttyGridRef,
        tag: GhosttyPointTag,
        point: *mut GhosttyPointCoordinate,
    ) -> GhosttyResult;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchProgress {
    pub total: usize,
    pub selected: Option<usize>,
    pub running: bool,
    pub generation: u64,
}

pub(super) struct Search {
    handle: *mut c_void,
    fed_generation: Option<u64>,
    status: Status,
    progress: SearchProgress,
}

// The handle is used only under VtScreen::search. Terminal-dependent calls
// additionally hold VtScreen::inner. No grid reference leaves those locks.
unsafe impl Send for Search {}

fn check(rc: GhosttyResult) -> Result<(), String> {
    if rc == GHOSTTY_SUCCESS {
        Ok(())
    } else {
        Err(format!("Ghostty search failed: rc={rc}"))
    }
}

impl Drop for Search {
    fn drop(&mut self) {
        // All callers dropping a live search also hold the terminal lock.
        unsafe { ghostty_search_free(self.handle) };
    }
}

impl Search {
    fn feed(&mut self, inner: &VtInner) -> Result<(), String> {
        if self.fed_generation != Some(inner.generation) || self.status == Status::FeedRequired {
            check(unsafe { ghostty_search_feed(self.handle) })?;
            check(unsafe {
                ghostty_search_get(
                    self.handle,
                    Data::Status,
                    (&mut self.status as *mut Status).cast(),
                )
            })?;
            self.fed_generation = Some(inner.generation);
        }
        Ok(())
    }

    fn progress(&self, generation: u64) -> Result<SearchProgress, String> {
        let mut total = 0usize;
        let mut selected = 0usize;
        check(unsafe {
            ghostty_search_get(self.handle, Data::Total, (&mut total as *mut usize).cast())
        })?;
        let rc = unsafe {
            ghostty_search_get(
                self.handle,
                Data::SelectedIndex,
                (&mut selected as *mut usize).cast(),
            )
        };
        if rc != GHOSTTY_NO_VALUE {
            check(rc)?;
        }
        Ok(SearchProgress {
            total,
            selected: (rc == GHOSTTY_SUCCESS).then_some(selected),
            running: self.status != Status::Complete,
            generation,
        })
    }

    pub(super) fn highlights(&mut self, inner: &VtInner) -> Result<Vec<Highlight>, String> {
        self.feed(inner)?;
        self.capture_highlights(inner.terminal, inner.cols, inner.rows, false)
    }

    pub(super) fn capture_highlights(
        &mut self,
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        reconcile: bool,
    ) -> Result<Vec<Highlight>, String> {
        // A parser callback can run several times before the feed generation
        // changes. Reconcile native pins against the terminal at this boundary.
        if reconcile {
            check(unsafe { ghostty_search_feed(self.handle) })?;
            self.fed_generation = None;
        }
        let mut buffer = SelectionBuffer {
            ptr: std::ptr::null_mut(),
            cap: 0,
            len: 0,
        };
        let rc = unsafe {
            ghostty_search_get(
                self.handle,
                Data::ViewportMatches,
                (&mut buffer as *mut SelectionBuffer).cast(),
            )
        };
        if rc != GHOSTTY_OUT_OF_SPACE {
            check(rc)?;
        }
        let mut matches = vec![GhosttySelection::default(); buffer.len];
        buffer.ptr = matches.as_mut_ptr();
        buffer.cap = matches.len();
        check(unsafe {
            ghostty_search_get(
                self.handle,
                Data::ViewportMatches,
                (&mut buffer as *mut SelectionBuffer).cast(),
            )
        })?;
        matches.truncate(buffer.len);

        // Screen coordinates permit clipping a wrapped match whose endpoint
        // lies outside the viewport. Viewport conversion alone would omit it.
        let mut origin = GhosttyGridRef::default();
        check(unsafe {
            ghostty_terminal_grid_ref(
                terminal,
                GhosttyPoint {
                    tag: GhosttyPointTag::Viewport,
                    value: GhosttyPointValue {
                        coordinate: GhosttyPointCoordinate::default(),
                    },
                },
                &mut origin,
            )
        })?;
        let origin = screen_point(terminal, &origin)?;
        let mut selected = GhosttySelection::default();
        let rc = unsafe {
            ghostty_search_get(
                self.handle,
                Data::SelectedMatch,
                (&mut selected as *mut GhosttySelection).cast(),
            )
        };
        if rc != GHOSTTY_NO_VALUE {
            check(rc)?;
        }
        let selected = if rc == GHOSTTY_SUCCESS {
            Some((
                screen_point(terminal, &selected.start)?,
                screen_point(terminal, &selected.end)?,
            ))
        } else {
            None
        };
        let mut highlights = Vec::new();
        for selection in matches {
            let start = screen_point(terminal, &selection.start)?;
            let end = screen_point(terminal, &selection.end)?;
            let mut cell = 0;
            let mut wide = 0i32;
            check(unsafe { ghostty_grid_ref_cell(&selection.end, &mut cell) })?;
            check(unsafe {
                ghostty_cell_get(cell, GhosttyCellData::Wide, (&mut wide as *mut i32).cast())
            })?;
            let end_col = (end.x + u16::from(wide == 1)).min(cols - 1);
            let is_selected = selected.is_some_and(|(a, b)| {
                a.x == start.x && a.y == start.y && b.x == end.x && b.y == end.y
            });
            for row in start.y.max(origin.y)..=end.y.min(origin.y + u32::from(rows) - 1) {
                highlights.push(Highlight {
                    row: (row - origin.y) as u16,
                    start: if row == start.y { start.x } else { 0 },
                    end: if row == end.y { end_col } else { cols - 1 },
                    selected: is_selected,
                });
            }
        }
        Ok(highlights)
    }
}

fn screen_point(
    terminal: GhosttyTerminal,
    reference: &GhosttyGridRef,
) -> Result<GhosttyPointCoordinate, String> {
    let mut point = GhosttyPointCoordinate::default();
    check(unsafe {
        ghostty_terminal_point_from_grid_ref(
            terminal,
            reference,
            GhosttyPointTag::Screen,
            &mut point,
        )
    })?;
    Ok(point)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Highlight {
    pub(super) row: u16,
    start: u16,
    end: u16,
    selected: bool,
}

pub(super) fn paint(snapshot: &mut ScreenSnapshot, highlights: &[Highlight]) {
    if highlights.is_empty() {
        return;
    }
    // Union overlapping matches before touching colors. Selected spans take
    // precedence and every cell is tinted at most once.
    let mut coverage = vec![0u8; snapshot.cells.len()];
    for highlight in highlights {
        let offset = usize::from(highlight.row) * usize::from(snapshot.cols);
        for col in highlight.start..=highlight.end {
            if let Some(value) = coverage.get_mut(offset + usize::from(col)) {
                *value = (*value).max(if highlight.selected { 2 } else { 1 });
            }
        }
    }
    for (index, (cell, coverage)) in snapshot.cells.iter_mut().zip(coverage).enumerate() {
        if coverage == 0 {
            continue;
        }
        let row = index / usize::from(snapshot.cols);
        let col = (index % usize::from(snapshot.cols)) as u16;
        if snapshot
            .selection_ranges
            .get(row)
            .copied()
            .flatten()
            .is_some_and(|range| range.contains(col))
        {
            // User selection is painted by the renderer; do not invert twice.
            continue;
        }
        if coverage == 2 {
            cell.attrs ^= ATTR_INVERSE;
        } else {
            // Monochrome, theme-derived highlight. Keep explicit opacity so
            // blank cells are painted by the Windows default-bg fast path.
            let (fg, bg) = if cell.attrs & ATTR_INVERSE == 0 {
                (cell.fg, cell.bg)
            } else {
                (cell.bg, cell.fg)
            };
            let mut tinted = 0xff;
            for shift in [8, 16, 24] {
                tinted |= ((((fg >> shift) & 0xff) + 3 * ((bg >> shift) & 0xff)) / 4) << shift;
            }
            if cell.attrs & ATTR_INVERSE == 0 {
                cell.bg = tinted;
            } else {
                cell.fg = tinted;
            }
        }
    }
}

impl VtScreen {
    pub fn search(&self, needle: &str) -> Result<bool, String> {
        let mut search = self.search.lock();
        let mut inner = self.inner.lock();
        if needle.is_empty() {
            *search = None;
        } else {
            if search.is_none() {
                let mut handle = std::ptr::null_mut();
                check(unsafe {
                    ghostty_search_new(std::ptr::null(), &mut handle, inner.terminal)
                })?;
                *search = Some(Search {
                    handle,
                    fed_generation: None,
                    status: Status::Complete,
                    progress: SearchProgress::default(),
                });
            }
            let state = search.as_mut().expect("search created above");
            let needle = GhosttyString {
                ptr: needle.as_ptr(),
                len: needle.len(),
            };
            check(unsafe {
                ghostty_search_set(
                    state.handle,
                    OptionKey::Needle,
                    (&needle as *const GhosttyString).cast(),
                )
            })?;
            state.fed_generation = None;
        }
        inner.generation = inner.generation.wrapping_add(1);
        Ok(true)
    }

    pub fn navigate_search(&self, previous: bool) -> Result<bool, String> {
        let mut search = self.search.lock();
        let Some(search) = search.as_mut() else {
            return Ok(false);
        };
        let mut inner = self.inner.lock();
        let rc = unsafe {
            ghostty_search_set(
                search.handle,
                if previous {
                    OptionKey::Previous
                } else {
                    OptionKey::Next
                },
                std::ptr::null(),
            )
        };
        if rc != GHOSTTY_NO_VALUE {
            check(rc)?;
        }
        inner.generation = inner.generation.wrapping_add(1);
        search.fed_generation = None;
        Ok(rc == GHOSTTY_SUCCESS)
    }

    /// One bounded search step; the caller schedules this off the UI thread.
    pub fn search_step(&self) -> Result<Option<SearchProgress>, String> {
        let mut search = self.search.lock();
        let Some(search) = search.as_mut() else {
            return Ok(None);
        };
        {
            let inner = self.inner.lock();
            if search.status == Status::Complete
                && search.fed_generation == Some(inner.generation)
                && search.progress.generation == inner.generation
            {
                return Ok(Some(search.progress));
            }
            search.feed(&inner)?;
        }
        check(unsafe { ghostty_search_tick(search.handle, &mut search.status) })?;
        let mut inner = self.inner.lock();
        // Output can arrive while tick scans copied data. Reconcile before
        // exposing counts/selection; stale C grid references are never retained.
        search.feed(&inner)?;
        let mut progress = search.progress(inner.generation)?;
        if (progress.total, progress.selected, progress.running)
            != (
                search.progress.total,
                search.progress.selected,
                search.progress.running,
            )
        {
            inner.generation = inner.generation.wrapping_add(1);
            search.fed_generation = Some(inner.generation);
            progress.generation = inner.generation;
        }
        search.progress = progress;
        Ok(Some(progress))
    }
}

impl crate::GhosttyTerminal {
    pub fn search(&self, needle: &str) -> Result<bool, String> {
        self.search_screen()
            .map_or(Ok(false), |screen| screen.search(needle))
    }

    pub fn end_search(&self) -> Result<bool, String> {
        self.search("")
    }

    pub fn navigate_search_next(&self) -> Result<bool, String> {
        self.search_screen()
            .map_or(Ok(false), |screen| screen.navigate_search(false))
    }

    pub fn navigate_search_previous(&self) -> Result<bool, String> {
        self.search_screen()
            .map_or(Ok(false), |screen| screen.navigate_search(true))
    }

    pub fn search_step(&self) -> Result<Option<SearchProgress>, String> {
        self.search_screen()
            .map_or(Ok(None), |screen| screen.search_step())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(screen: &VtScreen) -> SearchProgress {
        for _ in 0..1000 {
            let progress = screen.search_step().unwrap().unwrap();
            if !progress.running {
                return progress;
            }
        }
        panic!("search did not converge");
    }

    #[test]
    fn text_snapshot_waits_for_search_while_render_snapshot_skips_contention() {
        use std::sync::mpsc;
        use std::time::Duration;

        let screen = Arc::new(VtScreen::new(24, 4, None).unwrap());
        screen.feed(b"retained text");
        let expected = screen.snapshot().cells;
        let search = screen.search.lock();
        assert!(screen.try_snapshot().is_none());
        let (started_tx, started_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let reader = Arc::clone(&screen);
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            tx.send(reader.snapshot().cells).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let waiting = rx.recv_timeout(Duration::from_millis(100));
        // A waiting text reader must not monopolize the render lock.
        let render_available = screen.render.try_lock().is_some();
        drop(search);
        worker.join().unwrap();
        assert!(matches!(waiting, Err(mpsc::RecvTimeoutError::Timeout)));
        assert!(render_available);
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), expected);
    }

    #[test]
    fn synchronized_output_freezes_search_at_the_parser_boundary() {
        for prefix in [b"\r\nfourth".as_slice(), b"\x1b[2;1Hbar\x1b[3;1H\r\nfourth"] {
            let reference = VtScreen::new(12, 3, None).unwrap();
            let held = VtScreen::new(12, 3, None).unwrap();
            for screen in [&reference, &held] {
                screen.feed(b"first\r\nfoo\r\nthird");
                screen.search("foo").unwrap();
                assert_eq!(complete(screen).total, 1);
            }
            // Move or invalidate the match before SET, then scroll again
            // after it, all within one parser feed. Both stale matches and
            // projecting against the final viewport give the wrong overlay.
            reference.feed(prefix);
            let expected = reference.snapshot();
            held.feed(&[prefix, b"\x1b[?2026h\r\nfoo\r\nsixth"].concat());
            let captured = held.snapshot();
            assert_eq!(captured.cells, expected.cells);
            assert_eq!(captured.scrollbar, expected.scrollbar);
            complete(&held);
            assert_eq!(held.snapshot().cells, expected.cells);
            held.feed(b"\x1b[?2026l");
            assert_ne!(held.snapshot().cells, expected.cells);
        }
    }

    #[test]
    fn search_counts_navigates_wraps_and_closes_without_stale_highlights() {
        let screen = VtScreen::new(24, 4, None).unwrap();
        screen.feed(b"first Foo\r\nsecond fOO\r\nthird foo");
        let before = screen.snapshot();
        screen.search("foo").unwrap();
        assert_eq!(complete(&screen).total, 3);
        assert_ne!(screen.snapshot().cells, before.cells);
        assert!(screen.navigate_search(false).unwrap());
        assert_eq!(complete(&screen).selected, Some(0));
        assert!(screen.navigate_search(true).unwrap());
        assert_eq!(complete(&screen).selected, Some(2));
        assert!(screen.navigate_search(false).unwrap());
        assert_eq!(complete(&screen).selected, Some(0));
        let settled = complete(&screen);
        assert_eq!(
            complete(&screen),
            settled,
            "idle search must not repaint forever"
        );
        screen.search("").unwrap();
        assert!(screen.search_step().unwrap().is_none());
        assert_eq!(screen.snapshot().cells, before.cells);
        screen.search("absent").unwrap();
        assert_eq!(complete(&screen).total, 0);
        assert!(!screen.navigate_search(false).unwrap());
    }

    #[test]
    fn search_tracks_output_scrollback_resize_and_screen_switches() {
        let screen = VtScreen::new(30, 3, None).unwrap();
        screen.feed(b"history needle\r\n");
        for _ in 0..40 {
            screen.feed(b"filler\r\n");
        }
        screen.feed(b"visible needle");
        screen.search("needle").unwrap();
        assert_eq!(complete(&screen).total, 2);
        screen.navigate_search(false).unwrap();
        screen.navigate_search(false).unwrap();
        let snapshot = screen.snapshot();
        assert!(
            snapshot
                .cells
                .iter()
                .any(|cell| cell.codepoint == u32::from(b'h'))
        );
        screen.feed(b"\r\nnew needle");
        assert_eq!(complete(&screen).total, 3);
        screen.resize(17, 4, 8, 16).unwrap();
        assert_eq!(complete(&screen).total, 3);
        assert!(screen.try_snapshot().is_some());
        screen.feed(b"\x1b[?1049hALT needle");
        complete(&screen);
        assert!(screen.navigate_search(false).unwrap());
        assert!(screen.try_snapshot().is_some());
        screen.feed(b"\x1b[?1049l");
        assert_eq!(complete(&screen).total, 3);
        screen.clear_screen_and_scrollback();
        assert_eq!(complete(&screen).total, 0);
        assert!(screen.try_snapshot().is_some());
    }

    #[test]
    fn search_uses_ascii_case_folding_but_exact_unicode_and_wrapped_matches() {
        let screen = VtScreen::new(8, 4, None).unwrap();
        screen.feed("Ää 界\r\n123456XYZ".as_bytes());
        screen.search("ä").unwrap();
        assert_eq!(complete(&screen).total, 1);
        screen.search("界").unwrap();
        assert_eq!(complete(&screen).total, 1);
        screen.search("xyz").unwrap();
        assert_eq!(complete(&screen).total, 1);
        let mut search = screen.search.lock();
        let inner = screen.inner.lock();
        let spans = search.as_mut().unwrap().highlights(&inner).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].end), (6, 7));
        assert_eq!((spans[1].start, spans[1].end), (0, 0));
    }

    #[test]
    fn overlapping_matches_and_wide_glyphs_are_highlighted_once() {
        let screen = VtScreen::new(12, 3, None).unwrap();
        screen.feed("aaa A界B".as_bytes());
        let before = screen.snapshot();
        screen.acknowledge_snapshot(before.generation);
        screen.search("aa").unwrap();
        assert_eq!(complete(&screen).total, 2);
        let snapshot = screen.snapshot();
        assert_eq!(snapshot.cells[0].bg, snapshot.cells[1].bg);
        assert_eq!(snapshot.cells[1].bg, snapshot.cells[2].bg);
        assert_ne!(snapshot.cells[1].bg, before.cells[1].bg);
        assert_eq!(
            snapshot.dirty_rows,
            vec![0],
            "search must not redraw unrelated rows"
        );
        screen.navigate_search(false).unwrap();
        complete(&screen);
        let selected = screen.snapshot();
        assert_eq!(selected.cells[1].attrs & ATTR_INVERSE, ATTR_INVERSE);
        assert_eq!(selected.cells[2].attrs & ATTR_INVERSE, ATTR_INVERSE);
        assert_eq!(selected.cells[1].bg, before.cells[1].bg);

        screen.search("界").unwrap();
        complete(&screen);
        let snapshot = screen.snapshot();
        assert_ne!(snapshot.cells[5].bg, before.cells[5].bg);
        assert_eq!(snapshot.cells[5].bg, snapshot.cells[6].bg);
        screen.navigate_search(false).unwrap();
        complete(&screen);
        let selected = screen.snapshot();
        assert_eq!(selected.cells[5].attrs & ATTR_INVERSE, ATTR_INVERSE);
        assert_eq!(selected.cells[6].attrs & ATTR_INVERSE, ATTR_INVERSE);
    }

    #[test]
    fn user_selection_takes_precedence_over_selected_search_match() {
        let original = Cell {
            codepoint: u32::from(b'x'),
            fg: 0xeeeeeeff,
            bg: 0x20202000,
            ..Cell::default()
        };
        let mut snapshot = ScreenSnapshot {
            cols: 3,
            rows: 1,
            cells: vec![original.clone(); 3],
            selection_ranges: vec![Some(SelectionRange { start: 1, end: 1 })],
            ..ScreenSnapshot::default()
        };
        paint(
            &mut snapshot,
            &[Highlight {
                row: 0,
                start: 0,
                end: 2,
                selected: true,
            }],
        );
        assert_eq!(snapshot.cells[0].attrs, ATTR_INVERSE);
        assert_eq!(snapshot.cells[1], original);
        assert_eq!(snapshot.cells[2].attrs, ATTR_INVERSE);
        // The Windows renderer applies user selection after search painting.
        assert_eq!(snapshot.cells[1].attrs ^ ATTR_INVERSE, ATTR_INVERSE);
    }

    #[test]
    fn search_ffi_matches_linked_manifest() {
        let manifest: serde_json::Value = unsafe {
            serde_json::from_str(
                std::ffi::CStr::from_ptr(ghostty_type_json())
                    .to_str()
                    .unwrap(),
            )
            .unwrap()
        };
        let types = &manifest["types"];
        assert_eq!(
            types["GhosttySearchOption"]["values"]["NEEDLE"].as_i64(),
            Some(OptionKey::Needle as i64)
        );
        assert_eq!(
            types["GhosttySearchOption"]["values"]["SELECT_NEXT"].as_i64(),
            Some(OptionKey::Next as i64)
        );
        assert_eq!(
            types["GhosttySearchOption"]["values"]["SELECT_PREV"].as_i64(),
            Some(OptionKey::Previous as i64)
        );
        for (name, key) in [
            ("STATUS", Data::Status),
            ("TOTAL_MATCHES", Data::Total),
            ("SELECTED_INDEX", Data::SelectedIndex),
            ("SELECTED_MATCH", Data::SelectedMatch),
            ("VIEWPORT_MATCHES", Data::ViewportMatches),
        ] {
            assert_eq!(
                types["GhosttySearchData"]["values"][name].as_i64(),
                Some(key as i64)
            );
        }
        assert_eq!(
            types["GhosttySelectionBuffer"]["size"].as_u64(),
            Some(std::mem::size_of::<SelectionBuffer>() as u64)
        );
        assert_eq!(
            types["GhosttyPointTag"]["values"]["SCREEN"].as_i64(),
            Some(GhosttyPointTag::Screen as i64)
        );
        for (name, status) in [
            ("RUNNING", Status::Running),
            ("FEED_REQUIRED", Status::FeedRequired),
            ("COMPLETE", Status::Complete),
        ] {
            assert_eq!(
                types["GhosttySearchStatus"]["values"][name].as_i64(),
                Some(status as i64)
            );
        }
    }
}
