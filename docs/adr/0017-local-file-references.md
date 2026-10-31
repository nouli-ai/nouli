---
status: accepted
date: 2026-10-30
---

# Discover local files in the existing reference picker

## Context

Users need `@` to find files in the launch directory as well as saved answers.
Current provider adapters accept text/structured evidence and native images;
an arbitrary Office binary is not an implemented model input. The agreed first
version extracts document text/tables and sends standalone images natively.

## Decision

- Extend the existing picker with relative file paths, preserving saved-answer
  discovery across the ledger. Scan only the launch directory and subfolders,
  honor ignore rules, skip hidden/symlink paths, and bound directory discovery.
  Refresh on opening; read contents only after explicit selection.
- Reuse the file-drop importer. Enter inserts a snapshot in the focused field,
  without submitting or changing its answer type. Each imported file occupies
  one option/level; ordinary pasted lists retain their existing behavior.
- Extract DOCX body and PPTX slide text/tables with bounded ZIP/XML readers,
  preserving presentation order. Read workbook sheets with Calamine, including
  date/duration meaning and formulas missing cached values. Serialize table rows
  as JSON arrays so multiline cells and delimiters survive the form unchanged.
  Parse CSV/TSV with the same row representation. Reuse Poppler for PDF text.
- Render standalone SVGs to PNG and reuse existing native image evidence and
  capability/budget checks. Reject unresolved linked SVG assets explicitly.
  Do not introduce native Office/PDF uploads, page rendering, OCR, semantic file
  search, formula recalculation or a conversion service.
- Isolate modern document/SVG parsers in a private self-worker with bounded
  input/output, CPU/memory limits and a ten-second deadline. This also bounds
  dense allocation from sparse sheets and compressed embedded SVG images.
  Optionally use installed LibreOffice for legacy DOC/PPT, with a private
  directory/profile, inherited limits and process-group cleanup.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Build a second file browser/index | The existing picker already handles selection and field navigation. |
| Send Office bytes as text | Does not provide usable document contents to the model. |
| Render every document page | Adds dependencies and context usage beyond the agreed text/table scope. |
| Parse workbooks in the TUI process | Compressed size does not bound sparse-sheet allocation. |

## Consequences

No ledger schema or provider protocol changes are needed. Selected text follows
existing form/fact persistence, and image snapshots retain their current lifecycle.
Later file edits do not change a selected snapshot. Document layout, charts and
embedded images are outside this text/table version; optional converters remain
explicit local prerequisites rather than bundled installations.
