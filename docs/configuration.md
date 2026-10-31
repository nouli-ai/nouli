# Configuration

The CLI reads `~/.nouli/config.toml`; `--config FILE` selects a different file.
Start with `nouli connect`. Choose a service, authenticate, choose a model, and
optionally save it as your startup default. `nouli connect typesafe` skips the
service question. Shortcuts exist for OpenAI, TypeSafe, OpenRouter, Cloudflare
Workers AI, Ollama and llama.cpp; `custom` accepts any compatible endpoint.
Setup validates locally without contacting the provider.

Shortcuts only write ordinary configuration and credential sources. The runtime
never looks up a service by name, host or model ID. Hand-written configuration
has exactly the same behavior. Rename a connection, change its URL, or use any
model ID supported by your server without registering it with Nouli.

```sh
nouli connect
nouli connect openai
nouli connect cloudflare
nouli connect custom --name gateway --protocol system-one \
  --base-url https://gateway.example/api --model my-model \
  --env GATEWAY_TOKEN --default --non-interactive
nouli connect ollama --model clef --default --non-interactive
```

Authentication options are a masked key prompt, `--stdin`, `--env NAME`,
`--command PROGRAM ARGS...`, or `--no-auth`. Put options before `--command`,
whose remaining words are the credential program's arguments. Keys and credential
sources are stored in private `auth.toml`, separately from the configuration.
`--replace` explicitly replaces one existing connection. Existing unrelated TOML
comments and settings are preserved. Setup collects and validates the draft before
writing, serializes concurrent edits under a lock, writes credentials first, and
publishes the configuration by atomic rename. New directories/files are private
(0700/0600). `/cancel` or EOF cancels a prompt. A failed final config write may
leave a reusable credential entry, never a configuration with a newly missing key.

In a terminal, bare `nouli` starts setup when no providers exist, and prompts for
missing startup model or credentials before allocating a session. In the full-screen
session, `/connect [SERVICE]`, `/login [CONNECTION]`, `/logout [CONNECTION]`, and
`/model [CONNECTION]` run setup while idle, then reload settings and retain the
unsent draft. `/model` offers configured IDs and protocol examples and accepts a manually typed ID;
changing it affects the active session unless you choose to save a startup default.
Piped evaluation never prompts.

For manual setup, only the protocol and base URL define the connection:

```toml
default_provider = "my-server"
default_model = "any-compatible-model"

[providers.my-server]
protocol = "system-one"
base_url = "https://gateway.example/prefix"
api_key_env = "GATEWAY_TOKEN"
```

