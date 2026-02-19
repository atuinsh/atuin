#!/bin/bash
# Replay state snapshots from a debug state JSONL file
# Usage: ./replay-states.sh <state-file.jsonl> [entry-number]
#   With no entry: renders all frames in sequence (press Enter to advance)
#   With entry number: renders just that frame

set -e
# cd "$(dirname "$0")"

STATE_FILE="${1:-}"
ENTRY_FILTER="${2:-}"

if [[ -z "$STATE_FILE" ]]; then
    echo "Usage: $0 <state-file.jsonl> [entry-number]"
    echo ""
    echo "Examples:"
    echo "  $0 /tmp/state.jsonl          # Interactive replay of all frames"
    echo "  $0 /tmp/state.jsonl 15       # Show just entry 15"
    exit 1
fi

if [[ ! -f "$STATE_FILE" ]]; then
    echo "Error: File not found: $STATE_FILE"
    exit 1
fi

# Build once
cargo build -p atuin-ai --quiet

# Count entries
TOTAL=$(wc -l < "$STATE_FILE" | tr -d ' ')

if [[ -n "$ENTRY_FILTER" ]]; then
    # Show single entry
    LINE=$(sed -n "${ENTRY_FILTER}p" "$STATE_FILE")
    if [[ -z "$LINE" ]]; then
        echo "Error: Entry $ENTRY_FILTER not found (file has $TOTAL entries)"
        exit 1
    fi

    ENTRY=$(echo "$LINE" | jq -r '.entry')
    LABEL=$(echo "$LINE" | jq -r '.label')
    STATE=$(echo "$LINE" | jq -c '.state')

    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "[$ENTRY/$TOTAL] $LABEL"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    echo "$STATE" | cargo run -p atuin-ai --quiet -- debug-render -f plain
else
    # Interactive replay
    echo "Replaying $TOTAL frames from $STATE_FILE"
    echo "Press Enter to advance, 'q' to quit, or number+Enter to jump"
    echo ""

    CURRENT=1
    while [[ $CURRENT -le $TOTAL ]]; do
        LINE=$(sed -n "${CURRENT}p" "$STATE_FILE")
        ENTRY=$(echo "$LINE" | jq -r '.entry')
        LABEL=$(echo "$LINE" | jq -r '.label')
        STATE=$(echo "$LINE" | jq -c '.state')

        clear
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "[$CURRENT/$TOTAL] $LABEL"
        echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
        echo "$STATE" | cargo run -p atuin-ai --quiet -- debug-render -f plain
        echo ""
        echo "[Enter: next] [p: prev] [number: jump] [s: show state JSON] [q: quit]"

        read -r INPUT
        case "$INPUT" in
            q|Q)
                break
                ;;
            p|P)
                if [[ $CURRENT -gt 1 ]]; then
                    CURRENT=$((CURRENT - 1))
                fi
                ;;
            s|S)
                echo ""
                echo "State JSON:"
                echo "$STATE" | jq .
                echo ""
                echo "Press Enter to continue..."
                read -r
                ;;
            ''|' ')
                CURRENT=$((CURRENT + 1))
                ;;
            *[0-9]*)
                if [[ "$INPUT" =~ ^[0-9]+$ ]] && [[ "$INPUT" -ge 1 ]] && [[ "$INPUT" -le $TOTAL ]]; then
                    CURRENT=$INPUT
                else
                    echo "Invalid entry number (1-$TOTAL)"
                    sleep 1
                fi
                ;;
        esac
    done
fi
