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
