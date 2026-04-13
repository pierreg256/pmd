#!/usr/bin/env node
//
// PMD Unix Socket Client — JavaScript Example
//
// Demonstrates how to interact with a running PMD daemon from JavaScript
// using the Unix domain socket (JSON protocol). No dependencies required.
//
// Usage:
//   node examples/pmd_client.js                  # status + nodes on default port
//   node examples/pmd_client.js --port 4370      # talk to daemon on port 4370
//   node examples/pmd_client.js subscribe        # stream membership events
//   node examples/pmd_client.js lookup myapp     # look up a service
//

const net = require("net");
const os = require("os");
const path = require("path");

// ---------------------------------------------------------------------------
// Parse CLI arguments
// ---------------------------------------------------------------------------
const args = process.argv.slice(2);
let port = 4369;
let command = null;
let commandArg = null;

for (let i = 0; i < args.length; i++) {
  if (args[i] === "--port" && args[i + 1]) {
    port = parseInt(args[i + 1], 10);
    i++;
  } else if (!command) {
    command = args[i];
  } else if (!commandArg) {
    commandArg = args[i];
  }
}

const socketPath = path.join(os.homedir(), ".pmd", `pmd-${port}.sock`);

// ---------------------------------------------------------------------------
// Send a JSON request to the daemon and return the parsed response
// ---------------------------------------------------------------------------
function sendRequest(request) {
  return new Promise((resolve, reject) => {
    const client = net.createConnection(socketPath, () => {
      client.write(JSON.stringify(request) + "\n");
    });

    let data = "";
    client.on("data", (chunk) => {
      data += chunk.toString();
      // Response is a single JSON line
      const newline = data.indexOf("\n");
      if (newline !== -1) {
        const line = data.slice(0, newline);
        client.destroy();
        try {
          resolve(JSON.parse(line));
        } catch (e) {
          reject(new Error(`Invalid JSON: ${line}`));
        }
      }
    });

    client.on("error", (err) => {
      reject(new Error(`Cannot connect to PMD daemon (${socketPath}): ${err.message}`));
    });
  });
}

// ---------------------------------------------------------------------------
// Subscribe: long-lived connection streaming events
// ---------------------------------------------------------------------------
function subscribe() {
  const client = net.createConnection(socketPath, () => {
    console.log(`Connected to PMD daemon (${socketPath}), streaming events...`);
    client.write(JSON.stringify("Subscribe") + "\n");
  });

  let buffer = "";
  client.on("data", (chunk) => {
    buffer += chunk.toString();
    let newline;
    while ((newline = buffer.indexOf("\n")) !== -1) {
      const line = buffer.slice(0, newline);
      buffer = buffer.slice(newline + 1);
      if (line.trim()) {
        try {
          const event = JSON.parse(line);
          if (event === "Ok") {
            console.log("Subscribed. Waiting for events...\n");
          } else if (event.Event) {
            const e = event.Event;
            const time = new Date().toISOString();
            console.log(`[${time}] ${e.event}: node=${e.node_id} addr=${e.addr}`);
          }
        } catch {
          console.error("Unparseable line:", line);
        }
      }
    }
  });

  client.on("error", (err) => {
    console.error(`Connection error: ${err.message}`);
    process.exit(1);
  });

  client.on("close", () => {
    console.log("Connection closed.");
    process.exit(0);
  });
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------
function printStatus(resp) {
  if (resp.Status) {
    const s = resp.Status;
    console.log(`Node ID:    ${s.node_id}`);
    console.log(`Listen:     ${s.listen_addr}`);
    console.log(`Peers:      ${s.peer_count}`);
    console.log(`Nodes:      ${s.node_count}`);
  }
}

function printNodes(resp) {
  if (resp.Nodes) {
    const nodes = resp.Nodes.nodes;
    if (nodes.length === 0) {
      console.log("No nodes in cluster.");
      return;
    }
    console.log(
      "NODE ID".padEnd(40) +
      "ADDRESS".padEnd(22) +
      "LAST SEEN".padEnd(26) +
      "PHI"
    );
    for (const n of nodes) {
      let status;
      if (n.is_local) {
        status = "(local)";
      } else if (n.last_seen_at) {
        status = new Date(n.last_seen_at * 1000).toISOString().replace("T", " ").replace(".000Z", "Z");
      } else {
        status = "(indirect)";
      }
      const phi = n.phi != null ? n.phi.toFixed(2) : "-";
      console.log(
        n.node_id.padEnd(40) +
        n.addr.padEnd(22) +
        status.padEnd(26) +
        phi
      );
      for (const s of n.services || []) {
        console.log(`  └─ ${s.name} → ${s.host}:${s.port}`);
      }
    }
  }
}

function printServices(resp) {
  if (resp.Services) {
    const entries = resp.Services.entries;
    if (entries.length === 0) {
      console.log("No services found.");
      return;
    }
    for (const s of entries) {
      console.log(`${s.name} → ${s.host}:${s.port} (node: ${s.node_id})`);
    }
  }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------
async function main() {
  try {
    if (command === "subscribe") {
      subscribe();
      return; // long-lived, exits on close/signal
    }

    if (command === "lookup" && commandArg) {
      const resp = await sendRequest({ Lookup: { name: commandArg } });
      printServices(resp);
      return;
    }

    if (command === "nodes") {
      const resp = await sendRequest("Nodes");
      printNodes(resp);
      return;
    }

    if (command === "status") {
      const resp = await sendRequest("Status");
      printStatus(resp);
      return;
    }

    // Default: show status + nodes
    const status = await sendRequest("Status");
    printStatus(status);
    console.log("");
    const nodes = await sendRequest("Nodes");
    printNodes(nodes);
  } catch (err) {
    console.error(err.message);
    process.exit(1);
  }
}

main();
