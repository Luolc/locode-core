# Research — the tool-description interface (static `&str` vs dynamic)

> **Status: analysis locked down; NO interface decision made yet.** Tracked in
> [`tasks/tracker.md`](../../tasks/tracker.md) (TUI/tech-debt backlog). Prompted by
> the Task-20 claude-pack attribution finding: Claude Code's `Bash` description
> embeds the *running model's* name in a commit-attribution line, which a static
> `&str` `Tool::description(&self) -> &str` (built at `register()`, without
> `PackContext`) cannot render truthfully. We **dropped** that line (truth-first,
> see AGENTS.md "Fidelity vs. truth"); this doc records whether the *interface*
> itself needs to change before more packs land.

## The question
Across **all four studied harnesses** (not just the basic tools — the fancy ones
too), how is a tool's **model-facing description** constructed? If we keep everything
a static `&str` (our current `Tool::description(&self) -> &str`, backed by
`include_str!`), what can't we reproduce? If we go dynamic, what are the options?

## What each harness actually does

| Harness | Mechanism | Dynamic inputs |
|---|---|---|
| **opencode** | **Static** — `import DESCRIPTION from "./glob.txt"` (`.txt` imported as a string), used verbatim (`tool/glob.ts:7,23`). | none — exactly our `include_str!` model. |
| **claude-code** | **Functions** returning strings (`getSimplePrompt`, `getEditToolDescription`, `getWriteToolDescription`, `getDescription` …) that interpolate at build time. | tool names (`BASH_TOOL_NAME` …), **the running model** (commit attribution, `attribution.ts`), timeouts, **feature flags** (`MONITOR_TOOL`, background, sandbox), `USER_TYPE`, settings. Heavily dynamic. |
| **grok-build** | **Template + context render** — `Tool::description(&self, ctx: &ListToolsContext) -> ToolDescription` plus `description_template(&self) -> &str` (`xai-grok-tools/src/bridge.rs:666,678`). The template is rendered against a per-`ListTools` context. | **tool-name injection by kind** (grok's dialect — the actual registered name of the read/edit-kind tool), context. Dynamic via the context arg. |
| **codex** | **Owned `String`**, many built with `format!(...)` or passed in (`tools/handlers/*_spec.rs`, `code_mode/wait_spec.rs:34`, `multi_agents_spec.rs:674`, `create_request_user_input_tool(description: String)`). | interpolated values for the fancier tools (agent-type lists, installable-plugin lists, config); core tools are effectively static consts. |

**Takeaway:** two poles. opencode = pure static (our model). claude-code/grok-build =
dynamic (a build-time function or a runtime context render). codex sits between (owned
`String`, `format!` where a tool needs it).

## What a static `&str` **cannot** reproduce
For the packs we build (a *fixed* tool set with *fixed* wire names and a *fixed*
ported config), a static `&str` covers almost everything, because the "dynamic" inputs
are resolved **at port time** by our config choice (tool names are known; timeouts,
caps, feature-flag branches are frozen to the ported config; snapshots pin the bytes).
The genuine gaps are values only knowable **at run time**:

1. **Runtime model-dependent text** — Claude Code's `Co-Authored-By: {model}` (and any
   "You are powered by {model}" that lived in a *description* rather than the env
   block). The model isn't known at `register()`. **This is the only gap we've
   actually hit.** Decision: **drop it** (truth-first) rather than hardcode a wrong
   name — a static description that lies about the model is worse than an absent line,
   and it has ~zero effect on the code the model writes (what the A/B measures).
2. **Runtime-injected tool names that vary per run** — grok's `by_kind` dialect. For a
   single pack with fixed names this is resolvable at port time (we hardcode the
   names), so static works *today*; it would only bite if a pack registered the same
   tool under a run-chosen name.
3. **Config/feature-flag-conditional paragraphs at run time** — claude's sandbox /
   background sections. We pick one config at port time, so static works; we just
   can't flip config per run.

So: **static `&str` is sufficient for every pack we can foresee, once the one
runtime-model-dependent line is dropped.** The interface does *not* need to change to
finish the studied-harness packs.

## Options, if a future pack forces dynamic (ranked by blast radius)
1. **Thread a context into construction (recommended if forced).** Grow
   `Pack::register(&self, host, ctx: &PackContext, registry)`; a tool stores an owned
   `String`/`Cow` description built from `ctx` (codex's owned-String model). `description()`
   still returns `&str` (a borrow of the stored String) — **no `Tool` trait signature
   change.** Blast radius: the `Pack` framework + each pack's `register`.
2. **A tiny template layer.** Keep `include_str!` templates with `{placeholder}`
   markers; render with a simple `str::replace` (or a minimal engine) at construction,
   using the same threaded context as (1). This is grok's `description_template` +
   render model. Adds a render step, not a dependency (a full Jinja/Tera/minijinja
   engine is unjustified for a handful of placeholders).
3. **Change `Tool::description` to take a context** — `description(&self, ctx) -> Cow<str>`
   (grok's real signature). Most flexible, matches grok exactly, but a **public trait
   signature change** (`Tool`) touching every tool in every pack. Highest blast radius;
   avoid unless (1)/(2) prove insufficient.
4. **Per-tool callback** — `Box<dyn Fn(&Ctx) -> String>`. Maximal flexibility, least
   ergonomic; only if descriptions need arbitrary logic. Not currently justified.

## Recommendation (to revisit, not yet decided)
Keep the static `&str` + `include_str!` interface for now — it matches opencode
exactly and covers claude/grok/codex once the single runtime-model line is dropped
(done). If a future pack genuinely needs run-time-varying description text, reach for
option **1** (thread `PackContext` into `register`, store an owned `String`), and only
add option **2**'s templating if the placeholders multiply. Do **not** change the
`Tool::description` signature (option 3) unless 1+2 are proven insufficient.

Re-open this when: (a) a ported pack has a description that must vary with run-time
state we can't drop, or (b) we want the claude attribution line back with a truthful,
dynamic model name.