The two protocols are `system-one` (named questions, state, choice/noul/score) and
`openai-decisions` (OpenAI Decisions' native schema). Standard paths append
`v1/systemone` or `v1/decisions` to the URL, preserving its prefix. Override the
suffix with `connection.request_path` without introducing another protocol.
This is decision inference, not a generic chat-completions adapter.

HTTPS and explicit local HTTP on localhost/private/link-local IPs are supported.
URL credentials, query strings and fragments are rejected. `api_key_env` contains
a variable name, never its value. A provider with neither an environment source
nor a stored credential is unauthenticated. Explicit `connection.no_auth = true`
disables primary authentication, including stale stored keys. `auth login` can
store a source in advance and reports when the default model has auth disabled.
`/login` uses the active model's resolved settings; enable authentication there
before logging in.

Provider and model selection resolve independently: CLI, request, session defaults,
then top-level `default_provider` / `default_model`. Model tables are optional
exact-ID settings, not an allowlist. Quote IDs containing dots or slashes.

Limits resolve from `[defaults]`, then `[providers.NAME.defaults]`, then the exact
`[providers.NAME.models."ID"]`. Explicit `--context-budget` wins. The operational
budget is 8192 when omitted, clamped to an explicitly configured context window;
it is an estimate budget, not a claim about the model's capacity. Output reserve
defaults to zero. No provider/model capability catalog is loaded.

```toml
[defaults]
context_budget_tokens = 8192

[providers.my-server.defaults]
max_questions = 16

[providers.my-server.models."any-compatible-model"]
context_window_tokens = 32768
context_budget_tokens = 16000
reserved_output_tokens = 128
per_question_context_tokens = 12000
max_choices = 64
max_score_levels = 10
max_request_bytes = 1048576
input_modalities = ["text", "image"]
```

Connection fields overlay independently from global `[defaults.connection]`,
provider `[providers.NAME.defaults.connection]`, provider `[providers.NAME.connection]`,
then exact model `[providers.NAME.models."ID".connection]`. Maps merge by key;
HTTP headers merge case-insensitively, including across literal/environment sources.
A more specific header source replaces the earlier one.

```toml
[providers.my-server.connection]
binding = "standard"
request_path = "v1/systemone"
auth_header = "X-Api-Key"
auth_scheme = "" # empty means raw key; default is Bearer
image_encoding = "images-data-urls"
# Optional image preparation and approximate pixel accounting:
max_image_dimension = 1568
min_image_dimension = 280
pixel_patch_size = 28
pixel_multiplier = 4
pixel_overhead = 256

[providers.my-server.connection.headers]
X-Tenant = "example"

[providers.my-server.connection.headers_env]
X-Gateway-Token = "GATEWAY_HEADER" # resolved value never enters saved settings

[providers.my-server.connection.extra_body]
provider = { allow_fallbacks = false }
```

`headers` is for nonsecret values; use `headers_env` for secret headers. Primary
authentication overrides a header with the same name. `extra_body` cannot replace
model/state/input/questions/assets/images. Image encodings are `assets` (default
System One references), `images-base64`, `images-data-urls`, `state-image-urls`, and
`openai-content` (OpenAI default). Choose the encoding your server accepts; no model
or hostname selects it. Optional dimensions resize/pad an upright PNG copy; original
evidence remains immutable. Pixel or encoded-byte counts are approximate.

Cloudflare Workers AI uses the same System One codec with a different HTTP binding:

```toml
[providers.edge]
protocol = "system-one"
base_url = "https://api.cloudflare.com/client/v4/accounts/ACCOUNT_ID/ai"
api_key_env = "CLOUDFLARE_API_TOKEN"

[providers.edge.connection]
binding = "cloudflare-workers-ai"
image_encoding = "images-data-urls"

[providers.edge.models."@cf/cloudflare/clef".connection]
wire_model = "clef"
```

This posts to `BASE/run/@cf/cloudflare/clef`, sends `model = "clef"` in the body,
and unwraps the Workers AI `success/result` response. `wire_model` is an explicit
exact-model override, independent of the path selector. There is no third decision
protocol. For OpenRouter, `nouli connect openrouter` writes the System One route
under `https://openrouter.ai/api` and explicit routing preferences. Hand-written
settings can express the same connection. Local Ollama uses `images-base64`;
llama.cpp uses `images-data-urls`.

Unknown fields, invalid URLs/protocol combinations and nonpositive bounds fail
locally. Help, version, invalid requests and inspection do not initialize config.
Noninteractive first evaluation can create a minimal default scaffold but still
requires a configured selection. Missing explicit config files remain errors
except when intentionally authored by `nouli connect --config FILE`.

Before inference, selected settings, credentials, modalities, counts and estimated
budgets are checked. Correcting preflight is a new attempt, without
`--retry-interrupted`. Completed replay and frozen resume use saved settings.
Frozen settings now use format version 2. Older saved runs require a fresh store
(`--store DIR`); they are never silently migrated, rewritten or deleted.

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
nouli add Trip --input request.json
nouli add Trip --env-file .env.test --input request.json
nouli workflow resume RUN_ID --env-file /path/to/keys.env
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


All optional limits and counting behavior are described in
[limits](contracts/limits.md); [defaults](defaults.md) lists Nouli's operational
settings. Selecting a model does not copy another model's overrides.

## Credential store

`nouli auth login PROVIDER` stores a key once, so every command works from any
directory. The key goes in `auth.toml` next to `config.toml` (by default
`~/.nouli/auth.toml`; with `--config FILE`, next to that file). The decision is
recorded in [ADR 0003](adr/0003-central-credential-store.md).

```sh
nouli auth login typesafe                     # prompts for the key, without echo
printf '%s\n' "$KEY" | nouli auth login typesafe --stdin
nouli auth login typesafe --env TYPESAFE_TOKEN
nouli auth login typesafe --command op read op://Private/TypeSafe/credential
nouli auth list
nouli auth logout typesafe
```

Login needs a terminal unless you pass `--stdin`, `--env`, or `--command`. It
checks that the provider is in `config.toml` and never calls it. The file holds
one table per provider, and exactly one of `key`, `env`, or `command` in each:

```toml
[providers.typesafe]
type = "api_key"
key = "…"
# env = "TYPESAFE_TOKEN"
# command = ["op", "read", "op://Private/TypeSafe/credential"]
```

`type` is required, and only `api_key` is accepted for now; unknown types and
fields are errors that name the provider. A `command` runs without a shell, with
only `PATH` and `HOME` in its environment and a 10-second limit, and its trimmed
output (at most 64 KiB) is the key. It runs in its own process group, so a
timeout, Ctrl-C, or oversized output stops the command and anything it started,
and a background job that keeps the output open counts as a timeout.

A provider's key is the first of these that has a value:

1. The variable named by its `api_key_env`, exported in the shell.
2. The same variable in `--env-file` or `.env`.
3. Its entry in `auth.toml`.

An exported empty variable counts as a value and is an error, as before. If none
applies the call fails with `credential_unavailable`. A provider with neither an
`api_key_env` nor an entry is unauthenticated, which suits local models.
`nouli auth list` shows each provider's stored source type and which source
currently supplies its key (`shell`, `env file`, `store`, or `none`), and never a
value.

Nouli creates the directory with mode 0700 and the file with mode 0600 from the
start, writing a temporary file and renaming it into place. It refuses to read a
file that is readable by group or others, that you do not own, or that is a
symbolic link, with `auth_store_insecure`; fix it with `chmod 600` on the file.
Keys are never written to the ledger, to inspection exports, or to errors.

A saved workflow run records where its key comes from (`env:NAME`, or
`auth:PROVIDER` for a stored entry), not the key. Resuming uses the key as it is
then, so changing the stored key before `nouli workflow resume` takes effect.

| Code | Meaning |
| --- | --- |
| `credential_unavailable` | No source has a usable key; the message names the provider and suggests `nouli auth login PROVIDER`. A failing, empty, or timed-out `command` source gives the same code. |
| `auth_store_insecure` | `auth.toml` is accessible by others, owned by someone else, or a link. Run `chmod 600` on it. |
| `invalid_auth_store` | `auth.toml` does not parse or breaks the rules above; the message names the provider and field, never the value. |
| `auth_store_unreadable` | `auth.toml` exists but cannot be read. |
| `terminal_required` | `auth login` needs a terminal to prompt; use `--stdin`, `--env`, or `--command`. |
