#!/usr/bin/env node
// Kubyl for Claude Desktop: relays newline-delimited JSON-RPC from stdin to Kubyl's local MCP
// server (POST http://127.0.0.1:<port>/mcp with the bearer token) and its answers to stdout.
// No dependencies. Never logs the token. node:http, not fetch: Kubyl refuses requests that
// carry an Origin header.
"use strict";

const http = require("node:http");
const readline = require("node:readline");

const token = process.env.KUBYL_MCP_TOKEN || "";
const port = Number.parseInt(process.env.KUBYL_MCP_PORT || "7421", 10);

if (!token) {
  process.stderr.write("kubyl: no access token; set it in the extension's settings.\n");
}

// Kubyl doesn't use sessions today; pass one through if it ever hands one out.
let session = null;

const DEADLINE_MS = 13 * 60 * 1000;

function write(message) {
  process.stdout.write(JSON.stringify(message) + "\n");
}

// Answers every request in `message` (one or a batch) with `text`, so Claude doesn't wait.
function fail(message, text) {
  for (const item of Array.isArray(message) ? message : [message]) {
    if (item && item.id !== undefined && item.id !== null) {
      write({ jsonrpc: "2.0", id: item.id, error: { code: -32603, message: text } });
    }
  }
}

function explain(status) {
  switch (status) {
    case 401:
      return "Kubyl rejected the access token. Copy it again with \"MCP: Copy Token\" (it changes on \"MCP: Regenerate Token\") and update the extension's settings.";
    case 403:
      return "Kubyl refused the request.";
    default:
      return `Kubyl answered HTTP ${status}.`;
  }
}

// Writes each `data:` event of a text/event-stream body.
function writeEvents(body) {
  for (const event of body.split(/\r?\n\r?\n/)) {
    const data = event
      .split(/\r?\n/)
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trimStart())
      .join("\n");
    if (data) {
      try {
        write(JSON.parse(data));
      } catch {
        // Not JSON-RPC; skip it.
      }
    }
  }
}

function relay(line) {
  let message;
  try {
    message = JSON.parse(line);
  } catch {
    write({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
    return;
  }
  const body = Buffer.from(line, "utf8");
  const headers = {
    "Content-Type": "application/json",
    Accept: "application/json, text/event-stream",
    Authorization: `Bearer ${token}`,
    "Content-Length": body.length,
  };
  if (session) headers["Mcp-Session-Id"] = session;

  // A change can take 12 minutes (a 1 minute check, up to 10 minutes for the user to answer
  // Kubyl's approval dialog, a 1 minute run): give up a minute after that. No agent: an idle
  // keep-alive socket would keep the process alive after stdin closes.
  const request = http.request(
    { host: "127.0.0.1", port, path: "/mcp", method: "POST", headers, agent: false },
    (response) => {
      const chunks = [];
      response.on("data", (chunk) => chunks.push(chunk));
      response.on("end", () => {
        const status = response.statusCode || 0;
        const text = Buffer.concat(chunks).toString("utf8");
        if (response.headers["mcp-session-id"]) session = response.headers["mcp-session-id"];
        if (status === 202 || status === 204) return;
        if (status < 200 || status >= 300) {
          fail(message, explain(status));
          return;
        }
        if (String(response.headers["content-type"] || "").includes("text/event-stream")) {
          writeEvents(text);
        } else if (text.trim()) {
          try {
            write(JSON.parse(text));
          } catch {
            fail(message, "Kubyl sent an answer that isn't JSON.");
          }
        }
      });
    },
  );
  const deadline = setTimeout(() => {
    request.destroy(Object.assign(new Error("no answer"), { code: "ETIMEDOUT" }));
  }, DEADLINE_MS);
  request.on("close", () => clearTimeout(deadline));
  request.on("error", (err) => {
    const text =
      err.code === "ECONNREFUSED"
        ? `Kubyl isn't reachable on 127.0.0.1:${port}. Start Kubyl and check that its MCP server is on ("MCP: Show Server").`
        : err.code === "ETIMEDOUT"
          ? "Kubyl didn't answer within 13 minutes."
          : `Kubyl isn't reachable: ${err.message}`;
    fail(message, text);
  });
  request.end(body);
}

const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
input.on("line", (line) => {
  if (line.trim()) relay(line);
});
// Node exits by itself once stdin has closed and the last answer is written.
