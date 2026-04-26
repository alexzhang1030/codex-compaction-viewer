#!/usr/bin/env bash
set -euo pipefail

coverage_floor="${COVERAGE_FLOOR:-75}"
mutation_timeout="${MUTATION_TIMEOUT:-120}"

cargo test
cargo llvm-cov --workspace --all-features --summary-only --fail-under-lines "$coverage_floor"
cargo mutants \
  --workspace \
  --in-place \
  --baseline=skip \
  --timeout "$mutation_timeout" \
  --file src/cli.rs \
  --file src/parser.rs \
  --file src/tui.rs
