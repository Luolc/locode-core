//! A minimal **vendored terminal** for the dynamic bottom-pinned composer
//! (ADR-0022). It owns a full-screen double buffer and renders **one relative
//! frame** each paint — Claude Code's `log-update` model, expressed with
//! ratatui's public `Buffer`/`Backend` so it stays unit-testable against
//! `TestBackend` (which emulates `scroll_region_up` + a scrollback buffer).
//!
//! The model (see ADR-0022 for the full rationale and the four-harness study):
//!
//! - The **frame** = `[recent transcript tail] + [status] + [composer]`, painted
//!   bottom-anchored into a screen-sized buffer. Its bottom edge is the screen
//!   bottom, so the composer + status are pinned there — no reserved viewport
//!   rect whose `y` we'd have to move (codex's model, which forces the
//!   scrollback-corrupting reverse scroll on shrink).
//! - **Grow / commit to scrollback** ([`scroll_up`](Self::scroll_up)): when new
//!   transcript (streaming) or a taller composer pushes content past the top of
//!   the screen, `scroll_region_up` pushes those top rows into native scrollback
//!   — a clean, one-directional operation. The diff baseline shifts with it.
//! - **Shrink** is just a re-render: the frame is shorter, the diff clears the
//!   rows it vacated. We **never** `scroll_region_down` (it injects blanks into
//!   scrollback — the bug that killed the earlier attempts).
//! - The **scroll-back-below-viewport edge** (shrinking when committed rows would
//!   need to come back on screen) is handled by [`clear`](Self::clear) +
//!   full repaint — Claude Code's `fullReset`. Rare; correct if briefly flickery.
//!
//! Everything for a frame is one buffer diff (no separate `insert_before` path
//! interleaving with the diff — the streaming/insert collision the tail-in-
//! viewport attempt hit). The caller decides the `scroll_up` amount and when a
//! full reset is needed; this type is the mechanism, not the policy.

use std::io;

use ratatui::backend::Backend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::widgets::Widget;

/// Per-frame rendering handle over the full screen. Mirrors the slice of
/// `ratatui::Frame` the UI uses so wiring is mechanical.
pub struct Frame<'a> {
    area: Rect,
    buffer: &'a mut Buffer,
    cursor: Option<Position>,
}

impl Frame<'_> {
    /// The full-screen area for this frame (`0,0 .. width,height`).
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

/// A vendored terminal that renders one bottom-anchored relative frame per paint.
pub struct FrameTerminal<B: Backend> {
    backend: B,
    /// What is currently on screen (the diff baseline), full-screen sized.
    screen: Buffer,
    size: Size,
    cursor_hidden: bool,
}

impl<B: Backend> FrameTerminal<B> {
    /// Build the terminal, taking the current screen as blank (a fresh app
    /// starts by drawing over whatever the shell left; the first `draw` diffs
    /// against blank and paints only non-blank cells).
    ///
    /// # Errors
    /// Propagates the backend size query.
    pub fn new(backend: B) -> io::Result<Self> {
        let size = backend.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        Ok(Self {
            backend,
            screen: Buffer::empty(area),
            size,
            cursor_hidden: false,
        })
    }

    /// The terminal size (queried fresh).
    ///
    /// # Errors
    /// Propagates the backend size query.
    pub fn size(&self) -> io::Result<Size> {
        self.backend.size()
    }

    /// Current screen height (rows).
    #[must_use]
    pub fn height(&self) -> u16 {
        self.size.height
    }

    /// Commit the top `n` rows to native scrollback: `scroll_region_up` the whole
    /// screen by `n` (pushing the top rows out of view, into scrollback on a real
    /// terminal and into `TestBackend`'s scrollback buffer), then shift the diff
    /// baseline up by the same `n` so the next `draw` only repaints what actually
    /// changed at the bottom. This is the one-directional, scrollback-safe way
    /// transcript leaves the frame.
    ///
    /// # Errors
    /// Propagates the backend scroll failure.
    pub fn scroll_up(&mut self, n: u16) -> io::Result<()> {
        let v = self.size.height;
        let n = n.min(v);
        if n == 0 {
            return Ok(());
        }
        self.backend.scroll_region_up(0..v, n)?;
        shift_buffer_up(&mut self.screen, n);
        Ok(())
    }

