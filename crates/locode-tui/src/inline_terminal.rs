//! A minimal inline-viewport terminal with a **dynamic height** — the one thing
//! stock ratatui 0.29 can't do (its `Viewport::Inline(h)` is fixed and
//! `set_viewport_area` is private). Both Rust/ratatui references vendor a
//! terminal for this: codex `custom_terminal.rs`, grok `xai-ratatui-inline`
//! (ADR-0019 amendment 2026-07-22).
//!
//! We keep it small by reusing ratatui's **public** `Buffer` and `Backend`
//! (diffing, cell drawing, and the scroll-region ops all come from there) and
//! only add what ratatui withholds:
//!
//! - [`InlineTerminal::draw`] — render → `Buffer::diff` → `Backend::draw` (the
//!   stock double-buffer loop);
//! - [`InlineTerminal::insert_before`] — the transcript path: `scroll_region_up`
//!   above the viewport, then draw the freed rows (ratatui's
//!   `insert_before_scrolling_regions` recipe), so native scrollback still owns
//!   settled history;
//! - [`InlineTerminal::set_height`] — the new capability: resize the viewport
//!   with its top anchored to the transcript — grow the bottom edge down (or
//!   `scroll_region_up` the transcript when growing past the screen bottom),
//!   shrink the bottom edge up. No `scroll_region_down` (it can't pull settled
//!   transcript out of native scrollback → would corrupt it), so no blink.
//!
//! The geometry is unit-tested against ratatui's `TestBackend` (which implements
//! the scroll-region ops and a `scrollback()` buffer); only raw escape-sequence
//! behavior needs a real-terminal smoke.

use std::io;

use ratatui::backend::Backend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

/// The per-frame rendering handle. Mirrors the slice of `ratatui::Frame`'s API
/// the UI uses (`area`, `render_widget`, `set_cursor_position`) so switching the
/// UI over is mechanical.
pub struct Frame<'a> {
    area: Rect,
    buffer: &'a mut Buffer,
    cursor: Option<Position>,
}

impl Frame<'_> {
    /// The viewport area for this frame.
    #[must_use]
    pub fn area(&self) -> Rect {
        self.area
    }

    /// Render a widget into the frame buffer.
    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    /// Show the terminal cursor at `position` after this frame (else hidden).
    pub fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor = Some(position.into());
    }
}

/// An inline terminal whose viewport height can change at runtime.
pub struct InlineTerminal<B: Backend> {
    backend: B,
    /// Double buffer: `current` is drawn into, then diffed against the other.
    buffers: [Buffer; 2],
    current: usize,
    /// The live viewport. Its top is anchored to the transcript; the bottom edge
    /// moves as the composer grows/shrinks (bottom-anchored only while it fits at
    /// the screen bottom).
    viewport: Rect,
    /// Last known terminal size.
    screen: Size,
    cursor_hidden: bool,
}

impl<B: Backend> InlineTerminal<B> {
    /// Create the terminal, reserving `height` rows for a bottom-anchored inline
    /// viewport (mirrors ratatui's `compute_inline_size`: append lines to make
    /// room, scrolling existing content up if the cursor is near the bottom).
    ///
    /// # Errors
    /// Propagates backend size/cursor/scroll failures.
    pub fn new(mut backend: B, height: u16) -> io::Result<Self> {
        let screen = backend.size()?;
        let cursor = backend
            .get_cursor_position()
            .unwrap_or(Position { x: 0, y: 0 });
        let height = height.clamp(1, screen.height.max(1));
        let lines_after = height.saturating_sub(1);
        backend.append_lines(lines_after)?;
        let available = screen.height.saturating_sub(cursor.y).saturating_sub(1);
        let missing = lines_after.saturating_sub(available);
        let top = cursor.y.saturating_sub(missing);
        let viewport = Rect {
            x: 0,
            y: top,
            width: screen.width,
            height,
        };
        Ok(Self {
            backend,
            buffers: [Buffer::empty(viewport), Buffer::empty(viewport)],
            current: 0,
            viewport,
            screen,
            cursor_hidden: false,
        })
    }

    /// The current viewport height.
    #[must_use]
    pub fn height(&self) -> u16 {
        self.viewport.height
    }

