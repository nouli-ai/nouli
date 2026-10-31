# Configured limits and estimated budgets

Nouli owns protocol codecs and operational defaults. It has no bundled provider
or model capability registry, host recognition, model-name matching, or runtime
network discovery. Every compatible endpoint and arbitrary model ID follows the
same resolution path. Setup shortcuts write editable settings only.

`ModelSettings` supports context budget/window, output reserve, per-question
context, maximum questions/choices/score levels, maximum serialized request bytes,
and input modalities. Merge order is global defaults, provider defaults, exact
model settings. CLI budget overrides that result. Missing budgets use 8192;
a configured context window clamps the budget. Output reserve defaults to zero
and must remain smaller than the effective budget. Empty or oversized input can
still be rejected by the server; this operational default asserts no model capacity.

Portable decision contracts still require 2 to 255 choices and 2 to 10 ordered
score levels. Configured count bounds can be smaller. Text and image are the
implemented modalities; configured modality lists further restrict them. Audio
and video remain unsupported. The final serialized body is checked against an
optional request byte limit before HTTP dispatch.

`TokenCounter::count` receives the complete resolved `ProviderProfile`, assembled
context and complete questions. Embedders can supply a verified counter for their
server. The built-in `ByteCounter` is explicitly approximate: text bytes divided
by four, an estimated envelope and per-question overhead, plus image accounting.
There is no generic tokenizer accuracy claim. Native usage is separate, and never
replaced by the estimate.

Image encoding and preparation are explicit connection settings. Optional bounds
apply orientation, resize/padding and PNG conversion to a request copy, preserving
original evidence. Pixel accounting uses ceil(width/patch) times ceil(height/patch),
optional multiplier (default 4) and overhead (default 256). Without pixel settings,
encoded bytes supply an estimate. The counter and encoder use the same prepared
pixels. Shared image-list/state encodings count shared images once; positional
asset/OpenAI references account for each supported occurrence. Image counts are
marked `multimodal_estimate`, never exact usage.

The full request, mandatory questions, current evidence and protected history must
fit. Older optional context may be reduced through the ordinary history policy;
required images never disappear silently. Capacity reduction and bounded retries
remain engine-owned. No protocol adapter switches providers or models.
