# Task 1 — Cargo workspace + crate skeletons + toolchain/lint baseline

**Retrospective, source-grounded plan.** Task 1 is already implemented and merged; this
is the design doc we skipped, written *as if* planned up front but honest about the
as-built reality. Source of truth: `SPEC.md` (Project Structure + dependency direction),
ADR-0002 (multi-crate workspace), ADR-0010 (Rust tooling baseline + strict-from-empty
amendment), `tasks/todo.md` Task 1. Every non-obvious decision is grounded against the two
studied Rust harnesses with `file:line`/path citations. **No code was modified to write
this.**

Submodule roots (abbreviated in citations):
- `grok` = `~/dev/coding-cli-survey/submodules/grok-build`
- `codex` = `~/dev/coding-cli-survey/submodules/codex/codex-rs`
- survey = `~/dev/coding-cli-survey/survey`
- design = `survey/06-design-lessons/minimal-headless-rust-agent.md`
- tooling = `survey/06-design-lessons/rust-ci-and-tooling.md`

---

## 1. Purpose & scope

Stand up the **compilable foundation**: a Cargo workspace of eight `locode-*` crates with
the dependency graph from `SPEC.md` wired acyclically, a pinned toolchain, and the
format/lint configuration that every later slice lands green against. This is Phase 0
Task 1 (`tasks/plan.md:42`) — the "empty workspace compiles" half of Checkpoint A
(`tasks/plan.md:45`). Nothing here has runtime behavior; the deliverable is *structure the
compiler enforces* so the architectural seams (ADR-0002) exist before any code fills them.

### In scope (Task 1)
- Root `Cargo.toml`: `[workspace]` (`resolver = "2"`, `members = ["crates/*"]`),
  `[workspace.package]` (shared `version`/`edition`/`rust-version`/`publish`),
  `[workspace.dependencies]` (centralized versions), `[workspace.lints]`.
- Eight crate skeletons under `crates/` — seven libs + one bin (`locode-exec`) — each
  compiling empty, each dependency edge from the SPEC graph wired.
- `rust-toolchain.toml` (pinned stable channel + `rustfmt`, `clippy`).
- `rustfmt.toml` (near-default) and `clippy.toml` (test allow-lists + `doc-valid-idents`).
- `[workspace.lints]` — the strict-from-empty `rust` + `clippy` tables (see §5 for the
  honest Task-1-vs-Task-2 timeline).
- `Cargo.lock` committed (this repo ships a binary — ADR-0010 `:20`).

### Out of scope / deferred
- **CI (`.github/workflows/ci.yml`) and the `justfile`** — Task 2 (`tasks/todo.md:25`).
  Referenced here only where the lint config assumes a `-D warnings` gate.
- **`deny.toml` / cargo-deny, cargo-nextest, cargo-shear, pre-commit** — ADR-0010 Phase 2+
  (`ADR-0010:30-32`, amendment `:43`); nothing to check against an empty dep tree.
- **Any crate contents** — `locode-protocol` (Task 3), `locode-tools` (Task 4),
  `locode-provider` (Task 5), `locode-engine` (Task 6), etc. land later; Task 1 only
  guarantees they *compile empty*.
- **`[profile.*]` tuning** — none present; cargo defaults suffice for v0 (contrast
  codex `Cargo.toml:518-546`, grok `Cargo.toml:370-375`).

---

## 2. Layout (as built)

### The workspace root

```
Cargo.toml            [workspace] resolver=2, members=["crates/*"];
                      [workspace.package]; [workspace.dependencies]; [workspace.lints]
rust-toolchain.toml   channel 1.97.1 + components [rustfmt, clippy]
rustfmt.toml          edition = "2024"
clippy.toml           allow-{unwrap,expect}-in-tests; doc-valid-idents
Cargo.lock            committed (binary in the tree)
crates/               the eight member crates
```

There is **no** `deny.toml` and **no** `[profile]` section (verified). `members` is a glob
(`crates/*`), not an enumerated list.

### The eight crates and their roles

