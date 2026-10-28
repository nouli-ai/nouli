# Local ledger foundation

`storage::Ledger::open(root, mode)` selects `root/ledger.sqlite3` explicitly.
Write/create mode creates new directories with mode 0700 and the ledger with
mode 0600 on Unix. Existing permissions are preserved. Read-only/existing mode
never creates a directory, database, schema, or migration. Missing stores and
outdated/newer schemas have explicit errors.

Nouli-owned databases use a fixed SQLite application ID and `user_version`.
Unrelated nonempty databases are rejected. Migrations execute together in one
immediate transaction, including identity/version changes, so failed migration
steps roll back all intermediate changes. Future schema versions are never
downgraded. The initial schema covers sessions/names, fixed sources, immutable
turns/parent links/context records/events/manifests, request reservations,
attempts, workflow runs/nodes/events, and supporting indexes.

Connections enable foreign keys, recursive replacement guards, a two-second busy timeout, DELETE journaling,
and FULL synchronization for writers. DELETE avoids WAL/SHM creation or
checkpointing by read-only readers. Read-only opening checks SQLite's journal-format header before connecting and
rejects unsupported WAL stores without creating sidecars. The offsets follow the
[SQLite file-format reference](https://www.sqlite.org/fileformat.html).
Writes are short synchronous transactions; independent provider calls run outside
database transactions. Immutable history,
ancestry, sources, manifests, and logical events have update/delete guards.

This API is blocking. Async engine code must invoke storage on an owned blocking
worker and return from a transaction before awaiting provider work. The low-level
transaction callback is synchronous and commits only on success. No provider,
credential lookup, CLI, or subscriber is installed by storage.

Internal structured payloads use the strict TOON codec, retaining full supported
integer/floating values, nulls, strings, and history order. Only serializable
settings snapshots enter storage; resolved credentials are not serializable.
Storage errors preserve operation categories and SQLite codes without embedding
raw payloads, SQL parameter values, or upstream parser source context.

Temporary-store tests cover creation/reopening, atomic migration success/rollback,
future schemas, foreign keys, transaction rollback, read-only no-create/no-migrate
behavior, complete TOON values, and credential-free settings snapshots. Session
repository operations, process guards/replay, active context projection, and
branch/blend repositories are implemented by the following milestone tasks.
