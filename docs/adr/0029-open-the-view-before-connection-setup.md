---
status: accepted
date: 2026-11-17
---

# Open the view before connection setup

## Context

[ADR 0019](0019-independent-protocol-connections.md) introduced automatic
first-run connection setup. Bare `nouli` consequently asks line-based questions
before opening its interactive view when a connection, model, or credential is
missing. This hides the interface people launched and conflicts with the startup
behavior in [ADR 0005](0005-full-screen-session-view.md).

## Decision

- Open the interactive session immediately on a capable terminal, even without a
  connection, model, or credential. Keep configuration and store safety checks.
- Show persistent idle guidance: “No model configured” with `/connect` when no
  connections exist, or “No model selected” with `/model` when the selection is
  incomplete. Keep normal progress, errors, and contextual hints above this
  guidance in priority.
- Start setup only through explicit commands. Keep `/connect`, `/login`, and
  `/logout` and their draft retention. Keep the in-view `/model` picker and its
  connection and credential entry from
  [ADR 0024](0024-model-picker-in-the-view.md). `nouli connect` remains available
  for direct setup and scripts.
- Do not contact a provider or execute credential commands merely to open the view.
  Evaluate and fetch model lists only when the user requests those actions.

This supersedes ADR 0019's automatic first-run interaction. Its configuration
format, runtime boundaries, and atomic publication rules remain in force.

## Considered alternatives

- Keeping the startup wizard would continue to hide the interactive view behind
  questions the user did not request.
- Automatically opening the model picker would still interrupt startup and can
  contact providers before the user asks for a model list.
- A one-time startup message would disappear while typing. Idle guidance remains
  available until setup is complete.

## Consequences

First launch behaves the same on Windows and Unix. Users can draft input before
configuring a service and explicitly choose when to set it up. Evaluations still
require valid model settings and credentials; their existing errors remain.
Invalid configuration and unsafe persistence still fail startup with actionable
errors. Piped evaluation retains its noninteractive behavior.