    /// Commit `lines` to native scrollback (the oldest transcript scrolling off
    /// the top of the frame). Draws the lines into the top rows and immediately
    /// `scroll_region_up`s them out of view, so the **correct** lines land in
    /// scrollback regardless of what the frame currently shows there (unlike a
    /// bare [`scroll_up`](Self::scroll_up), which commits whatever rows happen to
    /// be on screen — wrong when the frame wasn't full, e.g. the first big
    /// response). Chunked by screen height so a burst taller than the screen
    /// still commits every line. The diff baseline is shifted to match, so the
    /// next [`draw`](Self::draw) repaints only real changes.
    ///
    /// # Errors
    /// Propagates backend draw/scroll/flush failures.
    pub fn commit_scrollback(&mut self, lines: &[ratatui::text::Line<'_>]) -> io::Result<()> {
        let v = self.size.height;
        let w = self.size.width;
        if v == 0 || w == 0 || lines.is_empty() {
            return Ok(());
        }
        let mut i = 0usize;
        while i < lines.len() {
            let end = (i + usize::from(v)).min(lines.len());
            let chunk = &lines[i..end];
            let n = u16::try_from(chunk.len()).unwrap_or(v).min(v);
            let area = Rect::new(0, 0, w, n);
            let mut buf = Buffer::empty(area);
            ratatui::widgets::Paragraph::new(chunk.to_vec()).render(area, &mut buf);
            let updates: Vec<(u16, u16, Cell)> = Buffer::empty(area)
                .diff(&buf)
                .into_iter()
                .map(|(x, y, cell)| (x, y, cell.clone()))
                .collect();
            self.backend
                .draw(updates.iter().map(|(x, y, cell)| (*x, *y, cell)))?;
            self.backend.scroll_region_up(0..v, n)?;
            shift_buffer_up(&mut self.screen, n);
            i = end;
        }
        // Deliberately NOT flushed: the caller always follows with `draw`, so the
        // commit-then-repaint is one atomic flush (no intermediate frame where the
        // frame content is scrolled up with a blank bottom → no flicker).
        Ok(())
    }

    /// Render one frame: build the desired full-screen buffer, diff it against
    /// what's on screen, and draw only the delta. The caller paints the frame
    /// content bottom-anchored (blank rows above read as margin / are where
    /// committed scrollback sits).
    ///
    /// # Errors
    /// Propagates backend draw/flush failures.
    pub fn draw<F: FnOnce(&mut Frame<'_>)>(&mut self, render: F) -> io::Result<()> {
        self.autoresize()?;
        let area = Rect::new(0, 0, self.size.width, self.size.height);
        let mut next = Buffer::empty(area);
        let cursor = {
            let mut frame = Frame {
                area,
                buffer: &mut next,
                cursor: None,
            };
            render(&mut frame);
            frame.cursor
        };

        let updates: Vec<(u16, u16, Cell)> = self
            .screen
            .diff(&next)
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
        self.screen = next;
        Ok(())
    }

    /// Full reset: clear the visible screen and drop the diff baseline so the
    /// next `draw` repaints everything (Claude Code's `fullReset`, used for the
    /// shrink-below-committed-scrollback edge and after a terminal resize).
    ///
    /// # Errors
    /// Propagates the backend clear failure.
    pub fn clear(&mut self) -> io::Result<()> {
        self.backend.clear()?;
        let area = Rect::new(0, 0, self.size.width, self.size.height);
        self.screen = Buffer::empty(area);
        Ok(())
    }

    /// Access the backend (teardown flushes / test inspection).
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Access the backend immutably (test inspection).
    #[must_use]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    fn autoresize(&mut self) -> io::Result<()> {
        let size = self.backend.size()?;
        if size != self.size {
            self.size = size;
            let area = Rect::new(0, 0, size.width, size.height);
            // The old baseline is meaningless at the new size; force a full
            // repaint against blank (callers pair a resize with a full redraw).
            self.screen = Buffer::empty(area);
        }
        Ok(())
    }
}

