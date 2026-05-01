# @skytale/tally-mcp

Claude Code MCP plugin for Tally — encrypted multi-agent team coordination.

## Status

Skeleton. Real implementation comes in Phase 1B.

## Install

```bash
claude mcp add tally -- npx -y @skytale/tally-mcp@latest
```

## What this will do

Once the runtime is implemented in Phase 1B, this plugin will let Claude Code:

- Create and join encrypted multi-agent teams
- Assign tasks to teammates with role-pack-defined behavior
- Read team-shared context with cryptographic attribution
- Coordinate across machines via Skytale's MLS-encrypted relay

## Current state

Exposes one tool: `tally_ping` (returns "pong"). Use this to verify the plugin is correctly installed and connected to Claude Code.

## License

Apache 2.0. See [LICENSE](../../LICENSE).
