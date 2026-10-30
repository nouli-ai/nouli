---
status: accepted
date: 2026-10-30
---

# Assign durable names to sessions created without an explicit name

## Context

Anonymous sessions appeared as UUID fragments, which were hard to remember or
resume. The user requested a unique random combination of a short lowercase
scientist surname, a number from 1–20, and a noun derived from an original
spinner-style verb that Claude does not already use.

## Decision

- Use `scientist-number-noun`, such as `curie-7-diffraction`. The pool contains
  32 short lowercase surnames and 31 noun forms: 19,840 combinations. Keep the
  source verb alongside each noun for review. These verb forms were checked for
  lowercase and capitalized occurrences in the installed Claude Code
  2.1.292, 2.1.311, 2.1.312, and 2.1.314 binaries on 2026-10-30; none occurred.
  This is a dated check, not a dependency on Claude or a prediction of future
  vocabulary. Claude documents its configurable spinner vocabulary in its
  [settings reference](https://code.claude.com/docs/en/settings-reference#spinnerverbs).
- Allocate the name centrally when inserting a durable session whose caller
  supplied no name. This covers direct, derived, request-reserved, and workflow
  sessions. Keep explicitly supplied names and existing ledger rows intact.
- Select a random starting combination from UUIDv7's random tail, then probe
  the finite pool until an unused name is found. Check all existing names,
  including manually assigned ones, in the same immediate SQLite transaction
  as the insertion. The existing unique-name constraint is the final guard.
- Persist the chosen name as the real `Session.name`; selectors, session lists,
  graph cards, and reopening use that same name. Session IDs and immutable
  decision histories stay unchanged. Legacy unnamed sessions remain readable.
- Bare `nouli` and `/new` create an automatically named session. `/new NAME`
  retains explicit naming. Tab completes `/new ` for optional naming; Enter can
  execute `/new` without a name. Other commands keep their existing arguments.
- If every generated combination is in use, return `session_names_exhausted`
  and allow an explicit name. Do not add a fourth component, expand the numeric
  range, or silently reuse a name.

This supersedes [ADR 0005](0005-full-screen-session-view.md)'s UUID-fragment
label for newly created anonymous sessions. It does not rename existing
sessions or change the portable session selector contract.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Generate a display label each time | Labels would change and could not be used to resume the session. |
| Allocate only in the TUI | Other durable session creation paths would remain unnamed. |
| Check uniqueness before starting the write transaction | Concurrent launches could choose the same available name. |
| Copy Claude's spinner vocabulary | The user requested original verbs. |
| Add a UUID suffix on collision | Violates the requested three-part shape and bounded number. |

## Consequences

- New sessions have memorable, durable, case-sensitive names without an API
  request or model-generated naming call.
- Uniqueness is scoped to a ledger and enforced across concurrent writers.
- The pool is bounded; explicit names remain available when it is exhausted.
- Automatically named empty roots appear in `/view` like other named sessions;
  `/sessions` continues to list sessions with saved decisions.
