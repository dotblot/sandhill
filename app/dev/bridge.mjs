// Development only: lets the UI run in an ordinary browser (`npm run dev`)
// against a real sandhilld, so it can be tried and tested without building
// the Mac app. Each WebSocket gets its own `sandhilld attach --start`, which
// is exactly what the Mac app runs. /blob/<hash> serves outputs' blobs the
// way the app's sandhill-blob:// protocol does.
//
//   SANDHILLD=../target/debug/sandhilld node dev/bridge.mjs

import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { createInterface } from "node:readline";
import { WebSocketServer } from "ws";

const port = Number(process.env.BRIDGE_PORT ?? 4639);
const daemon = process.env.SANDHILLD ?? new URL("../../target/debug/sandhilld", import.meta.url).pathname;
const extraArgs = process.env.SANDHILL_STATE_DIR ? ["--state-dir", process.env.SANDHILL_STATE_DIR] : [];

function attach(onLine) {
  const child = spawn(daemon, [...extraArgs, "attach", "--start"], { stdio: ["pipe", "pipe", "inherit"] });
  createInterface({ input: child.stdout }).on("line", onLine);
  return child;
}

// One shared connection for blob requests.
let blobId = 0;
const blobWaiters = new Map();
const blobConn = attach((line) => {
  const msg = JSON.parse(line);
  if (msg.type !== "response") return;
  blobWaiters.get(msg.id)?.(msg);
  blobWaiters.delete(msg.id);
});

function getBlob(hash, near) {
  const id = ++blobId;
  return new Promise((resolve) => {
    blobWaiters.set(id, resolve);
    blobConn.stdin.write(JSON.stringify({ id, method: "get_blob", params: { hash, near } }) + "\n");
  });
}

const BINARY = (mime) => (mime.startsWith("image/") && mime !== "image/svg+xml") || mime === "application/pdf";

const server = createServer(async (req, res) => {
  const url = new URL(req.url, `http://${req.headers.host}`);
  const match = /^\/blob\/([0-9a-f]{64})$/.exec(url.pathname);
  if (!match) {
    res.writeHead(404).end();
    return;
  }
  const mime = url.searchParams.get("mime") ?? "application/octet-stream";
  const msg = await getBlob(match[1], url.searchParams.get("near") ?? undefined);
  if (msg.error) {
    res.writeHead(404, { "content-type": "text/plain" }).end(msg.error);
    return;
  }
  const body = BINARY(mime) ? Buffer.from(msg.result.data, "base64") : Buffer.from(msg.result.data);
  res.writeHead(200, {
    "content-type": mime,
    "cache-control": "max-age=31536000, immutable",
    "access-control-allow-origin": "*",
  });
  res.end(body);
});

new WebSocketServer({ server }).on("connection", (ws) => {
  const child = attach((line) => ws.send(line));
  child.on("exit", () => ws.close());
  ws.on("message", (data) => child.stdin.write(`${data}\n`));
  ws.on("close", () => child.kill());
});

server.listen(port, "127.0.0.1", () => console.log(`bridge on ws://127.0.0.1:${port} -> ${daemon}`));
