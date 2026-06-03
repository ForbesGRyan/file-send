# Testing

Three layers, each proving something the others can't. See the design spec:
`docs/superpowers/specs/2026-06-03-testing-strategy-design.md`.

## Layers
- **Server / signaling** (`crates/server`) — protocol & room logic. `cargo test -p server`.
- **Client reducer & pure logic** (`crates/client`) — reconnect decisions, transfer
  state, protocol framing. `cargo test -p client`.
- **E2E** (`e2e/`) — two real browser peers over real WebRTC against the real
  server. `cd e2e && npx playwright test`.

## Marked-failing backlog (executable bug list)
These tests are committed but expected to fail until the underlying gap is fixed.
A fix is not "done" until its marker is removed and the test runs green in the
required gate.

| Test | Marker | Gap it pins | Fix lands by |
|---|---|---|---|
| `transfer_state::finalize_rejects_mismatched_id` | `#[ignore]` | Receiver doesn't validate `FileEnd.id` against the open file | Implement `finalize_decision_checked` id check; route `Control::End` through it |
| `rooms::reap_idle_expires_lonely_room` | `#[ignore]` | Idle single-peer rooms never expire (no TTL) | Implement `reap_idle`; call it from a server timer; `touch` on relay activity |
| `sleep-reconnect.spec.ts` | `test.fixme` | Offline/sleep peer doesn't reliably reconnect | Fix sleep/reconnect recovery |
| `limits.spec.ts` symmetric-NAT | `test.fixme` | Direct-connect failure UX unverifiable on loopback | Add TURN-less/netem ICE-failure harness |

Run the Rust backlog: `cargo test -- --ignored` (non-blocking `ignored-backlog`
CI job). Run the E2E fixmes: they auto-skip; un-skip locally to drive a fix.
