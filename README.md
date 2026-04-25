# Codex Compaction Viewer

Inspect local Codex JSONL sessions for context summary snapshots, token usage, and truncation policy metadata.

Codex does not currently write a Claude-style `compact_boundary` record. This tool treats non-empty `turn_context.summary` records as the Codex-specific compaction signal and reports the surrounding token/truncation metadata when available.

## Install

```bash
pip install .
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
- `ParsedMessage`: normalized event/message rows from `event_msg`, `response_item`, `turn_context`, and other records.
- `CompactionEvent`: non-empty `turn_context.summary` with line, timestamp, turn id, summary text, truncation policy, and nearest preceding token usage.
- `ConversationStats`: line counts, bad JSON count, token totals, model context window, and time bounds.

The parser streams JSONL line-by-line and tolerates bad JSON rows so large or partially-written sessions remain inspectable.

## Development

```bash
PYTHONPATH=src python3 -m unittest discover -s tests -v
PYTHONPATH=src python3 -m codex_compaction_viewer --scan --include-archived
```

