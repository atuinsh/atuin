#!/usr/bin/env bash
# Author michael <themichaeleden@gmail.com>
set -euo pipefail
set -x
SOURCE_DIR=$(readlink -f "${BASH_SOURCE[0]}")
SOURCE_DIR=$(dirname "$SOURCE_DIR")
cd "${SOURCE_DIR}/.."

function cleanup() {
    kill -9 ${FUZZINGSERVER_PID}
}
trap cleanup TERM EXIT

function test_diff() {
    if ! diff -q \
        <(jq -S 'del(."Tungstenite" | .. | .duration?)' 'autobahn/client-results.json') \
        <(jq -S 'del(."Tungstenite" | .. | .duration?)' 'autobahn/client/index.json')
    then
        echo 'Difference in results, either this is a regression or' \
             'one should update autobahn/client-results.json with the new results.' \
             'The results are:'
        exit 64
    fi
}

cargo build --release --example autobahn-client

wstest -m fuzzingserver -s 'autobahn/fuzzingserver.json' & FUZZINGSERVER_PID=$!
sleep 3
echo "Server PID: ${FUZZINGSERVER_PID}"
cargo run --release --example autobahn-client
test_diff
