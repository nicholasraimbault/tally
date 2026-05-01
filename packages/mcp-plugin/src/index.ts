#!/usr/bin/env node
/**
 * Tally MCP Plugin
 *
 * Claude Code MCP plugin that exposes Tally team coordination tools.
 * Distributed via npm as @skytale/tally-mcp.
 *
 * Status: skeleton. The real implementation comes in Phase 1B —
 * this version exposes a single ping tool to verify the plugin
 * can be installed, connected to, and called from Claude Code.
 *
 * See: https://github.com/nicholasraimbault/tally
 */

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { CallToolRequestSchema, ListToolsRequestSchema } from "@modelcontextprotocol/sdk/types.js";

const VERSION = "0.1.0";

export function createServer(): Server {
  const server = new Server(
    {
      name: "tally-mcp",
      version: VERSION,
    },
    {
      capabilities: {
        tools: {},
      },
    }
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: [
      {
        name: "tally_ping",
        description:
          "Returns 'pong' to verify the Tally MCP plugin is connected. " +
          "Skeleton tool; real Tally tools come in Phase 1B.",
        inputSchema: {
          type: "object",
          properties: {},
          required: [],
        },
      },
    ],
  }));

  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const { name } = request.params;

    if (name === "tally_ping") {
      return {
        content: [
          {
            type: "text",
            text: `pong (tally-mcp v${VERSION})`,
          },
        ],
      };
    }

    throw new Error(`Unknown tool: ${name}`);
  });

  return server;
}

async function main(): Promise<void> {
  const server = createServer();
  const transport = new StdioServerTransport();
  await server.connect(transport);

  // Stay alive — server runs until stdin closes
  process.stdin.resume();
}

// Only run main() when executed directly, not when imported by tests
if (process.argv[1]?.endsWith("index.js") || process.argv[1]?.endsWith("index.ts")) {
  main().catch((err) => {
    console.error("Fatal error:", err);
    process.exit(1);
  });
}
