# Codex Compaction Viewer

[![quality](https://github.com/alexzhang1030/codex-compaction-viewer/actions/workflows/quality.yml/badge.svg)](https://github.com/alexzhang1030/codex-compaction-viewer/actions/workflows/quality.yml)
[![release](https://github.com/alexzhang1030/codex-compaction-viewer/actions/workflows/release.yml/badge.svg)](https://github.com/alexzhang1030/codex-compaction-viewer/actions/workflows/release.yml)
[![line coverage](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/alexzhang1030/codex-compaction-viewer/main/badges/coverage.json)](https://github.com/alexzhang1030/codex-compaction-viewer/actions/workflows/quality.yml)
[![latest release](https://img.shields.io/github/v/release/alexzhang1030/codex-compaction-viewer)](https://github.com/alexzhang1030/codex-compaction-viewer/releases)

Inspect local Codex JSONL sessions for context summary snapshots, token usage, and truncation policy metadata.

`cxv` is a Rust binary. It does not require Python at runtime.

The parser supports these compaction shapes:

- Codex rollout `type: "compacted"` checkpoints, reported with replacement-history metadata and nearest preceding token usage when available.
- Legacy Codex `event_msg` / `payload.type: "context_compacted"` markers, used as a fallback when no nearby rollout checkpoint exists.
- Codex `turn_context.summary` records, reported with turn id, truncation policy, and nearest preceding token usage when available.
- Claude-style raw `system/subtype=compact_boundary` records paired with the following `user/isCompactSummary=true` record, reported with boundary line, trigger, and pre-compact token count when available.

## Install

Install a prebuilt macOS binary:

```bash
curl -fsSL https://raw.githubusercontent.com/alexzhang1030/codex-compaction-viewer/main/scripts/install.sh | sh
```

Install from source if you already have Rust:

```bash
cargo install --git https://github.com/alexzhang1030/codex-compaction-viewer
```

Build a local single executable for any supported Rust target:

```bash
cargo build --release
./target/release/cxv --help
```

## Usage

Launch the interactive terminal viewer:

```bash
cxv
```

Show the installed version:

```bash
cxv --version
cxv -v
```

Open the interactive viewer for one session:

```bash
cxv --tui ~/.codex/sessions/2026/04/25/rollout-example.jsonl
```

Open the interactive viewer with the full event history:

```bash
cxv --tui --mode verbose ~/.codex/sessions/2026/04/25/rollout-example.jsonl
```

Open the interactive viewer with the raw body popup already visible:

```bash
cxv --tui --raw-bodies ~/.codex/sessions/2026/04/25/rollout-example.jsonl
```

Open the interactive viewer with native terminal text selection enabled immediately:

```bash
cxv --tui --no-mouse ~/.codex/sessions/2026/04/25/rollout-example.jsonl
```

Scan active Codex sessions:

```bash
cxv --scan
```

Include archived sessions:

```bash
cxv --scan --include-archived
```

Emit structured scan output:

```bash
cxv --scan --json
```

Show context summary snapshots for one session:

```bash
cxv --summary ~/.codex/sessions/2026/04/25/rollout-example.jsonl
```

Inspect one session:

```bash
cxv ~/.codex/sessions/2026/04/25/rollout-example.jsonl
```

### Interactive TUI

The TUI shows:

- Title bar: package version and application name.
- Left sidebar: discovered Codex session files, newest first.
- Stats panel: message count, line count, compactions, token totals, context window, source path.
- Compaction panel: highlighted compaction events and summary sizes.
- History table: tidy by default, showing only user messages, compactions, assistant responses, tool-call requests, and tool-call responses. Verbose mode shows every parsed event row. Compaction rows are marked by `*`.
- Detail panel: full selected message metadata/content, or all compaction summaries.
- Session search: `/` filters the left sidebar by project/cwd/session text. Search terms can be scoped with `project:`, `cwd:`, `session:`, or `id:`; `tag:compaction` and `has:compaction` show only sessions with compaction events.
- Raw body popup: `r` opens the selected row's raw tool-call request body, tool response body, and source payload.
- Block selection: drag inside a TUI block to select only that block's text; dragging outside the block stays clipped to the original block.

Keybindings:

| Key | Action |
| --- | --- |
| `h` / `l` or left / right | Previous / next session |
| `j` / `k` or up / down | Move through conversation history |
| `Enter` | Focus/unfocus detail; while focused, `j` / `k` scroll detail text |
| `/` | Edit session search text |
| `g` | Toggle sessions with compaction events only |
| `v` | Toggle tidy / verbose history mode |
| `m` | Toggle mouse capture; when off, terminal text selection works normally |
| `r` | Open/close raw request/response body popup |
| `y` | Copy selected block text to the terminal clipboard with OSC 52 |
| `c` / `C` | Jump to next / previous compaction point |
| `s` | Toggle all compaction summaries in the detail panel |
| `Esc` | Return from search/detail focus; quit from history focus |
| `q` | Quit |

Mouse support:

| Mouse | Action |
| --- | --- |
| Left click session | Select session |
| Left click history row | Select history row |
| Left click detail | Focus detail |
| Left drag inside block | Select text scoped to that block |
| Wheel | Move history/session selection, or scroll detail/raw popup |
| Right click raw popup | Close popup |

Mouse capture is enabled by default so the TUI can support block-scoped selection, clicks, and wheel navigation. Press `m` or start with `--no-mouse` only when you want native terminal selection as a fallback; click, wheel, and block selection are inactive while mouse capture is off.

## Data Model

- `SessionMetadata`: source file, session id, cwd, CLI version, provider.
- `ParsedMessage`: normalized event/message rows from `event_msg`, `response_item`, `turn_context`, raw role records, and other records. Tool-call rows also retain raw request/response bodies for opt-in TUI inspection.
- `CompactionEvent`: compact summary with line, optional boundary line, source type, trigger, summary text, truncation policy, and token usage.
- `ConversationStats`: line counts, bad JSON count, token totals, model context window, and time bounds.

The parser follows the same resilient JSON/JSONL loading shape used by Euphony: it accepts normal object rows, skips blank rows, tolerates malformed rows, unwraps string-encoded JSON events, and keeps canonical `response_item` messages ahead of legacy fallback `event_msg` duplicates. This keeps large or partially-written sessions inspectable while making the TUI history less noisy.

## Development

```bash
cargo test
cargo run -- --scan --include-archived
```

Quality gate with coverage and mutation testing:

```bash
bash scripts/test-quality.sh
```

This runs:

- `cargo test`
- `cargo llvm-cov --workspace --all-features --summary-only --fail-under-lines ${COVERAGE_FLOOR:-85}`
- `cargo mutants --workspace --in-place --baseline=skip --file src/cli.rs --file src/parser.rs --file src/tui.rs`
