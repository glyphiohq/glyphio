# Glyphio documentation

This directory contains the maintained product and engineering documentation for Glyphio.
Start with the document that matches the job at hand; avoid creating phase notes or duplicate
setup guides.

| Need | Document |
| --- | --- |
| Understand the system, its boundaries, and the important design decisions | [Architecture](ARCHITECTURE.md) |
| Plan work or turn a proposal into a spec and implementation tickets | [Roadmap and delivery map](ROADMAP.md) |
| Install a released macOS build | [Installation](INSTALL.md) |
| Assess product and deployment security | [Security posture](SECURITY.md) |
| Implement a compatible sync service or client | [Sync protocol v1](SYNC-PROTOCOL.md) |
| Scope a Windows port | [Windows port](WINDOWS.md) |

Repository-level guides live alongside the code they describe:

- [Project overview](../README.md) and [contributing](../CONTRIBUTING.md)
- [Operator and self-hosting setup](../SETUP.md)
- [Reference sync server](../server/README.md)
- [AWS reference deployment](../infra/README.md)

## Documentation rules

- Keep durable facts in the document listed above. Update the relevant document in the same
  change as its implementation.
- Put protocol compatibility requirements in `SYNC-PROTOCOL.md`; changes must follow its versioning
  rules.
- Put proposed work in `ROADMAP.md` until it has an approved specification and issue tracker
  tickets. Link approved specifications directly from the roadmap; do not create empty routing
  pages or use numbered "phase" files as a planning system.
- Record a material, long-lived technical decision in `ARCHITECTURE.md` and link to the code or
  specification that enforces it.
