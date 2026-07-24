//! `ClaudeSessionState` — Claude Code's per-session `readFileState`
//! (`FileReadTool.ts:540-570,1032`): a path → last-read snapshot, consulted by
//! `Read` (the `file_unchanged` dedup) and, from Slice 3, by `Edit`/`Write` (the
//! read-before-edit + modified-since-read gate — CC's signature guardrail, the
//! deliberate behavioral divergence from the grok pack).
//!
//! Constructed once per `register()` (per run), matching CC's per-session store.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// One recorded observation of a file (CC's `readFileState` entry).
#[derive(Debug, Clone)]
struct ReadRecord {
    /// mtime at the observation, floored to whole milliseconds (CC's
    /// `Math.floor(mtimeMs)`); `None` if the platform doesn't report an mtime.
    mtime_ms: Option<u64>,
    /// The `Read` window that produced this entry; `None` for `Edit`/`Write`
    /// entries (CC stores `offset=undefined` for those, so they never
    /// dedup-match — the seam the freshness gate relies on).
    offset: Option<u64>,
    /// The `Read` `limit` at the observation (paired with `offset`).
    limit: Option<u64>,
}

/// CC's `Math.floor(mtimeMs)` — a `SystemTime` as whole milliseconds since the
/// Unix epoch. `None` for pre-epoch / unrepresentable times.
fn to_millis(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// The per-run read-freshness store.
#[derive(Debug, Default)]
pub(crate) struct ClaudeSessionState {
    entries: Mutex<HashMap<PathBuf, ReadRecord>>,
}

impl ClaudeSessionState {
    /// Record a successful `Read` of `path`: its window (`offset`/`limit`) and
    /// the observed mtime (CC's `readFileState.set` with `offset` set).
    pub(crate) fn record_read(
        &self,
        path: PathBuf,
        modified: Option<SystemTime>,
        offset: Option<u64>,
        limit: Option<u64>,
    ) {
        let record = ReadRecord {
            mtime_ms: modified.and_then(to_millis),
            offset,
            limit,
        };
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path, record);
    }

    /// Record a successful `Edit`/`Write` of `path` at `modified` (CC's
    /// `readFileState.set` with `offset=undefined`, `FileEditTool.ts:520`,
    /// `FileWriteTool`): the file now counts as "read" for the gate, and its
    /// post-write mtime invalidates nothing (a follow-up edit is fresh). The
    /// `offset=None` marks it Edit/Write-origin so `Read`'s dedup never matches it.
    pub(crate) fn record_write(&self, path: PathBuf, modified: Option<SystemTime>) {
        let record = ReadRecord {
            mtime_ms: modified.and_then(to_millis),
            offset: None,
            limit: None,
        };
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(path, record);
    }

    /// CC's read-before-edit / staleness gate (`FileEditTool.ts:275-310`):
    /// `None` = never read (errorCode 6 — "read it first"); `Some(false)` = read
    /// but modified since (errorCode 7 — mtime advanced past the recorded read);
    /// `Some(true)` = fresh (safe to edit). An unknown mtime on either side can't
    /// prove staleness, so it reads as fresh (never a false "modified" block).
    pub(crate) fn check_fresh(&self, path: &Path, current: Option<SystemTime>) -> Option<bool> {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let record = entries.get(path)?;
        match (record.mtime_ms, current.and_then(to_millis)) {
            (Some(stored), Some(now)) => Some(now <= stored),
            _ => Some(true),
        }
    }

    /// CC's dedup test (`FileReadTool.ts:547-558`): `true` iff `path` was already
    /// read from a `Read` (offset set) over the *same* `offset`/`limit` window and
    /// its mtime is unchanged since — the caller then returns `FILE_UNCHANGED_STUB`
    /// instead of re-sending the content. A missing/unknown mtime never matches.
    pub(crate) fn is_unchanged_read(
        &self,
        path: &Path,
        modified: Option<SystemTime>,
        offset: Option<u64>,
        limit: Option<u64>,
    ) -> bool {
        let Some(now_ms) = modified.and_then(to_millis) else {
            return false;
        };
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(record) = entries.get(path) else {
            return false;
        };
        record.offset.is_some()
            && record.offset == offset
            && record.limit == limit
            && record.mtime_ms == Some(now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    #[test]
    fn unchanged_read_matches_same_window_and_mtime() {
        let state = ClaudeSessionState::default();
        let p = PathBuf::from("/f.txt");
        state.record_read(p.clone(), Some(t(1000)), Some(1), None);
        // Same window + same mtime → unchanged (dedup).
        assert!(state.is_unchanged_read(&p, Some(t(1000)), Some(1), None));
        // Different mtime → changed.
        assert!(!state.is_unchanged_read(&p, Some(t(2000)), Some(1), None));
        // Different window → not a dedup hit.
        assert!(!state.is_unchanged_read(&p, Some(t(1000)), Some(5), None));
        // Unknown mtime → never matches.
        assert!(!state.is_unchanged_read(&p, None, Some(1), None));
        // Never read → no match.
        assert!(!state.is_unchanged_read(Path::new("/other"), Some(t(1000)), Some(1), None));
    }

    #[test]
    fn check_fresh_gate_transitions() {
        let state = ClaudeSessionState::default();
        let p = PathBuf::from("/f.txt");
        // Never read → None (errorCode 6).
        assert_eq!(state.check_fresh(&p, Some(t(1000))), None);
        // Read at 1000, unchanged → fresh.
        state.record_read(p.clone(), Some(t(1000)), Some(1), None);
        assert_eq!(state.check_fresh(&p, Some(t(1000))), Some(true));
        // Modified since (mtime advanced) → stale (errorCode 7).
        assert_eq!(state.check_fresh(&p, Some(t(2000))), Some(false));
        // An Edit records post-write mtime → fresh again for sequential edits.
        state.record_write(p.clone(), Some(t(2000)));
        assert_eq!(state.check_fresh(&p, Some(t(2000))), Some(true));
        // A write-origin entry counts as "read" (not None).
        assert_ne!(state.check_fresh(&p, Some(t(2000))), None);
    }

    #[test]
    fn write_origin_entry_never_dedup_matches_a_read() {
        let state = ClaudeSessionState::default();
        let p = PathBuf::from("/f.txt");
        // offset=None (Edit/Write) must not satisfy Read's dedup.
        state.record_write(p.clone(), Some(t(1000)));
        assert!(!state.is_unchanged_read(&p, Some(t(1000)), Some(1), None));
    }

    #[test]
    fn sub_millisecond_change_is_flattened_like_cc() {
        let state = ClaudeSessionState::default();
        let p = PathBuf::from("/f.txt");
        state.record_read(p.clone(), Some(t(1000)), Some(1), None);
        // 1000.4ms and 1000.9ms both floor to 1000 — CC's ms granularity.
        assert!(state.is_unchanged_read(
            &p,
            Some(UNIX_EPOCH + Duration::from_micros(1_000_900)),
            Some(1),
            None
        ));
    }
}
