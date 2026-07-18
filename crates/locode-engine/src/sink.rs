//! Where the loop's `stream-json` events go (ADR-0014).

use locode_protocol::Event;

/// A sink for the loop's [`Event`]s. `&mut self` so a writer sink can buffer/flush;
/// object-safe so `locode-exec` can pick one at runtime by `--output-format`.
pub trait EventSink: Send {
    /// Receive one event (called in emission order).
    fn emit(&mut self, event: Event);
}

/// Drops every event — for the `json`/`text` output modes that only want the report.
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&mut self, _event: Event) {}
}

/// Forwards each event to a closure — e.g. a JSONL stdout writer for `stream-json`,
/// or a `Vec`-pushing closure in tests.
pub struct FnSink<F: FnMut(Event) + Send>(pub F);

impl<F: FnMut(Event) + Send> EventSink for FnSink<F> {
    fn emit(&mut self, event: Event) {
        (self.0)(event);
    }
}
