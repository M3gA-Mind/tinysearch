#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
python3 scripts/test-e2e-version.py
cargo test --locked --workspace
cargo test --locked --workspace --all-features
cargo build --locked -p tinysearch --features static-link
cargo test --locked -p tinysearch --features static-link --lib linked_
cargo run --locked -p tinysearch --example basic
forbidden="$(cargo tree -p tinysearch-bus -e normal,build --prefix none | grep -Ei 'tinybus|tokio|reqwest|ureq|hyper|rusqlite|git2' || true)"
[[ -z "$forbidden" ]] || { echo "$forbidden" >&2; exit 1; }
case "${RUNNER_OS:-$(uname -s)}" in
  Windows|MINGW*|MSYS*|CYGWIN*) ;;
  *) python3 scripts/test-package-layout.py ;;
esac
