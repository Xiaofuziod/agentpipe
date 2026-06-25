# AgentPipe

**A cross-vendor adversarial review pipeline for AI coding CLIs — with deterministic command gates.**

One model *writes*, a **different** model *reviews*, and the loop repeats until the reviewer is clean. AgentPipe declares that flow as a small YAML manifest, runs your existing `claude` and `codex` CLIs as black boxes, and gates progress on real exit codes — not vibes.

> 中文说明见 [README.zh-CN.md](README.zh-CN.md)

![AgentPipe running the cross-vendor review loop](demo/agentpipe.gif)

> **Status: alpha / personal tool, made public as-is.** Built for one person's workflow (Claude Code + Codex, macOS-first). It is small (~8.6k LOC), tested (100+ tests), and the engine is cross-platform-aware, but it has not been hardened for general use. Issues and PRs welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).

## Why this exists (and how it's different)

The 2026 multi-agent landscape is mostly **parallel swarms and kanban boards**: run N agents at once, review the diffs by hand. AgentPipe is the other shape — a **serial, declarative quality pipeline** with three opinions:

- **Cross-vendor adversarial review.** The author and the reviewer are different vendors on purpose (Claude writes, Codex reviews). Different models have different blind spots, so the reviewer catches what the author can't see in its own work — then the author fixes and the reviewer re-checks, looping until clean.
- **Deterministic command gates.** A verify gate can be `by: command` — your own test/build/lint command, where **exit code 0 = goal met**. No LLM judging whether the tests "probably pass." Run the tests.
- **Fail-closed convergence.** A parse failure, a missing verdict, or hitting the loop's `max` does **not** silently pass. It stops and asks a human. The conservative branch is always the default.

If you want a parallel agent swarm, use [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) or Conductor. If you want *"Codex keeps tearing apart Claude's work until it's actually clean, gated on my test suite"*, that's this.

## Quickstart

```bash
cargo build --release        # builds the `agentpipe` CLI
cargo test                   # ~100 tests
```

Try the full flow with **no API calls** using the bundled stub binaries — this is exactly what the GIF above records:

```bash
mkdir -p /tmp/ap-demo/repo && (cd /tmp/ap-demo/repo && git init -q)
AGENTPIPE_CLAUDE_BIN=$PWD/demo/stub-claude.sh \
AGENTPIPE_CODEX_BIN=$PWD/demo/stub-codex.sh \
AGENTPIPE_HOME=/tmp/ap-demo \
./target/release/agentpipe run demo/demo-task.yaml
```

For real runs, have the `claude` and `codex` CLIs on your `PATH` (or point `AGENTPIPE_CLAUDE_BIN` / `AGENTPIPE_CODEX_BIN` at them) and:

```bash
agentpipe run task.yaml            # run a pipeline
agentpipe run task.yaml --dry-run  # parse + validate + print the plan, spawn nothing
agentpipe validate task.yaml       # parse + validate only
agentpipe runs                     # list past runs
agentpipe view  <run-id>           # replay a run's events
agentpipe cost  <run-id>           # per-step cost / turns / duration
agentpipe diff  <run-a> <run-b>    # diff two runs
```

## The killer pattern: an MR review-fix loop

[`templates/mr-review-loop.yaml`](templates/mr-review-loop.yaml) is the flow in the GIF: paste an MR/PR link, Claude checks out the branch, then Codex reviews the diff and Claude fixes the findings — **looping until Codex reports clean** (or stopping for a human at `max`).

```yaml
version: 1
name: "MR review-fix loop"
target: /abs/path/to/repo
mode: auto
steps:
  - id: mr
    kind: human
    instruction: "Paste the MR/PR link"
    expects: "MR link"
  - id: checkout
    kind: claude
    prompt: "Check out the branch for {{mr.artifact}} ..."
  - id: review-fix
    kind: loop
    until: codex-clean       # converge when Codex's verdict is `clean`
    max: 10                  # hit the ceiling -> pause for a human, never pass silently
    body:
      - id: review
        kind: codex
        action: review-mr
        base: main
      - id: fix
        kind: claude
        prompt: "Fix Codex's findings on {{mr.artifact}} and push.\n\n{{review.findings}}"
```

Data flows between steps by explicit interpolation — `{{mr.artifact}}`, `{{review.findings}}`, `{{review.verdict}}`.

## Step types

| kind   | what it does |
|--------|--------------|
| claude | Run Claude once on a prompt (optionally referencing a skill). Runs at the CLI's highest permission (bypassPermissions). |
| codex  | Codex as a reviewer: `review-doc` / `review-mr` (structured `verdict` + `findings`) / `ask`. |
| human  | A human does something (often in their own Claude Code session); the engine waits for approval + an artifact. Can be pre-seeded with `value` to run headless. |
| loop   | Wrap a body of sub-steps; `until: codex-clean` converges, or stop at `max`. |

Any `claude` / `codex` step can carry a **verify gate** (`verify: { by: codex | claude | command, ... }`) that re-checks whether the step met its goal and retries with feedback if not.

## Desktop GUI (Tauri)

```bash
cargo tauri dev      # auto-starts the Vite UI + a Tauri window
```

Three panes: **projects** (runs grouped by target repo, persisted under `~/.agentpipe/runs/` with cost, pairwise diff), **console** (live execution / read-only replay / a quick-run prompt bar), and **composer** (build a `task.yaml` visually, save it as a template, or fill in the launch conditions and run). The same stub binaries drive the GUI demo.

> Note: the GUI's labels are currently Chinese; the CLI is English. English GUI strings are a known gap.

## Persistence & audit

Every run is appended as NDJSON to `~/.agentpipe/runs/<run-id>.ndjson` (run-id is allowlisted to `[A-Za-z0-9_-]` to prevent path traversal). That's the basis for `view` / `cost` / `diff` and the GUI's read-only replay.

## Platform support

| Platform | CLI engine | Desktop GUI |
|----------|-----------|-------------|
| macOS    | primary, tested | primary, tested |
| Linux    | should work (unix paths; same code paths as macOS) | should work (untested) |
| Windows  | compiles (unix-only `libc`/process-group code is `cfg`-gated, with fallbacks); untested | Tauri is cross-platform; untested |

CI builds and tests on Linux + macOS and build-checks Windows. Reports of breakage on Linux/Windows are welcome.

## Adapting to other agents

AgentPipe wires to `claude` and `codex` today. The injection points, in order of effort:

- **Swap the binaries** (no code): `AGENTPIPE_CLAUDE_BIN` / `AGENTPIPE_CODEX_BIN` point the runners at any compatible executable (this is how the stub demo works).
- **Add a new agent runner**: `crates/engine/src/runner/` holds `claude.rs` / `codex.rs` behind `RunnerBins`; add a sibling and a `StepKind` variant in `crates/engine/src/manifest.rs`. See [CONTRIBUTING.md](CONTRIBUTING.md).

A general plugin/provider abstraction is intentionally **not** built yet (YAGNI for a two-vendor tool).

## Design docs

The repo carries its specs and plans — see [`docs/specs/`](docs/specs/) and [`docs/plans/`](docs/plans/). [`docs/research/`](docs/research/) compares AgentPipe to ccswarm and Conductor.

## License

MIT — see [LICENSE](LICENSE).