| Crate | Role (SPEC `:73-80`, ADR-0002 `:15-24`) | As-built status |
|---|---|---|
| `locode-protocol` | conversation model (4-role) + report envelope; pure types, no I/O | **Implemented** (Task 3/3b, `src/lib.rs` 525 ln + golden test) |
| `locode-tools` | `Tool` trait + `Registry` + `dispatch` door; host-agnostic framework | **Implemented** (Task 4, `tool/registry/error/ctx/lib.rs`) |
| `locode-provider` | `Provider` trait + `ConversationRequest` + `MockProvider` + `repair_pairing` | **Implemented** (Task 5/6, 7 files) |
| `locode-engine` | sample→dispatch→append loop + `Session` API | **Implemented** (Task 6, `config/sink/terminal/session/run/lib.rs`) |
| `locode-host` | fs/shell/path-jail/truncation/rg-resolution seam | **Skeleton** (3-line doc-comment lib; Task 7) |
| `locode-packs` | harness packs (grok first); faithful per-harness toolsets | **Skeleton** (3-line doc-comment lib; Tasks 8-13) |
| `locode` | thin facade re-exporting the public surface | **Skeleton** (3-line doc-comment lib; Task 14) |
| `locode-exec` | minimal headless binary; stdout discipline | **Skeleton** (`main.rs` stub + `#![deny(clippy::print_stdout)]`; Task 14) |

`locode-core` is the **repo/workspace name, not a crate** (SPEC `:62`, ADR-0002 `:13`).

### Dependency graph (as built, from the actual manifests)

```
locode-protocol         (leaf: serde, serde_json)
 ├── locode-host         → protocol
 ├── locode-tools        → protocol  (+ serde, serde_json, async-trait, schemars, thiserror, tokio-util)
 │     └── locode-packs  → protocol + tools + host
 ├── locode-provider     → protocol  (+ serde_json, async-trait, thiserror)
 └── locode-engine       → protocol + tools + provider  (+ serde_json, tokio[time], tokio-util)
        └── locode (facade) → all six libs
              └── locode-exec → locode only
```

