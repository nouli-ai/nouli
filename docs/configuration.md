# Configuration

The CLI reads `~/.nouli/config.toml`; `--config FILE` selects an explicit file.
A missing default is allowed, while a missing explicit file is an error. Unknown
keys, protocols, invalid URLs, and invalid limits fail without prompting. Help
and version exit before configuration loading. Loading/validation creates no
store and performs no provider calls. Decision execution is implemented in later
milestones; the current CLI validates these settings flags.

`fixtures/config/providers.toml` shows the supported layout. Profile names map
to `typesafe`, `openai-decisions`, or `openrouter-decisions` protocols. Custom
endpoints use those same protocols. A profile declares `base_url`, `model`,
optional `api_key_env`, `context_budget_tokens`, `context_window_tokens`,
`reserved_output_tokens`, `max_attempts`, and `attempt_timeout_seconds`. Example
model names are configuration examples, not availability claims.

Endpoints use HTTPS, or explicit local HTTP on loopback/private/link-local IPs
or localhost. Base URL path prefixes are preserved by the transport. Credentials
in URL userinfo/query/fragment are rejected. Put only environment-variable names
in `api_key_env`. Unauthenticated local profiles omit it. Only the selected
profile's variable is read; unrelated missing keys do not block a request.

The library receives explicit home paths, override layers, and an injected
credential lookup. It never reads process environment or installs a tracing
subscriber. `Credential` redacts Debug output and has no Serialize implementation.
`SettingsSnapshot` contains source names and resolved settings, never key values.
Resume re-resolves saved credential sources, preserving model/settings snapshots.
Keys must be nonempty printable ASCII tokens without whitespace/header injection.

Provider and model fields resolve independently in this order: CLI, request,
session defaults, configured default profile. A missing model falls back to the
chosen profile's model. Workflow node fields override the frozen run defaults;
the CLI establishes those run defaults. Overrides do not mutate session defaults.
Callers changing protocols should also select a compatible model explicitly.

Output defaults to `auto`; configured values are `auto`, `text`, and `json`, with
explicit output overrides taking precedence. Execution defaults are four parallel
nodes, three total attempts, 120 seconds per attempt, a 30-second maximum retry
delay, and 16 MiB per response. All are positive. Profile attempt/time limits
override global defaults; explicit runtime limits override profile limits.

The default global store root is `~/.nouli`. Configuration `store` replaces it,
and `--store DIR` overrides both. Relative paths are relative to the caller's
working directory; no project configuration discovery occurs. Settings resolution
returns the chosen path without opening or creating a ledger.
