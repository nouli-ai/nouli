# Fixed source snapshots

`Ledger::resolve_sources` resolves ordered session IDs or unique, case-sensitive names in one read transaction. Each selection can use its current finalized head or an explicit older finalized frontier belonging to that session. Missing, empty, duplicate, foreign, and unfinalized frontier entries fail validation. The same API works on an existing read-only ledger without creating sessions or writer guards.

`Ledger::create_session_from` freezes those source points and creates a new session atomically. One source branches; multiple sources blend. A derived session can have its own explicit provider/model defaults. Source sessions, outcomes, and record identities remain intact, including mixed-model histories. Source references are immutable after capture.

Ancestry traversal deduplicates common turns by immutable ID and orders ancestors before descendants, using source/frontier/parent declaration order for independent branches. Context projection uses causal revisions and reports incompatible active alternatives. Supply explicit resolution updates with the new evaluation; preparation rejects unresolved conflicts before dispatch. Source probabilities and scores remain separate judgments.

The first finalized derived turn references its captured source heads. Later turns reference the derived session's own finalized head. Advancing a source, including adding a replacement, leaves existing branches and blends attached to their original source points.
