# Tally

Encrypted multi-agent runtime built on [Skytale](https://github.com/nicholasraimbault/skytale).

## Status

Early development. Not ready for use. Skeleton structure only.

## What this will be

Tally is a serverless runtime for ephemeral encrypted AI agent teams. Built on Skytale's MLS-encrypted communication primitives, Tally provides:

- Wake-on-task ephemeral executors (Cloudflare Workers + Durable Objects)
- Role-pack-driven agent configuration
- Cross-machine team coordination with end-to-end encryption
- Audit trails of agent actions

## Packages

This repository is a monorepo with the following packages:

- `packages/runtime-sdk/` — The runtime SDK that ephemeral executors use
- `packages/role-pack/` — Role pack format library (parser, validator)
- `packages/mcp-plugin/` — Claude Code MCP plugin entry point
- `packages/cli/` — Tally CLI for end users

All packages are Apache 2.0 licensed.

## License

Apache 2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).

## Related projects

- [skytale](https://github.com/nicholasraimbault/skytale) — Underlying encryption primitives, SDK, relay
- [skytale-app](https://github.com/nicholasraimbault/skytale-app) — Skytale platform dashboard
