---
status: accepted
date: 2026-10-30
---

# Accept literal questions and files in the terminal form

The F5 binding is superseded by
[ADR 0022](0022-quiet-terminal-chrome-and-configurable-status-line.md).

Tab navigation and image budgeting are superseded by
[ADR 0007](0007-continuous-graph-and-native-image-preparation.md).
Click controls and Enter behavior are superseded by
[ADR 0009](0009-compact-keyboard-terminal-view.md).

## Context

[ADR 0005](0005-full-screen-session-view.md) made the form serialize back
through the one-line command grammar. That still required an ID and colon,
rejected otherwise valid punctuation, and made distinct option labels collide
when converted to identifiers. Local files could only be entered as path strings.

## Decision

- The terminal form constructs typed questions directly. Questions and options
  are literal text; IDs derive automatically from the full question and answer
  space. Options use positional internal identifiers. CLI verbs and piped input
  retain their existing grammar.
- Answer types use readable, clickable controls. Click fields to focus them;
  Tab cycles through Context, Question, and Options or Levels when applicable.
  Shift-Tab cycles answer types, with F2 as an alternative. A visible
  Send button, Ctrl-S, and F5 submit from any field, including unqueued context.
- Paste a list into Options to populate separate options. Other fields preserve
  paragraphs and indentation. Pasting never submits.
- Terminal drops are paths, parsed without invoking a shell. Accept bracketed
  paste and ordinary key streams, shell-escaped/quoted paths, Unicode, file URLs,
  and multiple files. Snapshot contents when importing; a filename alone is not
  evidence. Imported text is always literal, including leading slash commands.
- UTF-8 files insert their text into the focused field. PDFs use an optional
  `pdftotext` subprocess, with bounded input/output and a timeout. Supported
  raster images decode, preserving JPEG/PNG bytes and normalizing other formats
  to PNG, appearing as readable references
  at the insertion point. Unsupported binary files produce an actionable error.
- Persist image bytes as evidence records under `image:<id>` keys, with
  a `nouli_image` value carrying reference, filename, format, and base64 bytes.
  Current-turn references make their image records required, preventing context
  reduction from dropping an image being submitted.
  Context manifests retain those bytes for replay and inspection. No original
  file needs to remain on disk after the turn is saved.
- Provider adapters remove binary payloads from textual evidence and translate
  references to native inputs. TypeSafe and routed Jev use assets and positional
  references. OpenAI uses text/image parts in user messages and instructions;
  text-only option descriptions refer to labelled shared images. Other routed
  models receive labelled images in top-level state, their supported location.
  Unsupported models still fail through the provider's capability response.
- Image budgeting conservatively includes encoded bytes in the existing byte
  estimate. This may reject large images before a provider would; it does not
  assert exact visual token accounting. Required-image overflow fails explicitly.

This supersedes ADR 0005's grammar round trip and ID requirement. The single-question and durable-session contracts are unchanged.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Guess whether punctuation means a question or option list | Explicit fields already provide the meaning; punctuation should be content. |
| Store local paths and reread them on later turns | Files can move or change; saved decisions need the original evidence. |
| Send base64 as ordinary text | Models need native image input to inspect pixels. |
| Add arbitrary binary attachments to every provider | Current protocols do not provide a shared document/file contract. Extract supported content and explain unsupported formats. |
| Parse PDFs inside the TUI | A parser panic or diagnostic output must not break the terminal session. |

## Consequences

- Image bytes occupy ledger space. Historical images follow ordinary context
  reduction; images referenced by the current turn are explicitly required.
- PDF import requires Poppler's `pdftotext` executable on PATH. Scanned PDFs
  without text need a separate OCR step or their pages dropped as images.
- PNG, JPEG, GIF, WebP, BMP, TIFF, and ICO decode through the image library;
  animated formats import their first frame. Each imported file is capped at
  16 MiB. SVG and other UTF-8 files supply source text rather than rendered pixels.
- Terminal drag-and-drop targets the focused field. Terminal protocols do not
  provide an independent native file-drop location; click the destination first.
- Native serialization tests and local mock-provider checks do not prove live
  model acceptance, image quality, or exact visual token costs.