    /// The terminal size (queried fresh).
    ///
    /// # Errors
    /// Propagates the backend size query.
    pub fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }

    /// Render one frame: run `render` into the back buffer, then flush only the
    /// diff against what's on screen.
    ///
    /// # Errors
    /// Propagates backend draw/flush failures.
    pub fn draw<F: FnOnce(&mut Frame<'_>)>(&mut self, render: F) -> io::Result<()> {
        self.autoresize()?;
        let cursor = {
            let mut frame = Frame {
                area: self.viewport,
                buffer: &mut self.buffers[self.current],
                cursor: None,
            };
            render(&mut frame);
            frame.cursor
        };

        // Diff the freshly-rendered buffer against the previous one and draw the
        // delta. Collect owned so the backend borrow doesn't clash with buffers.
        let updates: Vec<(u16, u16, Cell)> = self.buffers[1 - self.current]
            .diff(&self.buffers[self.current])
            .into_iter()
            .map(|(x, y, cell)| (x, y, cell.clone()))
            .collect();
        self.backend
            .draw(updates.iter().map(|(x, y, cell)| (*x, *y, cell)))?;

        match cursor {
            None => {
                if !self.cursor_hidden {
                    self.backend.hide_cursor()?;
                    self.cursor_hidden = true;
                }
            }
            Some(pos) => {
                self.backend.set_cursor_position(pos)?;
                if self.cursor_hidden {
                    self.backend.show_cursor()?;
                    self.cursor_hidden = false;
                }
            }
        }
        self.backend.flush()?;

        // Swap: reset the old front buffer (becomes the next back buffer).
        self.buffers[1 - self.current].reset();
        self.current = 1 - self.current;
        Ok(())
    }

    /// Print `lines` into native scrollback just above the viewport (print-once
    /// transcript). Scrolls the region above the viewport up and draws the freed
    /// rows — ratatui's `insert_before_scrolling_regions` recipe for the common
    /// case (bottom-anchored viewport, not filling the screen).
    ///
    /// # Errors
    /// Propagates backend scroll/draw failures.
    pub fn insert_before(&mut self, lines: &[Line<'_>]) -> io::Result<()> {
        let mut height = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        if height == 0 || self.viewport.width == 0 {
            return Ok(());
        }
        // Render the lines into a temporary buffer, then draw them row-by-row
        // into space freed by scrolling the region above the viewport up.
        let area = Rect::new(0, 0, self.viewport.width, height);
        let mut buffer = Buffer::empty(area);
        Paragraph::new(lines.to_vec()).render(area, &mut buffer);
        let mut cells = buffer.content.as_slice();

        let vp_top = self.viewport.top();
        while height > 0 {
            let to_draw = height.min(vp_top);
            if to_draw == 0 {
                break; // viewport fills the screen — nowhere to insert (rare).
            }
            self.backend.scroll_region_up(0..vp_top, to_draw)?;
            cells = self.draw_cells(vp_top - to_draw, to_draw, cells)?;
            height -= to_draw;
        }
        self.backend.flush()?;
        Ok(())
    }

    /// Resize the live viewport to `height` rows (codex's `custom_terminal`
    /// approach). The viewport top is **anchored to the transcript**:
    ///
    /// - **grow with room below** → the bottom edge extends down (no scroll);
    /// - **grow past the screen bottom** → `scroll_region_up` scrolls the
    ///   transcript up as one block, then bottom-anchor;
    /// - **shrink** → the bottom edge retracts up (no scroll).
    ///
    /// We never `scroll_region_down` on shrink: on a real terminal that inserts
    /// blank lines (it can't pull settled transcript back out of native
    /// scrollback), which corrupted the transcript. Instead the region from the
    /// (unchanged or raised) top down is cleared and the viewport repainted, so
    /// any rows the viewport vacated at the bottom go blank cleanly.
    ///
    /// # Errors
    /// Propagates backend scroll/draw failures.
    pub fn set_height(&mut self, height: u16) -> io::Result<()> {
        let screen = self.backend.size()?;
        self.screen = screen;
        let height = height.clamp(1, screen.height.max(1));
        let old = self.viewport;
        if height == old.height {
            return Ok(());
        }
        let mut area = Rect {
            x: 0,
            y: old.y,
            width: screen.width,
            height,
        };
        if area.bottom() > screen.height {
            // Grow past the bottom: scroll the transcript up, then bottom-anchor.
            let scroll_by = area.bottom() - screen.height;
            if area.y > 0 {
                self.backend
                    .scroll_region_up(0..area.y, scroll_by.min(area.y))?;
            }
            area.y = screen.height - area.height;
        }
        // Clear the affected region (old viewport ∪ new viewport) so nothing
        // stale survives — especially rows the viewport vacated at the bottom on
        // shrink. The sentinel-primed repaint then fills the new viewport. All of
        // this flushes with the coming draw (no intermediate frame → no flicker).
        let clear_top = old.y.min(area.y);
        self.clear_rows(clear_top, screen.height)?;
        self.set_viewport_area(area);
        Ok(())
    }

    /// Draw blank cells over rows `[y_start, y_end)` (deterministic on both the
    /// real backend and `TestBackend`, unlike `clear_region`'s cursor-relative
    /// semantics).
    fn clear_rows(&mut self, y_start: u16, y_end: u16) -> io::Result<()> {
        if y_end <= y_start || self.screen.width == 0 {
            return Ok(());
        }
        let area = Rect::new(0, y_start, self.screen.width, y_end - y_start);
        let mut sentinel = Buffer::empty(area);
        for cell in &mut sentinel.content {
            cell.set_symbol("\u{0}");
        }
        let blank = Buffer::empty(area);
        let updates: Vec<(u16, u16, Cell)> = sentinel
            .diff(&blank)
            .into_iter()
            .map(|(x, y, cell)| (x, y, cell.clone()))
            .collect();
        self.backend
            .draw(updates.iter().map(|(x, y, cell)| (*x, *y, cell)))?;
        Ok(())
    }

    /// Access the backend (for teardown flushes / test inspection).
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Access the backend immutably (test inspection).
    #[must_use]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    // --- internals ---

    /// Set the viewport and prime a **full repaint** of it on the next draw:
    /// the front (draw-into) buffer is cleared, and the back (diff-against)
    /// buffer is filled with a sentinel so `diff` reports every cell — including
    /// blanks, which then get drawn as spaces over any stale/ghost content left
    /// on screen in the resized region.
    fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current] = Buffer::empty(area);
        let mut stale = Buffer::empty(area);
        for cell in &mut stale.content {
            cell.set_symbol("\u{0}"); // never equals a rendered cell → full repaint
        }
        self.buffers[1 - self.current] = stale;
        self.viewport = area;
    }

    fn autoresize(&mut self) -> io::Result<()> {
        let screen = self.backend.size()?;
        if screen != self.screen {
            self.screen = screen;
            let height = self.viewport.height.clamp(1, screen.height.max(1));
            let area = Rect {
                x: 0,
                y: screen.height.saturating_sub(height),
                width: screen.width,
                height,
            };
            self.set_viewport_area(area);
            self.buffers[1 - self.current].reset();
        }
        Ok(())
    }

    /// Draw `n` rows of `cells` at row `y` over cleared space (ratatui's
    /// `draw_lines_over_cleared`). Returns the unconsumed tail of `cells`.
    fn draw_cells<'a>(&mut self, y: u16, n: u16, cells: &'a [Cell]) -> io::Result<&'a [Cell]> {
        let width = usize::from(self.screen.width);
        let (to_draw, rest) = cells.split_at(width * n as usize);
        let area = Rect::new(0, y, self.screen.width, n);
        let old = Buffer::empty(area);
        let new = Buffer {
            area,
            content: to_draw.to_vec(),
        };
        let updates: Vec<(u16, u16, Cell)> = old
            .diff(&new)
            .into_iter()
            .map(|(x, y, cell)| (x, y, cell.clone()))
            .collect();
        self.backend
            .draw(updates.iter().map(|(x, y, cell)| (*x, *y, cell)))?;
        Ok(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::text::Line;

    /// A terminal wired to a `TestBackend` at `w x h`, cursor at the bottom row.
    fn term(w: u16, h: u16, height: u16) -> InlineTerminal<TestBackend> {
        let mut backend = TestBackend::new(w, h);
        // Put the cursor at the last row so the viewport anchors at the bottom.
        backend
            .set_cursor_position(Position { x: 0, y: h - 1 })
            .unwrap();
        InlineTerminal::new(backend, height).unwrap()
    }

    /// The visible buffer rows as trimmed strings.
    fn rows(t: &InlineTerminal<TestBackend>) -> Vec<String> {
        let b = t.backend.buffer();
        (0..b.area.height)
            .map(|y| {
                (0..b.area.width)
                    .map(|x| b[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn scrollback(t: &InlineTerminal<TestBackend>) -> Vec<String> {
        let b = t.backend.scrollback();
        (0..b.area.height)
            .map(|y| {
                (0..b.area.width)
                    .map(|x| b[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn starts_bottom_anchored() {
        let t = term(20, 10, 4);
        assert_eq!(t.height(), 4);
        // Viewport occupies the bottom 4 rows.
        assert_eq!(t.viewport.y, 6);
        assert_eq!(t.viewport.bottom(), 10);
    }

    #[test]
    fn draw_renders_into_the_viewport() {
        let mut t = term(20, 10, 4);
        t.draw(|f| {
            f.render_widget(Paragraph::new("hello"), f.area());
        })
        .unwrap();
        // "hello" lands on the first viewport row (row 6).
        assert_eq!(rows(&t)[6], "hello");
    }

    #[test]
    fn grow_scrolls_transcript_up_and_bottom_anchors() {
        let mut t = term(20, 10, 4);
        // Put some transcript above the viewport.
        t.insert_before(&[Line::from("t1"), Line::from("t2"), Line::from("t3")])
            .unwrap();
        let before = rows(&t);
        assert!(before.iter().any(|r| r == "t3"), "{before:?}");
        t.set_height(6).unwrap();
        assert_eq!(t.height(), 6);
        assert_eq!(t.viewport.y, 4, "bottom-anchored at new height");
        assert_eq!(t.viewport.bottom(), 10);
    }

    #[test]
    fn shrink_keeps_top_anchored_blank_below() {
        let mut t = term(20, 12, 8);
        assert_eq!(t.viewport.y, 4, "starts at [4,12)");
        t.set_height(4).unwrap();
        assert_eq!(t.height(), 4);
        // Top stays anchored to the transcript (no gap above); the bottom edge
        // retracts up, leaving blank below (no scroll_region_down → no transcript
        // corruption).
        assert_eq!(t.viewport.y, 4, "top anchored");
        assert_eq!(t.viewport.bottom(), 8, "bottom retracted");
    }

    #[test]
    fn grow_then_shrink_round_trips_height() {
        let mut t = term(30, 20, 4);
        for h in [4u16, 8, 12, 6, 3, 10, 4] {
            t.set_height(h).unwrap();
            assert_eq!(t.height(), h.max(1), "height applied at h={h}");
            assert!(t.viewport.bottom() <= 20, "within screen at h={h}");
            assert!(
                t.viewport.y + t.viewport.height <= 20,
                "no overflow at h={h}"
            );
        }
    }

    #[test]
    fn set_height_clamps_and_is_noop_when_unchanged() {
        let mut t = term(20, 10, 4);
        t.set_height(4).unwrap(); // no-op
        assert_eq!(t.height(), 4);
        t.set_height(100).unwrap(); // clamp to screen height
        assert_eq!(t.height(), 10);
        t.set_height(0).unwrap(); // clamp to >= 1
        assert_eq!(t.height(), 1);
    }

    #[test]
    fn resize_repaints_without_ghosts() {
        use ratatui::widgets::Paragraph;
        let mut t = term(20, 12, 4);
        // Draw distinctive content in the small (bottom-4) viewport.
        let old_area = t.viewport;
        t.draw(|f| f.render_widget(Paragraph::new("GHOST"), Rect::new(0, old_area.y, 20, 1)))
            .unwrap();
        assert!(rows(&t).iter().any(|r| r.contains("GHOST")));
        // Grow: the old content sits inside the new taller viewport and must be
        // repainted away (not left as a stale ghost).
        t.set_height(8).unwrap();
        let bottom = t.viewport.bottom() - 1;
        t.draw(|f| f.render_widget(Paragraph::new("FRESH"), Rect::new(0, bottom, 20, 1)))
            .unwrap();
        let r = rows(&t);
        assert!(
            !r.iter().any(|l| l.contains("GHOST")),
            "ghost cleared: {r:?}"
        );
        assert!(r.iter().any(|l| l.contains("FRESH")), "new content: {r:?}");
    }

    #[test]
    fn insert_before_pushes_lines_into_scrollback_when_room_runs_out() {
        // Viewport is tall (8 of 10); inserting 5 lines forces some to scroll
        // off into scrollback without panicking.
        let mut t = term(20, 10, 8);
        let lines: Vec<Line<'static>> = (0..5).map(|i| Line::from(format!("L{i}"))).collect();
        t.insert_before(&lines).unwrap();
        let all: Vec<String> = scrollback(&t).into_iter().chain(rows(&t)).collect();
        assert!(all.iter().any(|r| r == "L0"), "L0 somewhere: {all:?}");
        assert!(all.iter().any(|r| r == "L4"), "L4 somewhere: {all:?}");
    }
}
