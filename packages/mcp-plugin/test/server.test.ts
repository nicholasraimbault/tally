import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "../src/index.js";

test("createServer returns a Server instance", () => {
  const server = createServer();
  assert.ok(server, "Server should be created");
});

test("createServer registers tools", async () => {
  const server = createServer();
  // The Server's request handlers are internal; we verify the server
  // construct succeeds and is non-null. Deeper integration tests come
  // in Phase 1B when the real tool surface exists.
  assert.ok(server);
});
