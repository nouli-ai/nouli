# CLI output

Output selection is explicit `--json`, `--format text|json|jsonl|toon|auto`, or `--jsonl`, then the configuration's `output`, then terminal detection. Auto uses text for a terminal and JSON when redirected. Competing explicit output flags are rejected. Help and version remain ordinary command usage output.

Text shows the chosen option's meaning and probability, noul as P(yes), and score as an expected fractional level index with its ordered rubric. A refusal is distinct from a failed call. Explicit caller defaults render as labeled data, with no native-answer or confidence interpretation. Native confidence includes the recorded provider/model and its meaning; it is not a universally calibrated probability. Projected scalar choices retain selected identity/probability metadata; turn reports retain question metadata for rubric and option meanings. Missing report metadata is labeled unavailable rather than invented.

Final JSON is one version-1 `ResultEnvelope`; explicit TOON uses the same data model. Read-only inspection/export exposes its version-1 inspection contracts in either encoding. JSONL consists of version-1 events with consecutive sequence numbers, a stable optional run ID, and one terminal result/error event. Each JSONL line is flushed. A failed write makes that writer unusable, preventing accidental repeated partial lines or terminal attempts. Diagnostics use an explicit stderr sink. Text error reports use stderr; machine errors remain valid protocol data on stdout. No renderer installs a subscriber, emits ANSI decoration, or requests model explanations.

| Exit | Meaning |
| --- | --- |
| 0 | Completed decision or workflow, including completed replay and native per-question refusals |
| 2 | Invalid input or configuration |
| 3 | Provider, authentication, capability, contract, or capacity failure |
| 4 | Partial or failed workflow |
| 5 | Request, writer, context, or advanced-head conflict |
| 6 | Storage, internal, or output I/O failure |
| 130 | Interrupted or unfinished invocation requiring recovery |

Rendering uses explicit `Write` sinks and injected terminal status. Process exit policy stays in the CLI. Invalid non-finite values are rejected before report output; text escapes control characters in caller/model labels. Error messages come from safe structured engine reports and do not echo credential values, model context, or response bodies.
