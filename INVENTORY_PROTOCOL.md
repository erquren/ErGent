# Retained history and bounded inventory transfers

## Compatibility and rollout

Inventory chunking was introduced in `ergent.preview.v2`; terminal recovery now
uses `ergent.preview.v3` for both Agent and browser connections. Server, Agent and web assets must be upgraded together.
An old peer receives HTTP 426 `PROTOCOL_UNSUPPORTED` instead of a successful
connection with an incomplete list. A refreshed browser loads the new web assets;
old Agents must be upgraded. There is no fallback that truncates or drops history.
The protocol remains JSON; this is separate from the planned binary frame codec.

There is no database migration and no historical metadata deletion. Back up the
existing database before upgrades as usual. Upgrading/restarting an Agent cannot
preserve its existing OS processes; finish or save work before restarting it.

## Lifetime versus concurrent sessions

The Agent keeps each session's metadata and original create config throughout its
daemon lifetime. Reusing a session ID with that config returns the same record,
even after exit or removal of the old working directory; conflicting configs fail.
Exited sessions no longer count against the 20 concurrent-runtime limit. The
former 1,000 lifetime-create limit has been removed. After confirmed process exit,
PTY master/writer/killer handles, the VT parser, input leases and deduplication
caches are released. Existing viewers may retain bounded final-output recovery
records for at most 60 seconds plus the cleanup tick, independently of live-runtime
capacity. Final ACK, detach or disconnect releases them earlier; see
[terminal recovery guarantees](TERMINAL_RECOVERY.md). Historical records do not
retain a durable attachable terminal screen.

Exit publication waits for the PTY reader to drain the child's queued tail output.
ConPTY resources are closed outside the parser lock so their output can keep
draining. On Unix, a quiet 100 ms poll after shell exit ends the stream even if a
background descendant still holds a slave handle. Continuous descendant output is
drained for at most two seconds; a failed/expired drain sends a scoped
`terminal.ended` event with `truncated=true` to attached viewers before the final
lifecycle event, and persists
`SessionInfo.outputTruncated=true` in the historical metadata. Older stored records
default this field to false. This boundary does not claim
to capture future output from detached descendants. Only the bounded attached-viewer
final snapshot described above is retained. Metadata retention is independent of terminal-output retention.

The Server retains all session metadata in SQLite, including exited/lost records
from prior Agent epochs. A complete inventory marks only missing running sessions
as lost; it never deletes historical records. Metadata storage grows with history;
this change does not provide disk compaction, automatic retention or output-log
persistence. The sidebar displays active sessions; the selected final view stays read-only until
switching or dismissal. The authenticated sessions API continues to return retained records.

## Wire format

All Agent `Hello`/`Inventory` and browser `inventory.sync` messages are serialized
as a complete UTF-8 JSON document, then split into at most 32,768 raw-byte fragments.
Each fragment is base64-encoded inside a JSON envelope:

- Agent → Server: `{"type":"InventoryChunk","payload":{...}}`
- Server → browser: `{"type":"inventory.chunk","payload":{...}}`

The payload has `transferId` (decimal string), `index` (zero-based integer), `last`
(boolean), and `dataBase64`. Each final envelope is checked against the existing
65,536-byte WebSocket maximum. Splitting bytes rather than records handles large
single records, UTF-8 boundaries, escaping and multiple large device descriptions.
The empty inventory is still a nonempty JSON document and uses one final chunk.

Transfer IDs start at `"1"` on each new socket, increase exactly once per inventory,
and are never reused on that socket. Indices must be contiguous starting at zero.
Non-final fragments are exactly 32,768 bytes; a final fragment has 1–32,768 bytes.
Senders serialize the entire series without interleaving other application events.
Both send and receive have a 60-second transfer deadline. A stale socket's partial
inventory never carries into its replacement connection.

The receiver stages chunks without exposing partial state. Only a final chunk,
valid UTF-8/JSON, the expected inventory kind and valid/unique record ownership
permit publication. Duplicated, skipped, out-of-order, replayed, oversized,
interleaved or malformed chunks close the connection; timeout also reconnects.
A disconnect discards staging, keeping the last complete list and all stored history.
`Hello` is acknowledged with `Welcome` and the device is marked ready only after
complete reconciliation. A later full Agent `Inventory` uses the same transfer rules.
Server reconciliation acquires the SQLite writer transaction before the hub lock
(the same order used by create/close operations), validates the entire list, then
commits SQLite in one transaction before replacing hub metadata. There is no partial lost-session update
if validation or the database transaction fails.

The complete list still requires memory proportional to retained metadata, but
there is no arbitrary total-record cap. Fragments are emitted at the socket
boundary so one large inventory cannot overflow the 64-item event queue merely
because it contains more than 64 fragments. Existing overload handling reconnects
rather than silently discarding queued events.

## Regression coverage

Rust tests cover 1,501 records, >64 KiB Hello/Inventory/browser envelopes, large
individual payloads, exact fragment boundaries, Unicode, empty lists, replay,
incomplete/restarted transfers, cross-machine and duplicate IDs, replaced
connections, transactional rollback, and retaining history over a second sync.
Terminal tests cover real PTY exit cleanup, queued 128 KiB tail output before exit,
background descendants, and create/idempotency with 1,501
historical records plus the 20-active limit. Browser receiver tests run under the
Playwright test runner without needing Chromium and verify atomic list publication,
reconnect, malformed chunks and timeout. Existing browser end-to-end tests exercise
v3 negotiation and normal reconnect/create/close flows. Legacy v1/v2 peers are rejected.
