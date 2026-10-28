# Staged context reduction

`context::compact::Staged` implements the builder's policy seam. It scans full optional historical decisions in canonical ancestor-first order and proposes one useful compaction at a time. The builder encodes and recounts after every change. Only after all useful compaction is exhausted does the policy omit optional records, oldest first.

A compact decision retains source session/turn and actual provider provenance. Each result retains its original question ID and prompt. Choice retains the selected opaque ID, caller description, and selected probability. Noul retains its numeric probability of yes, including 0 and 1. Score retains the fractional expectation and ordered caller labels with level indices. Native confidence remains separate and nullable. A refusal remains typed with its reason.

Compaction drops historical state, unselected alternatives and probabilities, score distributions, and verbose rubric descriptions. It never generates a paraphrase or converts probabilities to acceptance decisions. Standalone context evidence and commitments receive only whole-record omission. Pins, required records, workflow base input, and current live input/questions stay protected.

Every final representation is recorded in the manifest. Protected-only overflow fails explicitly. Reduction is a read-only projection: original states, alternatives, distributions, immutable revisions, and ledger bytes remain intact.
