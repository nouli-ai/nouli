# Configuration

The CLI reads `~/.nouli/config.toml`; `--config FILE` selects an explicit file.
On the first valid decision or workflow evaluation, a missing default file is
created as a minimal scaffold containing `output = "auto"` and no providers.
Define a `[providers.NAME]` section, set `default_provider` to that name
and `default_model` to a compatible model (or pass `--provider NAME --model MODEL`),
and supply its credential environment variable or
separately run the configured local Laya service. A configuration with no providers
reports `providers_required` and asks you to define providers.

In your provider definitions, put environment-variable names in `api_key_env`, never key values. New Unix
configuration directories/files use modes 0700/0600. A complete temporary file
is published atomically without replacing an existing file, including when two
processes initialize concurrently. Existing or malformed configuration is
preserved. Missing explicit `--config` files are errors and are never generated.
The template is `nouli-cli/config.default.toml`.

Help, version, invalid decision/graph input, inspection, and saved workflow resume
do not initialize configuration. `--no-session` still allows first-run configuration
setup, but creates no ledger. The library loader remains read-only and accepts a
missing optional default as empty settings. CLI setup errors identify the missing
provider/model or selected credential variable and show the configuration path.

Before creating a new ledger, CLI evaluation validates selected providers and models, native
input/capability limits, budgets, and credentials. Correcting a preflight failure
is a normal new attempt; no request was accepted, so `--retry-interrupted` is
unnecessary. Existing ledgers keep engine-owned replay/recovery and session defaults:
completed replay and frozen resume can work without current provider definitions.
Unknown fields, invalid protocols/URLs, and invalid limits fail without prompting.
Configuration setup performs no provider calls.

`fixtures/config/providers.toml` shows the supported layout. Each named provider
specifies `protocol` (`typesafe`, `openai-decisions`, or `openrouter-decisions`),
`base_url`, optional `api_key_env`, and optional `max_attempts` and
`attempt_timeout_seconds`. Custom endpoints use those same protocols.

Top-level `default_provider` and `default_model` select independent defaults.
Neither is required in the file if the request, session, CLI, or workflow nodes
supply the missing selection. An evaluation with no selected model reports
`model_required`; it never chooses a model from the provider's model entries.

Optional `[providers.NAME.models."MODEL_ID"]` tables configure
`context_budget_tokens`, `context_window_tokens`, and `reserved_output_tokens`
for exact model IDs. They do not restrict which models you can select. Model IDs
containing dots or slashes must be quoted. The selected model uses only its own
limits, so switching models does not reuse another model's capacity. Model names
in examples are illustrative, not availability claims.

Endpoints use HTTPS, or explicit local HTTP on loopback/private/link-local IPs
or localhost. Base URL path prefixes are preserved by the transport. Credentials
in URL userinfo/query/fragment are rejected. Put only environment-variable names
in `api_key_env`. Unauthenticated local providers omit it. Only the selected
provider's variable is read; unrelated missing keys do not block a request.

The CLI automatically reads `.env` from the current working directory for
provider credentials. It does not search parent directories or the config
file's directory. Use `--env-file FILE` to select one different file; relative
paths are resolved from the current working directory. That file replaces the
automatic `.env`, rather than merging with it.

```dotenv
TYPESAFE_TOKEN="your-api-key"
```

Set `api_key_env = "TYPESAFE_TOKEN"` in your provider definition to use that name.
Assignments, comments, quoted values, and optional `export` prefixes are supported.
Env files are parsed as data, never executed as shell scripts. Exported shell
values take precedence, including an exported empty value, which still fails
credential validation. File values do not change configuration/store paths or
`HOME`.

```sh
nouli --input request.json
nouli --env-file .env.test --input request.json
nouli workflows resume RUN_ID --env-file /path/to/keys.env
```

A missing automatic `.env` is fine. An explicitly selected missing file reports
`env_file_not_found`; unreadable files report `env_file_unreadable`; invalid
syntax or UTF-8 reports `invalid_env_file`. Errors show the file path without
its contents. Execution commands validate an existing file even when credentials
are exported or the result is a completed replay. Help, version, invalid input,
and read-only inspection do not load env files. Env-file failures occur before
opening a ledger or sending a provider request.

Workflow resume keeps its saved credential variable name and resolves its current
value from the shell or the env file selected for that invocation. The env file
path and credential values are not frozen into the run.

The library receives explicit home paths, override layers, and an injected
credential lookup. It never reads process environment or installs a tracing
subscriber. `Credential` redacts Debug output and has no Serialize implementation.
`SettingsSnapshot` contains source names and resolved settings, never key values.
Resume re-resolves saved credential sources, preserving model/settings snapshots.
Keys must be nonempty printable ASCII tokens without whitespace/header injection.

Provider and model fields resolve independently in this order: CLI, request,
session defaults, then `default_provider` and `default_model`. Selecting another
provider does not change the selected model. Workflow node fields override the frozen run defaults;
the CLI establishes those run defaults. A workflow needs no global model default
if every node resolves its own model. Overrides do not mutate session defaults.
Callers changing protocols should also select a compatible model explicitly.

Output defaults to `auto`; configured values are `auto`, `text`, and `json`, with
explicit output overrides taking precedence. Execution defaults are four parallel
nodes, three total attempts, 120 seconds per attempt, a 30-second maximum retry
delay, and 16 MiB per response. All are positive. Provider attempt/time limits
override global defaults; explicit runtime limits override provider limits.

The default global store root is `~/.nouli`. Configuration `store` replaces it,
and `--store DIR` overrides both. Relative paths are relative to the caller's
working directory; no project configuration discovery occurs. Settings resolution
returns the chosen path without opening or creating a ledger.


Known model limits come from the dated, host/protocol/model-specific capability snapshot described in [limits](contracts/limits.md). A smaller `--context-budget` is allowed; exceeding a known/configured window fails. Unknown aliases, custom endpoints, and local Laya models require a budget or window in their model settings, or an explicit `--context-budget`. Output reservation defaults to zero if omitted and counts against the budget. Native usage remains separate from estimated preflight tokens. See [all numerical defaults](defaults.md).

For a separately running local Laya service, configure a provider such as the following. Endpoint, served model name, and capacity must match that service; these values demonstrate configuration shape, not a model installation or capacity guarantee:

```toml
default_provider = "local"
default_model = "english"
output = "auto"
[providers.local]
protocol = "typesafe"
base_url = "http://localhost:8000"

[providers.local.models."english"]
context_budget_tokens = 8192
reserved_output_tokens = 128
```

The offline [walkthrough](walkthrough.md) generates a temporary loopback provider instead. Hosted examples in `fixtures/config/providers.toml` name environment sources, never credentials. No key is collected interactively; supply it through the shell or an env file. Full request syntax, selection, and stdin rules are in [direct evaluation](cli-evaluation.md).

One provider can serve multiple models. Add a settings table for each model that
needs explicit limits, then select a model for a call:

```sh
nouli --input request.json --model another-model --context-budget 4096
nouli --input request.json --provider another-provider --model compatible-model
```

Use only models supported by the selected service and protocol. Models without
settings use their recognized capability limits, if available; custom or unknown
models require an explicit budget/window. The existing model settings are not
copied into an unlisted model.
