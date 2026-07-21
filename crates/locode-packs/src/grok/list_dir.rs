//! `list_dir` — a faithful port of Grok Build's directory tree walker
//! (`gb/list_dir/mod.rs`, Current version), over `locode-host` (Task 26;
//! supersedes the Task 11 flat-DFS simplification).
//!
//! Fidelity notes (see `tasks/audits/list_dir.md`):
//! - Entry collection goes through `Host::walk` (Slice 0a; the `ignore` crate
//!   lives host-side — plan Q1): grok's `standard_filters(true)` + the three
//!   gitignore flags, `RespectGitignore` frozen at its default **true**.
//! - Tree build/budget/render logic (`DirNode`, seed + deep walk with the
//!   depth-1 starvation guard, BFS budget expansion with summary-cost refund,
//!   collapsed `[N files in subtree: …]` summaries, truncation notices) is
//!   ported verbatim from `gb:86-453`.
//! - The truncation notice uses grok's renderer-less fallback copy
//!   (`gb:99-101`) — the faithful rendering for this pack.
//! - `ListDirParams.max_output_chars` is grok host config, not schema — frozen
//!   at the 10 000 default; not exposed (audit Quirks).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use locode_host::{FsError, Host, WalkOptions};
use locode_tools::{Tool, ToolCtx, ToolError, ToolKind, ToolOutput};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Default character budget for directory listing output (`gb:88-90`).
const DEFAULT_MAX_OUTPUT_CHARS: usize = 10_000;
/// Show top-N extension buckets in collapsed-directory summaries (`gb:92`).
const TOP_K_EXTENSIONS: usize = 3;
/// grok's renderer-less root-truncation notice (`gb:99-101`).
const ROOT_TRUNCATION_NOTICE: &str = "    ...\n\n\
    Note: this directory is too large to list fully. Try list_dir on a narrower path, or \
    use grep / bash.";
/// Hard cap on deep-walk (depth ≥ 2) items; depth-1 seed is not counted (`gb:107-109`).
const MAX_GLOBAL_ITEMS: usize = 100_000;
/// Cap on depth-1 seed entries (`gb:110-114`), pinned equal (`gb:115`).
const MAX_SEED_ITEMS: usize = 100_000;
const _: () = assert!(MAX_SEED_ITEMS == MAX_GLOBAL_ITEMS);

/// grok's `list_dir` tool.
pub(crate) struct GrokListDir {
    host: Arc<Host>,
}

impl GrokListDir {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self { host }
    }
}

/// Arguments for `list_dir` (grok's `ListDirInput`, `gb:32-38`).
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ListDirArgs {
    #[serde(rename = "target_directory")]
    #[schemars(
        description = "Path to directory to list contents of, relative to the workspace root or absolute."
    )]
    directory: String,
}

/// The structured (report) face; the rendered tree is the prompt face.
#[derive(Debug, Serialize)]
pub(crate) struct ListDirOutput {
    /// The jail-resolved absolute path listed.
    path: String,
    /// Whether an item cap fired (grok's cutoff-notice condition).
    truncated: bool,
    /// The rendered tree (prompt face only).
    #[serde(skip)]
    body: String,
}

impl ToolOutput for ListDirOutput {
    fn to_prompt_text(&self) -> String {
        self.body.clone()
    }
}

/// Per-subtree file accumulator (`DirAccum`, `gb:118-167`).
#[derive(Debug, Default)]
struct DirAccum {
    total_files: usize,
    by_ext: HashMap<String, usize>,
}

impl DirAccum {
    fn add_ext(&mut self, ext: &str) {
        self.total_files += 1;
        *self.by_ext.entry(ext.to_owned()).or_default() += 1;
    }