Acyclic; `protocol` is the shared base every crate reaches, exactly as the SPEC dep
direction requires (`SPEC.md:83`). **Deviation, flagged honestly:** the SPEC graph says
`engine → packs + tools + provider + host + protocol` (`SPEC.md:83`), but as built
`locode-engine/Cargo.toml:12-16` depends only on `protocol + tools + provider` — the
`packs`/`host` edges are deliberately absent, with an in-file comment ("`locode-packs` and
`locode-host` join when the grok pack + host land (Tasks 7-9)"). Task 6 proved the loop
with `MockProvider` + trivial in-test tools, so those edges weren't needed yet. See §5.

---

## 3. Key artifacts (actual contents + rationale)

### `Cargo.toml` (root)

```toml
[workspace]
resolver = "2"
members = ["crates/*"]

[workspace.package]
version = "0.0.0"
edition = "2024"
rust-version = "1.97"
publish = false

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
async-trait = "0.1"     # object-safe async tool traits (native async-fn-in-trait isn't dyn-safe)
schemars = "1"          # JSON Schema derived from Args (ADR-0003); v1 matches Grok Build
thiserror = "2"         # error taxonomy (ToolError); v2 matches both refs
tokio-util = "0.7"      # CancellationToken — the exact type Codex + Grok use
tokio = { version = "1" }

[workspace.lints.rust]
unsafe_code = "forbid"
unused_must_use = "deny"
missing_docs = "warn"
rust_2018_idioms = { level = "warn", priority = -1 }

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
unwrap_used = "deny"    # allowed in tests via clippy.toml
expect_used = "deny"    # allowed in tests via clippy.toml
dbg_macro = "deny"
```

Rationale highlights (full treatment in §4):
- `resolver = "2"` — required with edition 2024; feature unification across the workspace.
- `members = ["crates/*"]` — glob; any new `crates/<x>` is auto-enrolled.
- `[workspace.package]` — single source for `version`/`edition`/`rust-version`/`publish`;
  every crate manifest pulls them with `<field>.workspace = true` (e.g.
  `locode-protocol/Cargo.toml:3-6`). One line to bump the whole tree.
- `[workspace.dependencies]` — centralizes versions so crates write `<dep>.workspace = true`
  (e.g. `locode-tools/Cargo.toml:12-18`) and every crate resolves to the *same* version,
  with features opted in per crate (`tokio = { workspace = true, features = ["time"] }` in
  `locode-engine/Cargo.toml:15`).
- The `priority = -1` on `rust_2018_idioms` and `clippy::pedantic` is load-bearing: group
  lints must have lower priority than the individual `deny`s so a specific `#[allow(...)]`
  or a per-lint override can win over the group (Cargo lint-precedence rule).

### `rust-toolchain.toml`

```toml
[toolchain]
channel = "1.97.1"
components = ["rustfmt", "clippy"]
```

Pins the exact compiler for every developer and (later) CI. No `rust-src` (contrast codex
`rust-toolchain.toml:3`) and no cross-compile `targets` (contrast grok
`rust-toolchain.toml` `targets = [...]`) — neither is needed until the `bundle-rg` cross
build (Task 14, ADR-0011).

### `rustfmt.toml`

```toml
edition = "2024"
```

Near-default (tooling `:184` "keep nearly default"). Only the edition is set, and it is
**deliberately redundant** with `[workspace.package] edition` because `rustfmt` does not
reliably read the manifest edition in every invocation. No `imports_granularity` opinion
yet (contrast codex `rustfmt.toml:3` `imports_granularity = "Item"`).

### `clippy.toml`

```toml
allow-unwrap-in-tests = true
allow-expect-in-tests = true
doc-valid-idents = ["OpenAI", "OpenRouter", ".."]
```

- `allow-{unwrap,expect}-in-tests` — the "ban in library code, allow in tests" pattern
  (ADR-0010 `:40`), mirroring codex `clippy.toml:1-2` exactly. A panic is a fine failure
  signal in a test.
- `doc-valid-idents` — proper nouns that trip the pedantic `doc_markdown` lint but should
  not render as code; `".."` **extends** clippy's built-in valid-idents list rather than
  replacing it. This entry exists *because* we enabled `pedantic` (see §4).

### The crate manifests (the shape every crate repeats)

```toml
# crates/locode-protocol/Cargo.toml — representative
[package]
name = "locode-protocol"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[lints]
workspace = true        # inherit the workspace [workspace.lints] tables

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
```

`[lints] workspace = true` is what makes the strict lint tables actually apply to each
crate — without it, `[workspace.lints]` is inert.

---

## 4. Decisions as-built (source · why · why-not · harness diff)

### 4.1 Multi-crate workspace of eight `locode-*` crates
- **Source.** ADR-0002 `:12-24`; design `:79-102`. The design doc mirrors "the cleanest
  open-source splits (Codex and Grok both separate a portable tools layer from the session
  loop; Grok additionally separates *tool definition* from *dialect selection* and *wire
  protocol*)" (design `:81`, table `:94-102`).
- **Why these eight boundaries.** Each is a seam the compiler enforces rather than
  convention: `protocol` = pure shared base (no wire types leak inward); `tools` =
  host-agnostic framework, kept separate from concrete tools so the `Tool` contract stays
  stable; `packs` = concrete per-harness toolsets, separate so *tool definition ≠ harness
  selection* (the Grok `xai-grok-tools` registry vs `xai-grok-agent` toolset-presets lesson,
  design `:98`); `provider` = wire abstraction depending only on `protocol`, so wires stay
  swappable and network-isolated; `host` = the single side-effect seam (fs/shell/jail) so
  tools are trivially testable and sandbox-ready (design `:100`); `engine` = orchestration
  separate from tools (the codex `codex-tools` vs `codex-core` lesson, design `:96`);
  `locode` = facade for the future `locode-app`; `exec` = minimal binary depending only on
  the facade, so stdout discipline is enforced in isolation.
- **Why not fewer (single crate, or 3-4 coarse crates).** ADR-0002 `:27-33` rejected both:
  a single crate makes boundaries "conventions only" that erode as code grows; 3-4 coarse
  crates "soften exactly the boundaries we most want hard (host seam, provider wire, tools
  vs loop)." Guiding rule: "We can always merge later; splitting later is harder"
  (ADR-0002 `:33`).
- **Harness diff — crate granularity.** Grok ships ~130 crates under
  `crates/{build,codegen,common}/xai-*` (`grok/Cargo.toml:3-80+`, e.g. `xai-grok-tools`,
  `xai-grok-sampler`, `xai-tool-protocol`, `xai-tool-runtime`, `xai-tool-types`,
  `xai-grok-compaction`); Codex ships ~150 flat-named crates (`codex/Cargo.toml:1-130`,
  e.g. `core`, `tools`, `exec`, `protocol`, `apply-patch`, plus a `utils/*` subtree). Both
  split at the "every reusable unit is its own crate" grain appropriate to a large
  production tree with an internal merge queue. We split at the **architectural-seam** grain
  — eight crates that map onto the survey's proposed seven (design `:83-91`:
  `agent-{cli,loop,tools,dialects,protocol,provider,host}`) with two renames and one
  addition: `loop → engine`, `cli → exec`, `dialects → packs` (ADR-0012 superseded the
  dialect model), plus a `locode` **facade** crate the survey did not propose. Eight is the
  smallest set that keeps every seam hard without ~130-manifest overhead we have no reason
  to pay yet.
