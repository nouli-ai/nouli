# Request ownership and replay

`Ledger::session_guard` and `Ledger::run_guard` use OS file locks held by owned file handles. They span provider work without holding a SQLite transaction. Different sessions can execute independently. Dropping an owner or exiting its process releases ownership. Guard files remain as reusable lock identities and must not be removed while a store is in use.

`accept_request` requires a store-wide request ID. It fingerprints the validated submitted turn, session selection, updates, required records, and explicit model selection. Resolve explicit command-line model overrides into the request before acceptance. Encoding and output presentation do not affect identity. Initial acceptance atomically creates any new session/name and captures source frontiers and resolved settings. The returned owner authorizes preparation and finalization on that session.

A live owner returns `InProgress`. Different intent under an existing ID returns `IntentConflict`. A completed duplicate returns the saved turn and frozen settings without creating another attempt or session, even when defaults or source heads have advanced. Credentials are never in snapshots.

After ownership is lost, acceptance records interruption. A request without a running attempt is distinguished from one whose remote outcome is unknown. No automatic remote replay occurs. `retry_interrupted_request` explicitly reacquires the original reservation and settings and appends a new attempt, provided the original session head remains unchanged. An unknown outcome can have incurred a provider call and cost; this mechanism does not promise exactly-once remote inference.

Attempts carry owner identity. Finalization checks the capability, the expected session revision, and reservation ownership, then publishes the turn, active head, and completed replay result in one transaction. Earlier interrupted attempts remain inspectable.
