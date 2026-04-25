# Codex Compaction Viewer

Inspect local Codex JSONL sessions for context summary snapshots, token usage, and truncation policy metadata.

`cxv` is a Rust binary. It does not require Python at runtime.

The parser supports two compaction shapes:

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

## Data Model

- `SessionMetadata`: source file, session id, cwd, CLI version, provider.
- `ParsedMessage`: normalized event/message rows from `event_msg`, `response_item`, `turn_context`, raw role records, and other records.
- `CompactionEvent`: compact summary with line, optional boundary line, source type, trigger, summary text, truncation policy, and token usage.
- `ConversationStats`: line counts, bad JSON count, token totals, model context window, and time bounds.

The parser streams JSONL line-by-line and tolerates bad JSON rows so large or partially-written sessions remain inspectable.

## Development

```bash
cargo test
cargo run -- --scan --include-archived
```
