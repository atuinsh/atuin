#!/bin/bash
# Render all test cases from test-renders.json
# Usage: ./render-tests.sh [test_name]
#   With no args: renders all tests
#   With arg: renders only matching test (e.g., ./render-tests.sh 05)

set -e
cd "$(dirname "$0")"

JSON_FILE="test-renders.json"
FILTER="${1:-}"

# Build once
cargo build -p atuin-ai --quiet

# Count tests
TOTAL=$(jq length "$JSON_FILE")

for i in $(seq 0 $((TOTAL - 1))); do
    NAME=$(jq -r ".[$i].name" "$JSON_FILE")
    DESC=$(jq -r ".[$i].description" "$JSON_FILE")
    STATE=$(jq -c ".[$i].state" "$JSON_FILE")

    # Skip if filter provided and doesn't match
    if [[ -n "$FILTER" && ! "$NAME" =~ $FILTER ]]; then
        continue
    fi

    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "[$NAME] $DESC"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "$STATE" | cargo run -p atuin-ai --quiet -- debug-render -f plain
    echo ""
done
