# Tally Workers

Cloudflare-hosted runtime for the Stoa agent-coordination protocol. The dispatch substrate that powers [Tally Coding](https://github.com/nicholasraimbault/tally-coding) and (potentially) other Tally products.

Tally Workers implements [Stoa](https://github.com/nicholasraimbault/skytale)'s
`WakeRouter` trait on Cloudflare Workers + Durable Objects. It's the first
product on the Stoa protocol layer extracted via Option B (skytale issue
[#431](https://github.com/nicholasraimbault/skytale/issues/431)).

## Sibling products

- **[Tally Coding](https://github.com/nicholasraimbault/tally-coding)** — privacy-first AI coding team platform; native apps on every device; cloud-primary agents; E2E-encrypted team chat. Consumes Tally Workers as the wake-routing substrate. BSL 1.1 (same license family as this repo).
- **[Skytale](https://github.com/nicholasraimbault/skytale)** — open-source primitives (Apache 2.0). E2E encryption for AI agents (MLS RFC 9420), SDKs (Python, Rust, TypeScript), relay, REST API, and the Stoa protocol-interface crate that Tally Workers implements.

## Status

In active development as Phase 1B of the Pronoic agent-substrate roadmap.
Tracked in skytale issue [#444](https://github.com/nicholasraimbault/skytale/issues/444).

See `docs/specs/phase-1b-spec.md` for the canonical Phase 1B implementation
reference and `docs/specs/phase-1b-sub-pr-1-phase-0.md` for the Sub-PR 1
design notes.

## License

[Business Source License 1.1](./LICENSE). Converts to [Apache 2.0](./LICENSES/Apache-2.0)
on 2030-05-13. Additional Use Grant permits all internal use; the only
restriction is providing Tally to third parties as a hosted or managed service
without commercial agreement.

## History

This repo's main branch was reset on 2026-05-13 to match the post-Stoa-extraction
Phase 1B Phase 0 spec. The original 2026-04-30 scaffolding (pre-Stoa, Apache 2.0,
different decomposition) is preserved at tag `v0-archive-2026-04-30` for
retrieval-only access.

## Repository

This repo contains the Tally runtime (Workers + Durable Objects code), Tally
CLI (Rust binary; added in Sub-PR 3 of Phase 1B), and documentation. The Tally
MCP plugin (`@skytalesh/tally-mcp`, npm-distributed) is in a separate package
landing in Sub-PR 2.
