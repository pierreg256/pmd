#!/usr/bin/env bash
#
# Launch a 10-node PMD cluster on localhost.
#
# Usage:
#   ./examples/cluster_10.sh          # uses ./target/release/pmd
#   ./examples/cluster_10.sh debug    # uses ./target/debug/pmd
#
# To tear down:
#   ./examples/cluster_10.sh stop
#

set -euo pipefail

BASE_PORT=4369
NODE_COUNT=10

if [[ "${1:-}" == "debug" ]]; then
    PMD="./target/debug/pmd"
elif [[ "${1:-}" == "stop" ]]; then
    PMD="${PMD:-./target/release/pmd}"
    echo "Stopping $NODE_COUNT daemons (ports $BASE_PORT-$((BASE_PORT + NODE_COUNT - 1)))..."
    for i in $(seq 0 $((NODE_COUNT - 1))); do
        port=$((BASE_PORT + i))
        "$PMD" stop --port "$port" 2>/dev/null && echo "  stopped port $port" || echo "  port $port not running"
    done
    exit 0
else
    PMD="./target/release/pmd"
fi

echo "=== Launching $NODE_COUNT PMD daemons (ports $BASE_PORT-$((BASE_PORT + NODE_COUNT - 1))) ==="
echo ""

for i in $(seq 0 $((NODE_COUNT - 1))); do
    port=$((BASE_PORT + i))
    "$PMD" start --port "$port" --bind 127.0.0.1
    echo "  started port $port"
done

# Give daemons a moment to bind their control sockets
sleep 1

echo ""
echo "=== Connecting all nodes to the first node (port $BASE_PORT) ==="
echo ""

FIRST_ADDR="127.0.0.1:$BASE_PORT"
for i in $(seq 1 $((NODE_COUNT - 1))); do
    port=$((BASE_PORT + i))
    "$PMD" join "$FIRST_ADDR" --port "$port"
    echo "  port $port → joined $FIRST_ADDR"
done

# Wait for gossip to propagate across all nodes
echo ""
echo "Waiting for gossip convergence (15s)..."
sleep 15

echo ""
echo "=== Cluster status (from node on port $BASE_PORT) ==="
echo ""
"$PMD" nodes --port "$BASE_PORT"

echo ""
echo "=== Cluster status (from node on port $((BASE_PORT + NODE_COUNT - 1))) ==="
echo ""
"$PMD" nodes --port "$((BASE_PORT + NODE_COUNT - 1))"

echo ""
echo "Done. To stop all daemons: $0 stop"