    /// Render a summary like `[3 files in subtree: 2 *.rs, 1 *.toml]`.
    fn to_summary(&self, top_n: usize) -> String {
        if self.by_ext.is_empty() {
            return String::new();
        }
        let mut items: Vec<(String, usize)> =
            self.by_ext.iter().map(|(k, v)| (k.clone(), *v)).collect();
        items.sort_by(|a, b| match b.1.cmp(&a.1) {
            std::cmp::Ordering::Equal => a.0.cmp(&b.0),
            other => other,
        });
        let mut parts: Vec<String> = Vec::new();
        let mut top_sum: usize = 0;
        for (ext, count) in items.iter().take(top_n) {
            top_sum += *count;
            if ext == "no-ext" {
                parts.push(format!("{count} *no-ext"));
            } else {
                parts.push(format!("{count} *.{ext}"));
            }
        }
        let ellipsis = if top_sum < self.total_files {
            ", ..."
        } else {
            ""
        };
        let file_word = if self.total_files == 1 {
            "file"
        } else {
            "files"
        };
        format!(
            "[{} {} in subtree: {}{}]",
            self.total_files,
            file_word,
            parts.join(", "),
            ellipsis
        )
    }
}

fn ext_key_from_path(path: &Path) -> String {
    path.extension().map_or("no-ext".to_string(), |s| {
        s.to_string_lossy().to_ascii_lowercase()
    })
}

/// The materialized tree (`DirNode`, `gb:171-281`).
#[derive(Debug)]
struct DirNode {
    depth: usize,
    files: Vec<String>,
    subdirs: Vec<String>,
    children: HashMap<String, DirNode>,
    subtree: DirAccum,
    is_expanded: bool,
}

impl DirNode {
    fn new(depth: usize) -> Self {
        Self {
            depth,
            files: Vec::new(),
            subdirs: Vec::new(),
            children: HashMap::new(),
            subtree: DirAccum::default(),
            is_expanded: false,
        }
    }

    fn add_item(&mut self, rel_parts: &[&str], is_dir: bool) {
        if rel_parts.is_empty() {
            return;
        }
        if rel_parts.len() == 1 {
            let name = rel_parts[0].to_owned();
            if is_dir {
                let key = format!("{name}/");
                if !self.children.contains_key(&key) {
                    self.children
                        .insert(key.clone(), DirNode::new(self.depth + 1));
                    self.subdirs.push(key);
                }
            } else {
                let ext = ext_key_from_path(Path::new(&name));
                self.files.push(name);
                self.subtree.add_ext(&ext);
            }
            return;
        }
        let subdir = rel_parts[0];
        let key = format!("{subdir}/");
        if !self.children.contains_key(&key) {
            self.children
                .insert(key.clone(), DirNode::new(self.depth + 1));
            self.subdirs.push(key.clone());
        }
        if let Some(child) = self.children.get_mut(&key) {
            child.add_item(&rel_parts[1..], is_dir);
        }
        if !is_dir && let Some(last) = rel_parts.last() {
            let ext = ext_key_from_path(Path::new(last));
            self.subtree.add_ext(&ext);
        }
    }

    /// Sort files and subdirs case-insensitively, recursively (`gb:226-231`).
    fn sort_recursive(&mut self) {
        self.files.sort_by_key(|a| a.to_ascii_lowercase());
        self.subdirs.sort_by_key(|a| a.to_ascii_lowercase());
        for child in self.children.values_mut() {
            child.sort_recursive();
        }
    }

    fn all_subitems_sorted(&self) -> Vec<&str> {
        let mut items: Vec<&str> = self
            .files
            .iter()
            .map(String::as_str)
            .chain(self.subdirs.iter().map(String::as_str))
            .collect();
        items.sort_by_key(|a| a.to_ascii_lowercase());
        items
    }

    fn subitem_line(&self, name: &str) -> String {
        let indent = "  ".repeat(self.depth + 1);
        format!("{indent}- {name}")
    }

    fn summary_str(&self, top_k: usize) -> String {
        self.subtree.to_summary(top_k)
    }

    fn summary_char_cost(&self, top_k: usize) -> usize {
        let s = self.summary_str(top_k);
        if s.is_empty() {
            return 0;
        }
        (self.depth + 1) * 2 + s.len() + 1
    }

    /// Render this node's children, recursing into expanded child nodes.
    fn render_expanded(&self, top_k: usize) -> String {
        let mut out = String::new();
        for name in self.all_subitems_sorted() {
            out.push_str(&self.subitem_line(name));
            out.push('\n');
            if let Some(child) = self.children.get(name) {
                out.push_str(&child.render_subtree(top_k));
            }
        }
        out
    }