/// Shift a full-screen buffer's rows up by `n`, blanking the freed bottom rows —
/// the in-memory mirror of a `scroll_region_up` so the diff baseline stays true.
fn shift_buffer_up(buf: &mut Buffer, n: u16) {
    let w = buf.area.width;
    let h = buf.area.height;
    let n = n.min(h);
    if n == 0 || w == 0 {
        return;
    }
    for y in 0..h {
        for x in 0..w {
            let cell = if y + n < h {
                buf[(x, y + n)].clone()
            } else {
                Cell::default()
            };
            buf[(x, y)] = cell;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::text::Line;
    use ratatui::widgets::Paragraph;

    fn term(w: u16, h: u16) -> FrameTerminal<TestBackend> {
        FrameTerminal::new(TestBackend::new(w, h)).unwrap()
    }

    fn rows(t: &FrameTerminal<TestBackend>) -> Vec<String> {
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

    fn scrollback(t: &FrameTerminal<TestBackend>) -> Vec<String> {
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

    /// The frame paints bottom-anchored: a short frame sits on the last rows,
    /// blank above.
    #[test]
    fn frame_paints_bottom_anchored() {
        let mut t = term(20, 8);
        t.draw(|f| {
            // Two lines at the bottom (rows 6,7).
            f.render_widget(Paragraph::new("line-a"), Rect::new(0, 6, 20, 1));
            f.render_widget(Paragraph::new("line-b"), Rect::new(0, 7, 20, 1));
        })
        .unwrap();
        let r = rows(&t);
        assert_eq!(r[6], "line-a");
        assert_eq!(r[7], "line-b");
        assert!(r[0..6].iter().all(String::is_empty), "blank above: {r:?}");
    }

    /// Growing the frame in place (no scroll) just repaints — content that was
    /// lower moves up, nothing leaves the screen.
    #[test]
    fn grow_in_place_repaints_without_scroll() {
        let mut t = term(20, 8);
        t.draw(|f| f.render_widget(Paragraph::new("only"), Rect::new(0, 7, 20, 1)))
            .unwrap();
        assert_eq!(rows(&t)[7], "only");
        // Now a 3-tall frame at the bottom.
        t.draw(|f| {
            for (i, s) in ["x", "y", "z"].iter().enumerate() {
                let i = u16::try_from(i).unwrap();
                f.render_widget(Paragraph::new(*s), Rect::new(0, 5 + i, 20, 1));
            }
        })
        .unwrap();
        let r = rows(&t);
        assert_eq!(&r[5..8], &["x", "y", "z"]);
        assert!(
            scrollback(&t).iter().all(String::is_empty),
            "nothing committed"
        );
    }

    /// `scroll_up(n)` commits the top `n` rows to scrollback and shifts the
    /// baseline, so the next draw only repaints the freed bottom rows.
    #[test]
    fn scroll_up_commits_top_rows_to_scrollback() {
        let mut t = term(20, 4);
        // Fill the screen with t0..t3.
        t.draw(|f| {
            for i in 0..4u16 {
                f.render_widget(Paragraph::new(format!("t{i}")), Rect::new(0, i, 20, 1));
            }
        })
        .unwrap();
        assert_eq!(rows(&t), vec!["t0", "t1", "t2", "t3"]);
        // Commit the top 2 rows, then paint two fresh lines at the bottom.
        t.scroll_up(2).unwrap();
        t.draw(|f| {
            f.render_widget(Paragraph::new("t2"), Rect::new(0, 0, 20, 1));
            f.render_widget(Paragraph::new("t3"), Rect::new(0, 1, 20, 1));
            f.render_widget(Paragraph::new("t4"), Rect::new(0, 2, 20, 1));
            f.render_widget(Paragraph::new("t5"), Rect::new(0, 3, 20, 1));
        })
        .unwrap();
        assert_eq!(
            rows(&t),
            vec!["t2", "t3", "t4", "t5"],
            "screen scrolled up 2"
        );
        let sb = scrollback(&t);
        assert!(sb.iter().any(|r| r == "t0"), "t0 in scrollback: {sb:?}");
        assert!(sb.iter().any(|r| r == "t1"), "t1 in scrollback: {sb:?}");
    }

    /// Shrinking the frame re-renders and clears the vacated rows — no
    /// `scroll_region_down`, so scrollback is never touched.
    #[test]
    fn shrink_clears_vacated_rows_without_touching_scrollback() {
        let mut t = term(20, 8);
        t.draw(|f| {
            for (i, s) in ["a", "b", "c", "d"].iter().enumerate() {
                let i = u16::try_from(i).unwrap();
                f.render_widget(Paragraph::new(*s), Rect::new(0, 4 + i, 20, 1));
            }
        })
        .unwrap();
        assert_eq!(&rows(&t)[4..8], &["a", "b", "c", "d"]);
        // Shrink to a 2-tall frame at the bottom.
        t.draw(|f| {
            f.render_widget(Paragraph::new("c"), Rect::new(0, 6, 20, 1));
            f.render_widget(Paragraph::new("d"), Rect::new(0, 7, 20, 1));
        })
        .unwrap();
        let r = rows(&t);
        assert_eq!(&r[6..8], &["c", "d"]);
        assert!(
            r[0..6].iter().all(String::is_empty),
            "vacated rows blank: {r:?}"
        );
        assert!(
            scrollback(&t).iter().all(String::is_empty),
            "scrollback untouched"
        );
    }

    /// `clear` drops the baseline so the next frame fully repaints.
    #[test]
    fn clear_forces_full_repaint() {
        let mut t = term(20, 4);
        t.draw(|f| f.render_widget(Paragraph::new("keep"), Rect::new(0, 3, 20, 1)))
            .unwrap();
        t.clear().unwrap();
        t.draw(|f| f.render_widget(Paragraph::new("fresh"), Rect::new(0, 3, 20, 1)))
            .unwrap();
        assert_eq!(rows(&t)[3], "fresh");
    }

    #[test]
    fn shift_buffer_up_moves_rows_and_blanks_bottom() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 3));
        Line::from("aa").render(Rect::new(0, 0, 4, 1), &mut buf);
        Line::from("bb").render(Rect::new(0, 1, 4, 1), &mut buf);
        Line::from("cc").render(Rect::new(0, 2, 4, 1), &mut buf);
        shift_buffer_up(&mut buf, 1);
        let row = |y: u16| (0..4).map(|x| buf[(x, y)].symbol()).collect::<String>();
        assert_eq!(row(0).trim_end(), "bb");
        assert_eq!(row(1).trim_end(), "cc");
        assert_eq!(row(2).trim_end(), "", "bottom blanked");
    }
}