- **Naming note (deviation).** ADR-0002 `:19` still lists `locode-dialects`; the shipped
  crate is `locode-packs` because ADR-0012 replaced the dialect model with harness packs.
  The ADR-0002 table is stale on that row; SPEC `:75` and the tree use `locode-packs`.

### 4.2 Edition 2024, toolchain pin `1.97.1`, `rust-version = "1.97"` (MSRV)
- **Source.** `Cargo.toml:8` (`edition = "2024"`), `:9` (`rust-version = "1.97"`);
  `rust-toolchain.toml:2` (`channel = "1.97.1"`). ADR-0010 `:15` mandates
  "`rust-toolchain.toml` pinning current stable + `rustfmt`, `clippy` (bump one minor at a
  time, deliberately)"; grok's own bump policy is the model — "bump one point version at a
  time … wait at least a couple of weeks after the release" (`grok/rust-toolchain.toml`
  comment).
- **Why.** One compiler for everyone; edition 2024 is the current edition and unlocks the
  `resolver = "2"` default and the latest idioms. `rust-version` records the MSRV so a
  too-old toolchain fails fast with a clear cargo message rather than a cryptic build error.
- **Why-not alternative.** Not floating `stable` (reproducibility — CI and local would
  drift). Not lagging further behind like the harnesses (see diff) because we are greenfield
  with no merge-queue exposure; the churn cost of a newer pin is low.
- **Harness diff.** Codex pins `1.95.0` + `rust-src` (`codex/rust-toolchain.toml:2-3`); Grok
  pins `1.92.0` + cross-compile `targets` and a documented "wait weeks, bump one point"
  policy. We sit slightly ahead at `1.97.1` and omit both `rust-src` and `targets` (not
  needed until the rg cross-build). **Concern:** `rust-version = "1.97"` is pinned right at
  the toolchain, so there is *zero* MSRV headroom — fine for a binary/app
  (`publish = false`), but a stated MSRV floor matters more once `locode-app` consumes these
  as libraries. See §8.

