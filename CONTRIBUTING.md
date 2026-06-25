# Contributing to AgentPipe

Thanks for taking a look. First, set expectations honestly:

**AgentPipe is a personal tool published as-is.** It was built around one person's workflow (Claude Code + Codex, macOS-first). The bar for merging is "does it stay simple, tested, and fail-closed" — not "does it cover every workflow." Small, focused PRs and bug reports are very welcome. Large feature proposals are best opened as an issue first so we can agree on scope before you write code.

## Project layout

```
crates/engine/   # the orchestration engine (no UI deps): manifest, executor, runners, audit, worktree
crates/cli/      # the `agentpipe` CLI (run / validate / runs / view / cost / diff)
src-tauri/       # Tauri desktop shell (Rust side)
ui/              # React + TypeScript desktop UI
templates/       # example task.yaml files
demo/            # stub binaries + VHS tape for the README GIF
docs/            # specs (design intent), plans (steps), research (competitor comparisons)
```

The engine is the heart; it has no UI or Tauri dependency and is where most logic-tested behavior lives.

## Dev setup

```bash
cargo build                 # build the workspace
cargo test                  # Rust tests (engine + cli + tauri)

cd ui && npm install        # UI deps
npm test                    # UI unit tests (vitest)
npm run build               # tsc + vite build (this is what catches UI type errors)

cargo tauri dev             # run the desktop app (auto-starts the Vite UI)
```

### What "green" means before a PR

- `cargo test` passes (workspace).
- `cd ui && npm run build` passes (this runs `tsc`, which `npm test` alone does **not**).
- New behavior has a test. The engine especially favors tests on the pure logic (manifest parsing/validation, executor decisions, audit serialization, interpolation).

You can run the whole pipeline without any API calls using the stub binaries — see the Quickstart in the [README](README.md) and the `demo/` folder.

## Design principles (please keep these)

- **Fail-closed.** Anything ambiguous — a parse failure, a missing verdict, a loop hitting `max` — takes the conservative branch (stop / ask a human / treat as `changes_requested`). Never silently pass.
- **Explicit data flow.** Steps pass data via `{{id.field}}` interpolation, not hidden global state.
- **The manifest is the single source of truth.** A run should be reproducible from its YAML (plus seeded inputs). Display strings live in the CLI/UI layer, not the engine.
- **Security at the boundary.** run-ids are allowlisted before touching the filesystem; worktree isolation is fail-closed; `claude` runs at bypassPermissions, so the target repo must be trusted.

When in doubt, read the matching doc under `docs/specs/` — most non-trivial behavior has a design note.

## Adding support for another agent

1. Add a runner in `crates/engine/src/runner/` (mirror `claude.rs` / `codex.rs`) and wire it into `RunnerBins` (`crates/engine/src/executor.rs`).
2. Add a `StepKind` variant in `crates/engine/src/manifest.rs` (and its TS mirror in `ui/src/types.ts` if the GUI should build it).
3. Keep the binary path overridable via an `AGENTPIPE_*_BIN` env var, like the existing runners.

If you only need to swap the *binary* (not add a new step kind), no code change is needed — set `AGENTPIPE_CLAUDE_BIN` / `AGENTPIPE_CODEX_BIN`.

## Commits & PRs

- Keep commits focused; describe what changed and why.
- Note the commands you ran (`cargo test`, `npm run build`).
- For UI changes, a screenshot or short clip helps.

## Regenerating the README demo GIF

```bash
brew install vhs            # needs ttyd + ffmpeg (pulled in as deps)
vhs demo/agentpipe.tape     # writes demo/agentpipe.gif
```
