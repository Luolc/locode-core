//! Tests for codex `apply_patch`: parser / seek / apply units + the tool over a
//! tempdir host, plus provenance pins.

use super::*;
use locode_host::HostConfig;
use locode_tools::ToolKind;
use std::path::Path;
use tokio_util::sync::CancellationToken;

// ---- helpers ----

fn host() -> (tempfile::TempDir, Arc<Host>, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(Host::new(HostConfig::new(dir.path())).unwrap());
    let root = host.workspace_root().to_path_buf();
    (dir, host, root)
}

fn tctx(root: &Path) -> ToolCtx {
    ToolCtx::new(
        root.to_path_buf(),
        "c1".into(),
        root.to_path_buf(),
        CancellationToken::new(),
    )
}

fn wrap(body: &str) -> String {
    format!("*** Begin Patch\n{body}\n*** End Patch")
}

async fn run(host: &Arc<Host>, root: &Path, patch: &str) -> Result<String, ToolError> {
    let tool = CodexApplyPatch::new(Arc::clone(host));
    tool.run(&tctx(root), patch.to_string())
        .await
        .map(|o| o.summary)
}

// ---- parser ----

#[test]
fn parses_add_delete_update_hunks() {
    let hunks = parser::parse(&wrap(
        "*** Add File: a.txt\n+hello\n*** Delete File: gone.txt\n*** Update File: b.txt\n@@\n-old\n+new",
    ))
    .expect("parses");
    assert_eq!(hunks.len(), 3);
    assert!(matches!(&hunks[0], Hunk::AddFile { contents, .. } if contents == "hello\n"));
    assert!(matches!(&hunks[1], Hunk::DeleteFile { .. }));
    assert!(matches!(&hunks[2], Hunk::UpdateFile { chunks, .. } if chunks.len() == 1));
}

#[test]
fn parse_rejects_bad_first_line() {
    let err = parser::parse("nope\n*** End Patch").expect_err("bad first line");
    assert_eq!(
        format_parse_error(&err),
        "Invalid patch: The first line of the patch must be '*** Begin Patch'"
    );
}

#[test]
fn parse_rejects_missing_end_patch() {
    let err = parser::parse("*** Begin Patch\n*** Add File: a.txt\n+x").expect_err("no end");
    assert_eq!(
        format_parse_error(&err),
        "Invalid patch: The last line of the patch must be '*** End Patch'"
    );
}

#[test]
fn parse_rejects_bad_hunk_header() {
    let err = parser::parse(&wrap("*** Frobnicate File: a.txt")).expect_err("bad header");
    assert!(format_parse_error(&err).starts_with("Invalid patch hunk on line"));
}

#[test]
fn parse_unwraps_heredoc() {
    // The GPT-4.1 local_shell path wraps the patch in a heredoc.
    let hunks = parser::parse("<<'EOF'\n*** Begin Patch\n*** Delete File: x\n*** End Patch\nEOF")
        .expect("heredoc unwrapped");
    assert_eq!(hunks.len(), 1);
}

// ---- seek ----

#[test]
fn seek_exact_rstrip_trim_and_unicode() {
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
    // exact
    assert_eq!(
        seek::seek_sequence(&s(&["a", "b", "c"]), &s(&["b", "c"]), 0, false),
        Some(1)
    );
    // trailing whitespace
    assert_eq!(
        seek::seek_sequence(&s(&["foo   ", "bar\t"]), &s(&["foo", "bar"]), 0, false),
        Some(0)
    );
    // leading+trailing whitespace
    assert_eq!(
        seek::seek_sequence(&s(&["  foo  "]), &s(&["foo"]), 0, false),
        Some(0)
    );
    // unicode dash fold (pattern uses ASCII '-', file uses an em dash)
    assert_eq!(
        seek::seek_sequence(&s(&["a\u{2014}b"]), &s(&["a-b"]), 0, false),
        Some(0)
    );
    // pattern longer than input
    assert_eq!(
        seek::seek_sequence(&s(&["only"]), &s(&["a", "b"]), 0, false),
        None
    );
}

// ---- apply (unit) ----

#[test]
fn derive_replaces_and_normalizes_trailing_newline() {
    let hunks = parser::parse(&wrap("*** Update File: f\n@@\n one\n-two\n+TWO\n three")).unwrap();
    let Hunk::UpdateFile { chunks, .. } = &hunks[0] else {
        panic!("update");
    };
    let out = apply::derive_new_contents("one\ntwo\nthree\n", "f", chunks).unwrap();
    assert_eq!(out, "one\nTWO\nthree\n");
}

// ---- tool over a host ----

#[tokio::test]
async fn add_file_creates_it() {
    let (_d, host, root) = host();
    let summary = run(&host, &root, &wrap("*** Add File: new.txt\n+line1\n+line2"))
        .await
        .unwrap();
    assert_eq!(
        summary,
        "Success. Updated the following files:\nA new.txt\n"
    );
    let read = host.read_file(&root, Path::new("new.txt")).await.unwrap();
    assert_eq!(read.contents, "line1\nline2\n");
}

#[tokio::test]
async fn add_file_makes_parent_dirs() {
    let (_d, host, root) = host();
    run(&host, &root, &wrap("*** Add File: nested/deep/x.txt\n+hi"))
        .await
        .unwrap();
    let read = host
        .read_file(&root, Path::new("nested/deep/x.txt"))
        .await
        .unwrap();
    assert_eq!(read.contents, "hi\n");
}