### 4.3 `unsafe_code = "forbid"`
- **Source.** `Cargo.toml:30`; ADR-0010 amendment `:38` ("`unsafe_code = "forbid"` — this
  project needs no `unsafe`").
- **Why.** The core is pure orchestration + typed data; no FFI, no `mmap`, no perf-critical
  unsafe. `forbid` is strictly stronger than `deny` — it cannot be re-enabled by an inner
  `#[allow(unsafe_code)]`, so no crate can quietly opt back in. Free to adopt while the
  crates are empty (the strict-from-empty thesis, §5).
- **Why-not.** `deny` would let a local `#[allow]` punch through; `forbid` closes that door.
- **Harness diff.** Neither Grok nor Codex forbids `unsafe` workspace-wide — both are large
  trees with PTY/sandbox/FFI/process-hardening crates that legitimately need it
  (codex `windows-sandbox-rs`, `linux-sandbox`; grok `xai-grok-sandbox`, `ptyctl`). We can
  forbid precisely because the *core* library carves those side-effecting concerns out into
  `locode-host` and the (future) sandbox seam. **Watch-out:** if the `bundle-rg`
  self-extract (Task 14) or any future FFI needs `unsafe`, `forbid` blocks it even with a
  local `#[allow]`; that would force a workspace-lint change (or isolating the unsafe into a
  crate that overrides the lint). See §8.

### 4.4 `clippy::pedantic = "warn"` (the group) + `unwrap_used`/`expect_used`/`dbg_macro = "deny"`
- **Source.** `Cargo.toml:36-39`; ADR-0010 amendment `:39` ("`pedantic = "warn"` (the
  opinionated high-value group; individual lints get `#[allow(…)]` with a reason when
  genuinely wrong), plus `unwrap_used`/`expect_used`/`dbg_macro = "deny"`").
- **Why.** Take the whole opinionated `pedantic` group as a baseline and grant
  `#[allow(..., reason = "...")]` exceptions where a lint is genuinely wrong, rather than
  hand-curating a list. `unwrap_used`/`expect_used` denied in library code (panics are
  banned on the input-driven paths — Definition of Done, `tasks/plan.md:86`); `dbg_macro`
  denied so no stray `dbg!` ships.
- **Why-not — Codex's enumeration.** Codex does **not** enable the `pedantic` group; it
  cherry-picks ~35 individual clippy lints to `deny`
  (`codex/Cargo.toml:472-507`: `await_holding_lock`, `redundant_clone`,
  `uninlined_format_args`, the whole `manual_*`/`needless_*`/`redundant_*` families, plus
  `unwrap_used`/`expect_used`). That avoids group churn but is a long list to maintain. We
  accept the group + reasoned allow-list instead. **Tradeoff (real):** `pedantic` *grows*
  across rustc releases, so a toolchain bump can surface new pedantic warnings that fail CI
  under `-D warnings` — a churn vector that partly works against the very reason we pinned
  the toolchain. Codex's enumeration is immune to that. See §8.
- **`missing_docs = "warn"` (not `deny`).** `Cargo.toml:32`, ADR-0010 `:38`. Every public
  item should be documented, but it is a `warn` — under CI's `-D warnings` (Task 2) it
  effectively blocks, yet locally it merely nags. Same for `pedantic`: `warn` at the source,
  `deny` in practice via the gate.
- **Harness diff — posture.** Grok's `[workspace.lints.clippy]` is mostly **`allow`**
  (`grok/Cargo.toml:378-403`): it *loosens* defaults (`uninlined_format_args = "allow"`,
  `too_many_arguments = "allow"`, …). The famous comment there (`grok/Cargo.toml:388-397`)
  explains why — "When older branches … get merged, they pass their own CI … once merged to
  main, the code violates main's lint rules … break the build for everyone. … TODO: ->
  "deny" once/if merge queue enabled." That is the exact "tightening lints *after* code
  exists is painful" trap ADR-0010's amendment cites (`ADR-0010:35`). We are in the opposite
  position — empty crates, solo repo, no merge queue — so we **tighten** where Grok is
  forced to loosen.

### 4.5 `clippy.toml`: `allow-{unwrap,expect}-in-tests` + `doc-valid-idents`
- **Source.** `clippy.toml:5-10`; ADR-0010 `:40`.
- **Why.** `unwrap`/`expect` are denied in library code but a panic is a perfectly good
  failure signal in a test, so allow them there. `doc-valid-idents = ["OpenAI",
  "OpenRouter", ".."]` teaches `doc_markdown` (a `pedantic` lint) that these proper nouns
  are not code — a **direct consequence** of enabling `pedantic` in §4.4; without pedantic
  this file would not need the entry.
- **Harness diff.** The test allow-lists are copied verbatim from codex
  (`codex/clippy.toml:1-2`). Codex's `clippy.toml` also carries `disallowed-methods`
  (ratatui color bans) and `await-holding-invalid-types`; Grok's bans raw
  `std::fs::canonicalize` in favor of `dunce` (`grok/clippy.toml:26-30`). We have **no
  domain bans yet** — none apply until concrete fs/shell code exists in `locode-host`
  (Task 7), at which point a `canonicalize` ban is a candidate (SPEC threat model + path
  jail, ADR-0008).

### 4.6 `[workspace.dependencies]` centralization
- **Source.** `Cargo.toml:12-25`; each crate references with `<dep>.workspace = true`.
- **Why.** One place to set versions + feature baselines; every crate resolves the same
  version, and features are opted in per crate (dev-only `macros`/`rt` for tokio in
  `locode-provider/Cargo.toml:20`, `time` for the engine's resample backoff in
  `locode-engine/Cargo.toml:15`). Prevents version skew across eight manifests.
- **Harness diff.** Standard practice in both harnesses. The version choices are pinned to
  the refs: `schemars = "1"` and `async-trait = "0.1"` "matches … Grok Build's tool
  runtime" (`Cargo.toml:16-19`); `thiserror = "2"` "matches both refs" (`:21`); `tokio-util`
  is "the exact type Codex and Grok use" for `CancellationToken` (`:22`).

### 4.7 `resolver = "2"`, `members = ["crates/*"]`, `Cargo.lock` committed
- **Source.** `Cargo.toml:2-3`; `Cargo.lock` present (7.4 KB); ADR-0010 `:20` ("Commit
  `Cargo.lock` (this repo ships a binary)").
- **Why.** Resolver 2 is the edition-2024 default (per-target/dev feature isolation). The
  `crates/*` glob keeps the members list zero-maintenance. Lockfile committed because the
  tree ships `locode-exec` (industry baseline, tooling `:131`).
- **Harness diff.** Both harnesses **enumerate** members explicitly
  (`grok/Cargo.toml:4-80`, `codex/Cargo.toml:2-130`) — necessary at their scale where dirs
  under `crates/` may not all be members and ordering/inclusion is curated. Our glob is
  simpler but will silently enroll any new `crates/<dir>`; a minor footgun at eight crates
  (see §8).

---

## 5. What actually happened / deviations from the ideal

1. **Lints landed in Task 2, not Task 1 — timeline honesty.** `tasks/todo.md:15` scopes
   Task 1's `[workspace.lints]` to the *mild* `unused_must_use = "deny"` only; the strict
   `rust`/`clippy` tables (unsafe-forbid, pedantic, unwrap/expect/dbg deny, missing_docs)
   were formally introduced by the **ADR-0010 amendment** dated the same day and enumerated
   under "Enabled at scaffold time (**Task 2**)" (`ADR-0010:37-40`, `tasks/todo.md:32`). The
   as-built root `Cargo.toml` today carries the full strict tables. This doc covers them
   because the user scoped "fmt/clippy config + `[workspace.lints]`" into Task 1's plan and
   excluded only CI/justfile — but the strict lint *table* is chronologically a Task-2
   artifact. Practically the two tasks were a single scaffolding push.

2. **`[workspace.dependencies]` is a Task-3+ artifact, not pure Task 1.** At true Task 1 the
   crates were empty and needed no external deps. The centralized version table
   (`serde`, `schemars`, `thiserror`, `tokio-util`, `async-trait`, `tokio`) was populated as
   Tasks 3-6 added real deps (Task 4 design notes explicitly record adding `async-trait`,
   `schemars`, `thiserror`, `tokio-util` — `tasks/todo.md:94`). Each of those is an
   "Ask first: adding a dependency" boundary item (SPEC `:124`). The manifests here reflect
   the *merged* state, not a hypothetical empty-Task-1 snapshot.

3. **Half the crates are still 3-line skeletons.** `locode-host`, `locode-packs`, `locode`
   are doc-comment-only libs; `locode-exec` is a `main()` stub (with the
   `#![deny(clippy::print_stdout)]` guard already in place — `crates/locode-exec/src/main.rs:5`).
   The other four (`protocol`, `tools`, `provider`, `engine`) are fully implemented
   (Tasks 3-6, Checkpoint B reached — `tasks/plan.md:54`). So the workspace is half real,
   half reserved seam. This is exactly ADR-0002's accepted bet ("splitting later is harder")
   — but see the boundary-churn concern in §8.

4. **`engine` does not yet realize the full SPEC dep graph.** `SPEC.md:83` says
   `engine → packs + tools + provider + host + protocol`; as built it depends on
   `protocol + tools + provider` only (`locode-engine/Cargo.toml:12-16` + the explanatory
   comment). The `packs`/`host` edges are pending Tasks 7-9. Not a bug — the loop was proven
   against `MockProvider` + in-test tools — but the graph in the SPEC is aspirational until
   Phase 2 lands.

5. **ADR-0002's crate table is stale** on the `locode-dialects` row (now `locode-packs` per
   ADR-0012). The decision changed via a superseding ADR, as the working agreement requires;
   the older ADR was not retro-edited.

---

## 6. How it's verified

The Task 1 verification bar (`tasks/todo.md:19`), i.e. the "empty workspace compiles" half
of Checkpoint A (`tasks/plan.md:45`):

```sh
cargo build --workspace                                   # every crate compiles (empty lib / bin)
cargo fmt --all -- --check                                # rustfmt clean
cargo clippy --workspace --all-targets -- -D warnings     # lint-clean under the deny gate
```

- `Cargo.lock` committed and consistent.
- Dependency directions acyclic (a cycle would fail `cargo build` — the compiler is the
  test here; there is no runtime behavior to exercise yet).
- Because CI is Task 2, at Task 1 the gate is run locally; the same three commands become
  the CI job later (SPEC `:42-46`, ADR-0010 `:13`). The full "mandatory triangle" adds
  `cargo test --workspace`, which at Task 1 has no tests to run.

---

## 7. Dependencies considered

No dependency is *required* to make eight empty crates compile — `cargo build` on
doc-comment-only libs needs nothing beyond std. The `[workspace.dependencies]` table exists
to **pre-declare shared versions** so later tasks add `<dep>.workspace = true` without a
version decision each time. All entries trace to a ref or ADR:

| Dep | Version | Why / grounding |
|---|---|---|
| `serde` (+derive) | 1 | serialization of protocol/report types (SPEC tech stack `:34`) |
| `serde_json` | 1 | `Value` for tool args + report JSON |
| `async-trait` | 0.1 | object-safe async `Tool`/`Provider` (native async-fn-in-trait not dyn-safe) — `Cargo.toml:16-17`, ADR-0003 |
| `schemars` | 1 | JSON Schema derived from `Args` (ADR-0003); v1 matches Grok |
| `thiserror` | 2 | error taxonomy; v2 matches both refs |
| `tokio-util` | 0.7 | `CancellationToken` for `ToolCtx.cancel` — the exact type Codex/Grok use |
| `tokio` | 1 | async runtime; features opted in per crate |

**Not added (deliberately):** `deny.toml`/cargo-deny (empty dep tree — ADR-0010 `:43`),
`reqwest`/`clap`/`minijinja` (arrive with their consuming tasks — SPEC `:36-38`), and the
clippy `cargo` group (needs license/metadata fields first — ADR-0010 `:43`). Each future
external crate is an "Ask first" event (SPEC `:124`).

---

## 8. Open questions / concerns / future considerations

Exhaustive and honest — surface everything worth an interview.

1. **A `locode-transcript` (or `-history`) crate?** Pairing repair (`repair_pairing`) and
   the conversation model currently straddle `locode-protocol` (types) and
   `locode-provider` (the `repair.rs` logic, decided in Task 6 — `tasks/todo.md:138`). Task
   6's plan debated hosting it in `protocol` vs `provider` vs `engine`. If history hygiene
   grows (dedup, compaction, durability/JSONL — SPEC open Q4), a dedicated transcript crate
   may earn its keep. Right now it's a cross-cutting concern with no clean home — a latent
   boundary question.

2. **Crate-boundary churn risk.** Boundaries were drawn (ADR-0002) *before* the code that
   fills them. Four crates are still skeletons; the `engine → packs/host` edges don't exist
   yet. Empty crates can hide a *wrong* boundary until code lands. Concretely: will
   `locode-packs` and `locode-host` prove to be the right cut, or will pack-specific host
   needs (e.g. grok's `search_replace` freshness, rg resolution) blur the packs/host line?
   ADR-0002 bet "merge later is easy, split later is hard" — but a merge still churns
   manifests and imports across the tree.

3. **`locode-packs` / `locode-host` staying skeleton — a smell?** They compile but do
   nothing (3-line docs). Two crates carrying only a module comment is defensible as a
   reserved seam, but it also means the two seams most central to v0's *remaining* work
   (the side-effect boundary and the harness-pack model) are entirely unvalidated by code.
   If Task 7/8 reveal the shape is wrong, the cost lands late.

4. **MSRV policy is unstated.** `rust-version = "1.97"` is pinned flush against the toolchain
   (`1.97.1`) — no headroom. For `publish = false` binaries that's fine, but `locode-app`
   will consume these as libraries and may want a deliberate MSRV floor with headroom. There
   is also **no documented bump cadence** in-repo (Grok writes its "one point at a time, wait
   weeks" policy directly in `rust-toolchain.toml`; we don't). ADR-0010 `:15` states the
   intent ("bump one minor at a time, deliberately") but it isn't codified where a bumper
   would see it.

5. **`pedantic`-group + toolchain-bump churn (the sharpest lint tradeoff).** `clippy::pedantic`
   grows across rustc releases, and CI runs `-D warnings` (Task 2). So a toolchain bump can
   introduce brand-new pedantic warnings that fail CI on unchanged code — the exact churn we
   pinned the toolchain to avoid, reintroduced through the lint group. Codex sidesteps this
   by enumerating individual lints (`codex/Cargo.toml:472-507`). Open question: keep the
   group (less to maintain, more bump-sensitive) or migrate to an enumerated deny-list
   (bump-stable, more upfront curation)? The `doc-valid-idents` list will also grow with
   every new proper noun the docs mention.

6. **`unsafe_code = "forbid"` vs the `bundle-rg` path.** Task 14's rg self-extract
   (`include_bytes!` + runtime extraction, ADR-0011) may want `unsafe` (mmap/exec bits). A
   `forbid` cannot be locally `#[allow]`-ed — it would force a workspace-lint edit or
   isolating the unsafe in a crate that overrides the table. Decide now whether the core
   truly never needs unsafe, or downgrade to `deny` for override flexibility.

7. **`print_stdout`/`print_stderr` not denied workspace-wide — a hardening gap.** SPEC
   boundary: "`println!` from library crates or non-report paths" is forbidden (SPEC `:125`),
   and it's enforced *structurally* only in `locode-exec` via
   `#![deny(clippy::print_stdout)]` (`crates/locode-exec/src/main.rs:5`). A library crate
   (`protocol`/`tools`/`provider`/`engine`) could `println!` today and clippy would **not**
   catch it, because `print_stdout` is not in `[workspace.lints.clippy]`. Codex denies it in
   its `exec` crate too (design `:34`), but for us the *library* crates are the ones the
   boundary targets. Candidate: add `print_stdout`/`print_stderr = "deny"` to the workspace
   clippy table (with a per-`exec` allow for the one report write) to make the invariant
   compiler-enforced everywhere, not just in the binary.

8. **`members = ["crates/*"]` glob footgun.** Any new directory under `crates/` is
   auto-enrolled as a member; a stray scratch/fixture dir with a `Cargo.toml` would join the
   build silently. Both harnesses enumerate. Low risk at eight crates; note it.

9. **Publishing / metadata.** `publish = false` + `version = "0.0.0"` + no `license`,
   `description`, or `repository` fields. Fine while `locode-app` consumes via path/git.
   Publishing later needs real semver, license, and metadata (and unblocks the clippy `cargo`
   group ADR-0010 `:43` deferred). Also: all crates share one `version` — independent
   per-crate semver would be needed if we ever publish crates individually. SPEC open Q5
   (facade surface — how much `locode` re-exports) is the adjacent unresolved question.

10. **workspace-hack (cargo-hakari).** Unneeded at eight crates, but if the tree grows toward
    harness scale, a workspace-hack crate reduces feature-unification rebuild churn. Note for
    later; not now.

11. **`rustfmt.toml` is opinion-free.** Only `edition` is set — no import grouping
    (`imports_granularity`, codex `rustfmt.toml:3`) or ordering policy. If import-diff noise
    becomes annoying across contributors/agents, adopt an explicit granularity. The
    edition is also double-declared (manifest + rustfmt.toml); intentional but a maintenance
    nuance if the edition ever bumps (two places to change).

12. **No `rust-src` component / no cross-compile `targets`.** Omitted vs codex (`rust-src`)
    and grok (`targets`). Harmless until the `bundle-rg` cross-build or any `build-std`/miri
    need — at which point `rust-toolchain.toml` grows.

13. **No `deny.toml` / supply-chain policy.** Deferred (ADR-0010 `:43`, Phase B). The moment
    real deps land (`reqwest` and its transitive tree in Task 12), advisories/licenses become
    worth a `cargo-deny`/`cargo-audit` gate.

14. **No `[profile]` tuning.** Cargo defaults only. Both harnesses tune dev/release profiles
    (grok `Cargo.toml:370-375` `debug = "line-tables-only"`; codex `:518-546`). Irrelevant
    for a library-heavy v0, but a fast-compile dev profile may be wanted once the tree and
    test suite grow.

---

## 9. Speech-to-text / identifier confirmations

This plan worked entirely from written source (ADRs, SPEC, manifests) — no spoken
identifiers were reconstructed, so there is nothing here to confirm. The one
naming discrepancy surfaced is documentary, not a guess: **ADR-0002's crate table lists
`locode-dialects`, but the shipped crate is `locode-packs`** (ADR-0012 superseded it) —
flagged in §4.1/§5 rather than silently normalized.
