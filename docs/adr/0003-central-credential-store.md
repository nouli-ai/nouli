---
status: accepted
date: 2026-10-29
---

# Keep provider credentials in a central, private credential store

A provider's key can come only from an environment variable named by
`api_key_env`, read from the shell or a `.env` file in the current directory.
That is right for CI and agents, but people expect to log in once and have every
command work from any directory, as coding agents do. TypeSafe has no OAuth, and
OpenAI's Decisions API only takes API keys, so the store holds API keys now, and
its format must leave room for other credential types. ADR 0001 deferred
keychain integration, and the library never reads the environment or files
itself: it receives explicit paths and an injected credential lookup.

## Decision

- **File.** `auth.toml` sits next to `config.toml` (by default
  `~/.nouli/auth.toml`) and follows the configuration file's home and override
  rules. Entries are keyed by the provider names in `config.toml`:

  ```toml
  [providers.typesafe]
  type = "api_key"
  key = "…"   # exactly one of key, env, or command
  ```

  `type` is required, and only `api_key` is accepted. Any other type is an
  error that names the provider, so `oauth` can be added without changing the
  format. Unknown fields are rejected. `env` names an environment variable that
  holds the key. `command` is a program and its arguments.
- **Where the code goes.** Types, parsing, and validation are in `nouli-core`
  (`config::auth`) and touch no file or process. The CLI reads and writes the
  file, checks permissions, runs commands, and builds the credential lookup.
  Parse errors never repeat the file's text, because the parser's own messages
  quote the line they failed on.
- **Command sources.** A command runs directly with no shell, in an environment
  of only `PATH` and `HOME`, with a 10-second limit, in its own process group that is killed whole on a
  timeout or cancellation. Its trimmed output is the key. A nonzero exit, a
  timeout, output over 64 KiB, or empty output is `credential_unavailable`.
  The output and error text are never logged or reported.
- **File safety.** The directory is created with mode 0700 and the file with
  0600 from the moment it exists: the writer opens a temporary file in the same
  directory with mode 0600, flushes and syncs it, and renames it into place.
  Reading refuses a file that is group- or world-accessible, not owned by the
  current user, or a symbolic link, with `auth_store_insecure`, naming the path
  and the fix (`chmod 600`). Credential values never appear in errors, `Debug`
  output, logs, the ledger, or inspection exports.
- **Resolution order** for a provider: the variable named by its `api_key_env`
  exported in the shell, then the same variable in `--env-file` or `.env`, then
  its entry in `auth.toml`. A variable that is exported but empty is a value and
  does not fall through. `api_key_env` is optional when the store has an entry,
  and a provider with neither stays unauthenticated. Otherwise the result is
  `credential_unavailable`, with a suggestion to run `nouli auth login NAME`.
- **Saved runs.** A frozen run records the credential source, never a value. The
  source is `env:NAME` (the profile's `api_key_env`, with the provider's stored
  entry as the fallback) or `auth:PROVIDER` (the profile's `stored_credential`
  flag). Runs saved before the store have only `api_key_env` and read as
  `env:NAME`. A resumed run resolves the current value of its source.
- **Commands.** `nouli auth login PROVIDER` prompts without echo and needs a
  terminal; `--stdin` reads one line, and `--env NAME` and
  `--command PROG [ARGS...]` store those sources instead of a value. Login
  checks that the provider exists in `config.toml` and never calls it.
  `nouli auth logout PROVIDER` removes the entry. `nouli auth list` shows each
  provider, its stored source type, and which source currently wins (shell, env
  file, store, or none), and never a value.

## Considered options

| Alternative | Reason for the chosen direction |
| --- | --- |
| Environment variables only | Keeps one source, but every directory needs its own `.env` or a shell profile that exports secrets to every program. People expect to log in once. |
| The operating system keychain for now | Better at rest, but each platform needs its own integration and tests, headless Linux has no common keychain, and the library's explicit-lookup boundary would need a platform dependency. The `command` source already reaches a keychain or password manager. Revisit it with a real need. |
| Pi-style shell strings for commands | A string handed to a shell invites quoting mistakes and injection from a configuration value. A program and an argument list run directly, with a scrubbed environment, do the same job. |
| Store keys in `config.toml` | The configuration file is meant to be shared and committed. Credentials need a file with its own permissions and its own refusal rules. |

## Consequences

- A person can run `nouli auth login typesafe` once and use any command from any
  directory. CI and agents keep using environment variables, which win.
- Every command reads `auth.toml` when it loads credentials, so an insecure file
  stops them until it is fixed. That is deliberate: a file with loose
  permissions may already have been read by someone else.
- The key is stored as plain text, protected by file permissions only. The
  `env` and `command` sources keep it out of the file.
- Adding OAuth later means a new `type`, a new refresh step in the CLI, and a
  new credential source; saved runs and the file format do not change.
