# Codex Compaction Viewer

Inspect local Codex JSONL sessions for context summary snapshots, token usage, and truncation policy metadata.

The parser supports two compaction shapes:

- Codex `turn_context.summary` records, reported with turn id, truncation policy, and nearest preceding token usage when available.
- Claude-style raw `system/subtype=compact_boundary` records paired with the following `user/isCompactSummary=true` record, reported with boundary line, trigger, and pre-compact token count when available.

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
- `ParsedMessage`: normalized event/message rows from `event_msg`, `response_item`, `turn_context`, raw role records, and other records.
- `CompactionEvent`: compact summary with line, optional boundary line, source type, trigger, summary text, truncation policy, and token usage.
- `ConversationStats`: line counts, bad JSON count, token totals, model context window, and time bounds.

The parser streams JSONL line-by-line and tolerates bad JSON rows so large or partially-written sessions remain inspectable.

## Development

```bash
PYTHONPATH=src python3 -m unittest discover -s tests -v
PYTHONPATH=src python3 -m codex_compaction_viewer --scan --include-archived
```