    /// Render subtree: expanded nodes show children, collapsed show summary.
    fn render_subtree(&self, top_k: usize) -> String {
        if self.is_expanded {
            return self.render_expanded(top_k);
        }
        let summary = self.summary_str(top_k);
        if summary.is_empty() {
            return String::new();
        }
        let indent = "  ".repeat(self.depth + 1);
        format!("{indent}{summary}\n")
    }
}

/// BFS-expand directories within the character budget (`budget_expand`,
/// `gb:364-423`), rendering the body.
fn budget_expand(root: &mut DirNode, max_chars: usize, top_k: usize, truncated: bool) -> String {
    let cutoff_msg = if truncated {
        format!(
            "\nNote: there are more than {MAX_GLOBAL_ITEMS} items in the directory, \
             so not all files may be shown.\n"
        )
    } else {
        String::new()
    };
    if root.files.is_empty() && root.subdirs.is_empty() {
        return cutoff_msg;
    }
    root.is_expanded = true;
    let root_expanded = root.render_expanded(top_k);
    if root_expanded.len() > max_chars {
        let mut out = render_truncated_root(root, max_chars, top_k, ROOT_TRUNCATION_NOTICE);
        out.push_str(&cutoff_msg);
        return out;
    }
    let mut remaining = max_chars - root_expanded.len();
    let mut queue: std::collections::VecDeque<Vec<String>> = std::collections::VecDeque::new();
    for name in &root.subdirs {
        queue.push_back(vec![name.clone()]);
    }
    while let Some(node_path) = queue.pop_front() {
        let Some(node) = navigate_mut(root, &node_path) else {
            continue;
        };
        node.is_expanded = true;
        let expanded = node.render_expanded(top_k);
        let summary_cost = node.summary_char_cost(top_k);
        if expanded.len() > remaining + summary_cost {
            node.is_expanded = false;
            continue;
        }
        remaining += summary_cost;
        remaining -= expanded.len();
        let child_names: Vec<String> = node.subdirs.clone();
        for child_name in child_names {
            let mut child_path = node_path.clone();
            child_path.push(child_name);
            queue.push_back(child_path);
        }
    }
    let mut out = root.render_expanded(top_k);
    out.push_str(&cutoff_msg);
    out
}

fn navigate_mut<'a>(root: &'a mut DirNode, path: &[String]) -> Option<&'a mut DirNode> {
    let mut node = root;
    for key in path {
        node = node.children.get_mut(key)?;
    }
    Some(node)
}

/// Render as many root items as fit, then the truncation notice (`gb:432-453`).
fn render_truncated_root(root: &DirNode, max_chars: usize, top_k: usize, notice: &str) -> String {
    let mut out = String::new();
    let mut remaining = max_chars;
    let child_summary_indent = "  ".repeat(root.depth + 2);
    for name in root.all_subitems_sorted() {
        let mut chunk = root.subitem_line(name);
        chunk.push('\n');
        if let Some(child) = root.children.get(name) {
            let summary = child.summary_str(top_k);
            if !summary.is_empty() {
                use std::fmt::Write as _;
                let _ = writeln!(chunk, "{child_summary_indent}{summary}");
            }
        }
        if chunk.len() > remaining {
            break;
        }
        out.push_str(&chunk);
        remaining -= chunk.len();
    }
    out.push_str(notice);
    out
}

/// grok's display-path normalization (`compute_display_path`, `gb:75-85`).
fn compute_display_path(display_base: &Path, target: &str) -> std::path::PathBuf {
    let t = target.trim().trim_start_matches("./");
    if t.is_empty() || t == "." {
        display_base.to_path_buf()
    } else {
        display_base.join(t)
    }
}

