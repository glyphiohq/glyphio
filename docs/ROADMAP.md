# Glyphio roadmap and delivery map

This is the single intake and planning document for future work. It distinguishes an idea from
an approved specification and from an implementation ticket, so planned work does not become an
ambiguous promise.

## Status vocabulary

| Status | Meaning |
| --- | --- |
| `discovery` | Problem is known; scope and design are not approved. |
| `specified` | Acceptance criteria, risks, and compatibility impact are documented. |
| `ready` | Tickets are small enough to implement and dependencies are resolved. |
| `in progress` | An assignee is actively delivering the ticket set. |
| `released` | Shipped and release verification is recorded. |

## How work flows

```text
Issue / opportunity → product or technical spec → implementation tickets → PRs → release verification
       (problem)             (decision + acceptance)        (owned work)        (evidence)
```

1. Open one issue for the user, reliability, security, or platform problem.
2. Create a short spec when the work changes architecture, data, protocol, security, UX flow, or
   spans more than one ticket. Link it from the issue.
3. Split the approved spec into independently reviewable tickets. Each ticket links back to both
   its issue and spec, names dependencies, and has testable acceptance criteria.
4. Update this map when work moves status; do not preserve plans in separate phase documents.

## Required issue and ticket fields

| Item | Required fields |
| --- | --- |
| Issue | Problem, impact, owner, priority, area, evidence, desired outcome |
| Spec | Context, goals/non-goals, proposed design, data/API impact, security/privacy impact, migration/rollback, acceptance criteria |
| Ticket | Parent issue/spec, scope, dependency, implementation notes, tests, definition of done |

Recommended labels: `area:desktop`, `area:engine`, `area:sync`, `area:server`, `area:infra`,
`area:ui`, `area:docs`; plus `type:feature`, `type:bug`, `type:security`, `type:chore`, and
`priority:p0` through `priority:p3`.

## Portfolio map

The IDs below are stable planning identifiers until the project creates corresponding tracker
issues (for example, `GH-001`). Replace the placeholder in the **Tracker** column rather than
renumbering the work item.

| ID | Initiative | Status | Priority | Spec / source | Ticket sequence | Dependencies |
| --- | --- | --- | --- | --- | --- | --- |
| PLAT-01 | Windows support | discovery | p2 | [Windows port](WINDOWS.md) | WIN-01 compile seams → WIN-02 engine build → WIN-03 non-interactive capture → WIN-04 picker → WIN-05 UIA/scroll/OCR | Windows CI and signing decision |
| REL-01 | Production release readiness | discovery | p1 | [Installation](INSTALL.md), [Security](SECURITY.md) | REL-01 signing/notarization → REL-02 release pipeline → REL-03 managed-device validation → REL-04 release checklist | Developer ID, distribution authority |
| SYNC-01 | End-to-end encryption feasibility | discovery | p2 | [Sync protocol](SYNC-PROTOCOL.md) trust model | ENC-01 threat/design study → ENC-02 envelope/protocol proposal → ENC-03 migration plan → ENC-04 implementation | Explicit product decision; protocol versioning |
| CAP-01 | Browser-page capture | discovery | p2 | Architecture capture boundary | CAP-01 browser support study → CAP-02 consent/permission UX spec → CAP-03 implementation → CAP-04 privacy test plan | Browser integration choice |
| SNIP-01 | Content-addressed snippet assets | discovery | p3 | Architecture local/sync data boundary | AST-01 size/retention study → AST-02 storage spec → AST-03 migration → AST-04 transfer implementation | Sync payload and storage design |
| QA-01 | Release-grade macOS verification | ready | p1 | [Architecture](ARCHITECTURE.md) verification | QA-01 permission/expansion matrix → QA-02 capture/editor matrix → QA-03 OIDC + two-device sync → QA-04 deployment smoke test | Test Macs, test IdP, deployment credentials |

## Initiative briefs

### PLAT-01 — Windows support

Goal: ship a native Windows build without weakening the local-first and capture quality
guarantees. The implementation order and platform risks are maintained in
[Windows port](WINDOWS.md). The interactive multi-monitor picker is the primary technical
risk; it must have its own specification and acceptance tests before implementation begins.

### REL-01 — Production release readiness

Goal: establish a signed, notarized, reproducible release path and validate it on managed Macs.
This is operational work, not a change to the product's local-first behavior. It also needs an
explicit OSS/legal review before broad distribution of the bundled GPL engine fork.

### SYNC-01 — End-to-end encryption feasibility

Goal: decide whether team snippet bodies can be encrypted from the sync service. This requires a
new design rather than an incremental server change because current LWW metadata, restricted
group access, search/admin behavior, recovery, and key rotation all affect the protocol.

### CAP-01 — Browser-page capture

Goal: capture web content beyond the visible viewport without silently gaining browser access or
weakening privacy. The discovery spec must compare a browser extension, browser automation, and
the current native capture modes, including consent, supported browsers, and data boundaries.

### SNIP-01 — Content-addressed snippet assets

Goal: replace large inline image payloads only if real usage shows the current size limits are
insufficient. The spec must define deduplication, local retention, transfer authorization,
encryption posture, and behavior for incompatible servers.

### QA-01 — Release-grade macOS verification

Goal: convert the remaining permission-gated checks into a repeatable release matrix. A release
is not complete until expansion, scoped snippets, Retina capture/editing, OIDC, two-device sync,
and the chosen deployment path have recorded evidence.

## Spec template

Create a spec in `docs/specs/<topic>.md` only after discovery is accepted. Use this outline:

```md
# <Topic>

Status: discovery | specified | approved | superseded
Owner: <name>
Parent issue: <tracker URL>

## Problem and goals
## Non-goals
## Proposed design
## Data, protocol, and compatibility impact
## Security and privacy review
## Migration and rollback
## Alternatives considered
## Acceptance criteria
## Ticket breakdown
```

## Ticket definition of done

- The ticket links to its parent issue and, when required, its approved spec.
- Tests cover the changed behavior at the appropriate layer.
- User-facing, operational, protocol, security, and architecture docs are updated when affected.
- Migration, rollback, and compatibility behavior are verified where applicable.
- The PR includes evidence for the acceptance criteria.
