---
status: accepted
date: 2026-10-30
---

# Keep answer types and reuse omitted choice options or score levels

Reuse notices and terminal presentation are simplified by
[ADR 0009](0009-compact-keyboard-terminal-view.md).

## Context

Follow-up information should be enough to request a revised judgment without
retyping the question's answer list. The user chose the Citrus palette from
[the color review](../design/question-type-colors.html) and requested that the
current answer type stay selected, with prior options or levels used when
omitted. They explicitly rejected adding a separate follow-up mode and excluded
Yes/No from fallback behavior.

## Decision

- Keep the selected answer type after a saved turn, while clearing editable
  fields. Citrus uses lime `#A3E635` for Yes/No, cyan `#67E8F9` for Choose one,
  and rose `#FDA4AF` for Score, with ANSI 256-color fallbacks 155, 87, and 217.
  Form controls, results, and graph cards use the same type colors; context and
  model information stay neutral. No-color output keeps labels and emphasis.
- In the full-screen form only, a choice or score with no nonblank answer list
  reuses the previous question's exact typed answer space and criteria. A visible
  "Prior options will be used" or "Prior levels will be used" notice and preview
  explain the fallback. A new explicit list replaces the old list entirely and
  undergoes ordinary validation. There is no separate mode or toggle.
- A blank choice/score prompt can reuse the previous prompt, shown as a dim
  preview. An explicit Send requests a new judgment with any added or replaced
  evidence. Reused answer lists preserve question identity, including when the
  wording changes, allowing the existing result comparison to show the change.
- Cache only the latest saved single Choice/Score question in the selected
  session. Reopening a session restores its sole frontier's owned question;
  empty branches do not inherit an ancestor's definition. Independent sessions,
  multiple-question heads, and later Yes/No turns clear the fallback.
- Require the preceding turn and all current new/replacement evidence in the
  submitted context. This protects both the earlier judgment and new information
  from ordinary context reduction without permanently pinning anything.
- Failure, cancellation, and unsaved what-ifs keep input and fallback intact.
  Inherited previews do not count as unsent edits or cause discard warnings.

This supersedes [ADR 0004](0004-ask-each-question-once.md) only for omitted
full-screen form fields on an explicit Send. Each request still names one typed
question, saves a new immutable turn, and never rewrites a prior answer. No
standing questions or background re-evaluation return. Command-line and piped
question syntax are unchanged.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| A separate follow-up mode | Rejected by the user; omission already expresses reuse. |
| Populate editable fields with the previous values | Makes an idle form look like unsent input and obscures whether a new list replaces the old one. |
| Reconstruct the previous rubric from displayed labels | Loses criteria, detailed level structure, and original option IDs. |
| Search backwards for any matching question | Can reuse stale lists across intervening decisions or branches. |
| Change the previous saved outcome | Would destroy the decision history and its provenance. |

## Consequences

- A user can add new context and press Send after a choice or score. The prior
  result remains visible in the graph and the new result is a child turn.
- A genuinely new question with the same type must supply a new answer list to
  replace the fallback. Switching to a different type has no compatible list.
- A tight context budget fails explicitly if the prior judgment and current
  evidence cannot fit together, rather than silently dropping the new evidence.