#[async_trait]
impl Tool for GrokListDir {
    type Args = ListDirArgs;
    type Output = ListDirOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Glob
    }

    #[allow(clippy::unnecessary_literal_bound)] // trait ties &str to &self; ours is a literal
    fn description(&self) -> &str {
        // grok's template (`gb:584-592`) with `${{ params.list.target_directory }}`
        // rendered to `target_directory`.
        "Lists files and directories in a given path.\nThe 'target_directory' parameter can be relative to the workspace root or absolute.\n\nOther details:\n    - The result does not display dot-files and dot-directories.\n    - Respects .gitignore patterns (files/directories ignored by git are not shown).\n    - Large directories are summarized with file counts and extension breakdowns instead of listing all files."
    }

    async fn run(&self, ctx: &ToolCtx, args: ListDirArgs) -> Result<Self::Output, ToolError> {
        let target = Path::new(&args.directory);
        let display_path = compute_display_path(&ctx.cwd, &args.directory);
        let resolved = self
            .host
            .resolve_in_jail(&ctx.cwd, target)
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;

        // Target-type probe via the host seam; grok's Current error texts
        // (`gb:501-535`), path hints off (default).
        if let Err(e) = self.host.read_dir(&ctx.cwd, target).await {
            let msg = match &e {
                FsError::Io { source, .. } => match source.kind() {
                    std::io::ErrorKind::NotFound => {
                        format!("Error: {} does not exist.", display_path.display())
                    }
                    std::io::ErrorKind::PermissionDenied => {
                        format!("Permission denied: {}", display_path.display())
                    }
                    std::io::ErrorKind::NotADirectory => {
                        format!(
                            "Error: {} is a file, not a directory.",
                            display_path.display()
                        )
                    }
                    _ => format!(
                        "Error: {} is not a valid directory.",
                        display_path.display()
                    ),
                },
                FsError::Path(_) => e.to_string(),
            };
            return Err(ToolError::Respond(msg));
        }

        // Depth-1 seed (uncounted vs the global cap — the starvation guard,
        // `gb:294-322`), then the deep walk counting only depth ≥ 2
        // (`gb:328-362`). RespectGitignore frozen at grok's default: true.
        let seed = self
            .host
            .walk(
                &ctx.cwd,
                target,
                WalkOptions {
                    respect_gitignore: true,
                    max_depth: Some(1),
                    max_entries: Some(MAX_SEED_ITEMS + 1),
                },
            )
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;
        let seed_truncated = seed.len() > MAX_SEED_ITEMS;
        let mut root_node = DirNode::new(0);
        for entry in seed.iter().take(MAX_SEED_ITEMS) {
            let Some(name) = entry.path.file_name().and_then(|n| n.to_str()) else {
                continue; // non-UTF-8 names are skipped, not lossy (gb:312-314)
            };
            root_node.add_item(&[name], entry.is_dir);
        }

        let deep = self
            .host
            .walk(
                &ctx.cwd,
                target,
                WalkOptions {
                    respect_gitignore: true,
                    max_depth: None,
                    // Fetch bound: uncounted depth-1 (≤ seed cap) + counted
                    // deep items (cap + 1 to detect overflow).
                    max_entries: Some(MAX_SEED_ITEMS + MAX_GLOBAL_ITEMS + 1),
                },
            )
            .await
            .map_err(|e| ToolError::Respond(e.to_string()))?;
        let mut item_count: usize = 0;
        let mut walk_truncated = false;
        for entry in &deep {
            if entry.depth <= 1 {
                continue;
            }
            let Ok(rel) = entry.path.strip_prefix(&resolved) else {
                continue;
            };
            let parts: Vec<&str> = rel.iter().filter_map(|c| c.to_str()).collect();
            if parts.is_empty() {
                continue;
            }
            item_count += 1;
            if item_count > MAX_GLOBAL_ITEMS {
                walk_truncated = true;
                break;
            }
            root_node.add_item(&parts, entry.is_dir);
        }
        root_node.sort_recursive();
        let truncated = seed_truncated || walk_truncated;

        let body = budget_expand(
            &mut root_node,
            DEFAULT_MAX_OUTPUT_CHARS,
            TOP_K_EXTENSIONS,
            truncated,
        );
        let trimmed_body = body.trim_end();
        let output = format!("- {}/\n{}", display_path.display(), trimmed_body);

        Ok(ListDirOutput {
            path: resolved.display().to_string(),
            truncated,
            body: output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // gb's summary format (`gb:127-167`): count-desc/name-asc, no-ext bucket,
    // top-3 + ellipsis, singular "file".
    #[test]
    fn summary_format_matches_grok() {
        let mut acc = DirAccum::default();
        for _ in 0..2 {
            acc.add_ext("rs");
        }
        acc.add_ext("toml");
        assert_eq!(acc.to_summary(3), "[3 files in subtree: 2 *.rs, 1 *.toml]");

        let mut one = DirAccum::default();
        one.add_ext("no-ext");
        assert_eq!(one.to_summary(3), "[1 file in subtree: 1 *no-ext]");

        let mut many = DirAccum::default();
        for ext in ["a", "b", "c", "d"] {
            many.add_ext(ext);
        }
        assert_eq!(
            many.to_summary(3),
            "[4 files in subtree: 1 *.a, 1 *.b, 1 *.c, ...]"
        );
    }

    // gb's display-path cases (`gb:615-644`).
    #[test]
    fn display_path_normalization_matches_grok() {
        let base = Path::new("/ws");
        assert_eq!(compute_display_path(base, "."), Path::new("/ws"));
        assert_eq!(compute_display_path(base, ""), Path::new("/ws"));
        assert_eq!(compute_display_path(base, "  "), Path::new("/ws"));
        assert_eq!(compute_display_path(base, "./foo"), Path::new("/ws/foo"));
        assert_eq!(compute_display_path(base, "foo"), Path::new("/ws/foo"));
    }

    // Merged case-insensitive sort with trailing `/` on dirs (`gb:226-242`):
    // `.` < `/` so `foo.txt` sorts before `foo/`.
    #[test]
    fn merged_sort_matches_grok() {
        let mut node = DirNode::new(0);
        node.add_item(&["Zeta.txt"], false);
        node.add_item(&["alpha"], true);
        node.add_item(&["foo.txt"], false);
        node.add_item(&["foo"], true);
        node.sort_recursive();
        assert_eq!(
            node.all_subitems_sorted(),
            vec!["alpha/", "foo.txt", "foo/", "Zeta.txt"]
        );
    }

    // Cutoff notice text verbatim (`gb:371-379`), constant not actual count.
    #[test]
    fn cutoff_notice_verbatim() {
        let mut node = DirNode::new(0);
        node.add_item(&["a.txt"], false);
        let body = budget_expand(&mut node, 10_000, 3, true);
        assert!(
            body.ends_with(
                "\nNote: there are more than 100000 items in the directory, so not all files may be shown.\n"
            ),
            "{body:?}"
        );
    }

    // Budget expansion: a fat dir collapses to its summary; a later sibling
    // still expands (`gb:834-867` shape).
    #[test]
    fn budget_collapses_fat_dir_expands_sibling() {
        let mut root = DirNode::new(0);
        for i in 0..200 {
            root.add_item(&["fat", &format!("file{i:03}.rs")], false);
        }
        root.add_item(&["thin", "one.txt"], false);
        root.sort_recursive();
        // Budget fits the root lines + thin expansion but not fat's 200 files.
        let body = budget_expand(&mut root, 300, 3, false);
        assert!(body.contains("- fat/\n"), "{body}");
        assert!(
            body.contains("[200 files in subtree: 200 *.rs]"),
            "fat collapsed: {body}"
        );
        assert!(body.contains("- one.txt"), "thin expanded: {body}");
    }

    // Root over budget: as many root lines as fit + the fallback notice.
    #[test]
    fn root_over_budget_uses_fallback_notice() {
        let mut root = DirNode::new(0);
        for i in 0..100 {
            root.add_item(&[format!("file-{i:04}.txt").as_str()], false);
        }
        root.sort_recursive();
        let body = budget_expand(&mut root, 100, 3, false);
        assert!(body.ends_with(ROOT_TRUNCATION_NOTICE), "{body:?}");
        assert!(body.starts_with("  - file-0000.txt\n"), "{body:?}");
    }
}