#[tokio::test]
async fn update_file_replaces_region() {
    let (_d, host, root) = host();
    host.write_file(&root, Path::new("f.txt"), "alpha\nbeta\ngamma\n")
        .await
        .unwrap();
    let summary = run(
        &host,
        &root,
        &wrap("*** Update File: f.txt\n@@\n alpha\n-beta\n+BETA\n gamma"),
    )
    .await
    .unwrap();
    assert_eq!(summary, "Success. Updated the following files:\nM f.txt\n");
    let read = host.read_file(&root, Path::new("f.txt")).await.unwrap();
    assert_eq!(read.contents, "alpha\nBETA\ngamma\n");
}

#[tokio::test]
async fn update_at_eof_uses_trailing_empty_retry() {
    let (_d, host, root) = host();
    host.write_file(&root, Path::new("f.txt"), "keep\ndrop\n")
        .await
        .unwrap();
    run(
        &host,
        &root,
        &wrap("*** Update File: f.txt\n@@\n keep\n-drop\n+DONE\n*** End of File"),
    )
    .await
    .unwrap();
    let read = host.read_file(&root, Path::new("f.txt")).await.unwrap();
    assert_eq!(read.contents, "keep\nDONE\n");
}

#[tokio::test]
async fn delete_file_removes_it() {
    let (_d, host, root) = host();
    host.write_file(&root, Path::new("gone.txt"), "bye")
        .await
        .unwrap();
    let summary = run(&host, &root, &wrap("*** Delete File: gone.txt"))
        .await
        .unwrap();
    assert_eq!(
        summary,
        "Success. Updated the following files:\nD gone.txt\n"
    );
    assert!(host.read_file(&root, Path::new("gone.txt")).await.is_err());
}

#[tokio::test]
async fn move_writes_dest_and_removes_source() {
    let (_d, host, root) = host();
    host.write_file(&root, Path::new("src.txt"), "one\ntwo\n")
        .await
        .unwrap();
    let summary = run(
        &host,
        &root,
        &wrap("*** Update File: src.txt\n*** Move to: dst.txt\n@@\n one\n-two\n+TWO"),
    )
    .await
    .unwrap();
    // A moved file is summarized under M by its original path.
    assert_eq!(
        summary,
        "Success. Updated the following files:\nM src.txt\n"
    );
    assert!(host.read_file(&root, Path::new("src.txt")).await.is_err());
    let read = host.read_file(&root, Path::new("dst.txt")).await.unwrap();
    assert_eq!(read.contents, "one\nTWO\n");
}

#[tokio::test]
async fn context_not_found_is_soft_error() {
    let (_d, host, root) = host();
    host.write_file(&root, Path::new("f.txt"), "a\nb\n")
        .await
        .unwrap();
    let err = run(
        &host,
        &root,
        &wrap("*** Update File: f.txt\n@@ nowhere\n a\n-b\n+B"),
    )
    .await
    .expect_err("context missing");
    assert!(
        matches!(&err, ToolError::Respond(m) if m == "Failed to find context 'nowhere' in f.txt"),
        "{err:?}"
    );
}

#[tokio::test]
async fn lines_not_found_is_soft_error() {
    let (_d, host, root) = host();
    host.write_file(&root, Path::new("f.txt"), "a\nb\n")
        .await
        .unwrap();
    let err = run(&host, &root, &wrap("*** Update File: f.txt\n@@\n-zzz\n+B"))
        .await
        .expect_err("lines missing");
    assert!(
        matches!(&err, ToolError::Respond(m) if m.starts_with("Failed to find expected lines in f.txt:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn empty_patch_reports_no_files_modified() {
    let (_d, host, root) = host();
    let err = run(&host, &root, "*** Begin Patch\n*** End Patch")
        .await
        .expect_err("empty");
    assert!(matches!(&err, ToolError::Respond(m) if m == "No files were modified."));
}

#[tokio::test]
async fn summary_orders_added_modified_deleted() {
    let (_d, host, root) = host();
    host.write_file(&root, Path::new("upd.txt"), "x\n")
        .await
        .unwrap();
    host.write_file(&root, Path::new("del.txt"), "y")
        .await
        .unwrap();
    let summary = run(
        &host,
        &root,
        &wrap("*** Add File: add.txt\n+z\n*** Update File: upd.txt\n@@\n-x\n+X\n*** Delete File: del.txt"),
    )
    .await
    .unwrap();
    assert_eq!(
        summary,
        "Success. Updated the following files:\nA add.txt\nM upd.txt\nD del.txt\n"
    );
}

// ---- tool surface + provenance pins ----

#[test]
fn tool_surface_is_freeform_lark_edit() {
    let (_d, host, _root) = host();
    let tool = CodexApplyPatch::new(host);
    assert_eq!(tool.kind(), ToolKind::Edit);
    assert!(matches!(
        tool.input_format(),
        ToolInputFormat::Freeform {
            syntax: GrammarSyntax::Lark,
            ..
        }
    ));
}

#[test]
fn description_is_the_pinned_apply_patch_md() {
    // sha256 1d2e0992b289d9389b4e765287dd932db0b2937cb4abfe422d6323c717293ebe.
    assert_eq!(DESCRIPTION.len(), 108, "apply_patch.md byte length changed");
    assert_eq!(
        DESCRIPTION,
        "The `apply_patch` tool can be used to edit files. This is a FREEFORM tool, so do not wrap the patch in JSON."
    );
}

#[test]
fn lark_grammar_is_pinned() {
    // sha256 d6367f4826ed608c424b0a308f3d6163527df63c22513d089b91863552f8bfeb.
    assert_eq!(
        APPLY_PATCH_LARK.len(),
        578,
        "apply_patch.lark byte length changed"
    );
    assert!(APPLY_PATCH_LARK.starts_with("start: begin_patch hunk+ end_patch"));
    assert!(APPLY_PATCH_LARK.contains("%import common.LF"));
}
