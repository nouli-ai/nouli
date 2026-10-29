# Workflow CLI

```sh
nouli workflows run workflow.json --name Travel --parallel 2
nouli workflows run workflow.toon --from Research --from Alternatives --jsonl
nouli workflows run - --input-format json < workflow.json
nouli workflows show 019a0000-0000-7000-8000-000000000400 --json
nouli workflows resume 019a0000-0000-7000-8000-000000000400 --jsonl
```

Run accepts a portable `WorkflowSpec` or a complete `WorkflowRequest` containing `workflow`, optional `session`, `updates`, `required_records`, and `request_id`. JSON and TOON use the same strict boundary. Omitted workflow/embedded-turn versions default to v1. Stdin requires an explicit format; file/stdin input is bounded to 16 MiB. Graph validation rejects cycles, hidden references, invalid typed conditions, and unsupported node shapes before execution. Bare workflow commands print actionable usage without reading credentials.

`--from`, `--session`, and `--name` use the same selection rules as [the command line guide](cli.md). One source branches and several blend into a new session. `SELECTOR@TURN_ID` selects an explicit finalized source head. `--parallel` bounds ready-node concurrency in addition to the graph's limit. `--provider`, `--model`, and `--context-budget` set run defaults/overrides; explicit node provider/model selections take precedence. Actual served identities appear on completed node summaries and selected outputs, separately from frozen requested settings.

`--no-session` runs a fresh or sourced ephemeral workflow. A spec whose mode is already `ephemeral` has the same restrictions. Ephemeral runs reject continuation, naming, request IDs, and resume. Fresh runs open no store; saved sources use an existing read-only ledger. Returned persisted run/session/turn IDs are absent.

A durable run saves its graph, settings, source points, and caller base before dispatch. Each node sees its declared completed ancestry, with no unrelated sibling imported by completion order. Partial results retain successful branches and record IDs; failed, blocked, skipped, and interrupted nodes remain distinct. Final output contains declared typed references or terminal-node answers, preserving fractional score scales, refusals, absence, and native confidence. Full intermediate outcomes and actual contexts are available through workflow/session inspection.

Show is read-only and includes frozen inputs/settings, current recorded node state, live-owner and head-compatibility observations, attempts and unknown-outcome receipts, ordered events, and prior retry rounds. It does not dispatch or recover work. Resume takes only the saved run ID; it does not read a workflow file or current provider definitions. It reuses completed/skipped nodes, starts a new bounded retry round for eligible work, uses the original source snapshots and model selections, and resolves credentials afresh from saved source names. An advanced target head fails explicitly instead of overwriting later turns. A missing resume store is reported without creating it. Completed replay performs no provider work or database content rewrite.

Workflow run and resume load credentials from the current directory's `.env`,
or the file selected with `--env-file FILE`. Exported shell values win. Resume
uses the original saved credential variable name with the current value; workflow
show does not load env files. See [configuration](configuration.md).

Text, JSON, JSONL, and explicit TOON use the [output policy](output.md). JSONL delivers live versioned progress and one terminal result/error event, without diagnostic prose on stdout. Delivery failure returns an I/O failure, reports the saved run ID on stderr when available, and does not append another terminal event. A consumer that stops draining cannot prevent Ctrl-C cleanup: nonblocking output waits on writable readiness and cancellation. If cancellation interrupts a partial backpressured frame, the process exits 130 with the saved run ID on stderr; the unfinished stream remains incomplete rather than claiming a delivered terminal result. Ctrl-C cancels the shared engine, awaits owned task cleanup, preserves recoverable progress, and releases guards. A remote result that was not saved remains unknown; resume can repeat that inference.

No workflow command executes arbitrary scripts, tools, loops, or interactive approval prompts. Invalid current provider profiles do not prevent show/resume; store/output configuration must still parse. Older store schemas require a writable migration before read-only inspection or resume preflight.
