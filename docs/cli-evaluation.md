# Direct CLI evaluation

Decision input makes evaluation the root command. Bare invocation prints help. There is no required `decide` mode or interactive prompt.

```sh
nouli --state 'The repair costs 120 and replacement costs 300.' \
  --questions '[{"id":"repair","prompt":"Should we repair?","answer_space":{"type":"noul"}}]'
nouli --input request.json
nouli --input request.toon
nouli --input - --input-format json < request.json
```

`--state` (`-s`) accepts repeatable text inputs. `--questions` (`-q`) accepts a JSON question array, or an object containing only `questions`. Use `--input-format toon` for an inline TOON question list/object. Structured state and context updates belong in request files. Inline state/questions and `--input` are mutually exclusive.

A file can contain a portable `TurnRequest` (`version`, `state`, `questions`), or an `EvaluationRequest` containing `turn`, optional `session`, `updates`, `required_records`, `model`, and `request_id`. Omitted turn versions default to v1; omitted session selection creates a new unnamed persisted session. Unknown fields and duplicate keys fail. JSON/TOON share the same request model. Files use `.json`/`.toon` or an explicit format; stdin always requires `--input-format`. There is no grammar fallback. File/stdin input is bounded to 16 MiB.

```sh
nouli --input request.json --name Research
nouli --input request.json --session Research
nouli --input request.json --from Research --name Branch
nouli --input request.json --from Research@019a0000-0000-7000-8000-000000000001
nouli --input request.json --from Branch --from Research --name Blend
nouli --input request.json --from Blend --no-session
```

The inline-selection examples assume a portable turn file, or a full request that omits `session`.

`--session` selects an existing ID or unique case-sensitive name; an unknown selector never creates a session. One `--from` branches and several blend into a new session. `SELECTOR@TURN_ID` captures an explicit finalized source head; without `@`, acceptance captures the current source frontier once. Subsequent source changes do not flow into the saved branch/blend. `--name` applies only to a new persisted session. CLI session/from/name selection cannot compete with explicit file selection. `--no-session` controls durability and is allowed with unnamed new/from selection, including one supplied by a file; it rejects continuation, naming, request IDs, and interrupted-request retry.

Persisted calls use the user ledger by default. Fresh ephemeral calls open no ledger; sourced ephemeral calls open an existing store read-only. The first valid evaluation scaffolds a missing default config; configuration loading for help and inspection creates no files. `HOME` supplies default config/store paths; fully explicit config and store paths also work without `HOME`.

Provider credentials can come from the current directory's `.env` or an explicit
`--env-file FILE`. Exported shell values take precedence. Missing automatic files
are fine; malformed/unreadable files and missing explicit files fail before
execution. See [configuration](configuration.md) for credential setup and errors.

`--provider` selects a named provider and `--model` selects a model independently. Configuration defaults are top-level `default_provider` and `default_model`. Optional `[providers.NAME.models."MODEL_ID"]` settings apply only to that model. `--context-budget TOKENS` overrides its budget, subject to known/configured context limits. Resolution is CLI selection, request selection, session defaults, then config defaults. The engine freezes settings and fixed sources for accepted work, records the actual served provider/model, and never falls back to another model/provider. Native uncertainty and fractional score semantics remain intact through all three adapters.

`--request-id KEY` identifies normalized intent globally. Completed replay returns the saved turn even when formatting or current configuration differs. Syntax whitespace does not affect intent; changing evidence text does. A conflicting intent fails. `--retry-interrupted` requires that request identity and explicitly retries eligible interrupted work using saved settings/source points and fresh credentials from the saved source name. Earlier attempts remain inspectable. Ctrl-C signals cancellation to the shared engine and awaits its cleanup before exiting; an unsaved remote result can remain unknown.

Output controls and exit codes are described in [output.md](output.md). Provider profiles and named credential sources are described in [configuration.md](configuration.md). No keys are collected interactively or stored in history.
