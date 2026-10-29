# Nouli

Nouli coordinates typed model judgments over supplied evidence and recorded
decision histories. These terms distinguish judgments, caller commitments, and
the histories used to make subsequent decisions.

## Language

**Decision question**:
A caller-supplied question with instructions and a defined answer space or
rubric.

**Decision result**:
A typed model judgment for a decision question, with the uncertainty information
the model supplies. It does not assert that the caller adopted the judgment.
_Avoid_: Caller commitment, completed action

**Choice**:
A decision question whose result selects a supplied category and gives a
probability distribution over the supplied options.

**Noul**:
A decision question whose result is the probability that a supplied proposition
is true or that the answer is yes.
_Avoid_: Boolean verdict, confidence

**Score**:
A decision question whose result is the probability-weighted mean of ordered
rubric-level indices. It may be fractional and differs from the most probable
level.
_Avoid_: Selected level, rounded rating

**Provider confidence**:
A model's reported measure of uncertainty, with meaning specific to that model
and question type. It is distinct from a rubric score or the probability of a
proposition being true.

**Caller commitment**:
A caller assertion that a decision was adopted or an action occurred.
_Avoid_: Model recommendation

**Turn**:
A submitted unit of context and one or more independent decision questions,
together with their recorded outcomes.

**Session**:
An identified decision history that can be continued and used as a source for
new histories.

**Decision ledger**:
The durable record of submitted information, decision outcomes, context changes,
and their ancestry.
_Avoid_: Model context

**Model context**:
The information actually supplied to the model for a particular evaluation.
It may contain less information than the ledger.
_Avoid_: Full history, implicit model memory

**Source point**:
A fixed point in a session history used as the basis for a branch or blend.

**Branch**:
A new session derived from one source point.

**Blend**:
A new session derived from multiple source points, with distinct source histories
and any shared ancestry retained.
_Avoid_: Probability averaging

**Pinned context**:
Caller-designated information kept available when its history is used, until
explicitly released or superseded.

**Supersession**:
An explicit replacement of an earlier active context item by a later item,
with the earlier item preserved in history.
_Avoid_: Deletion

**Compaction**:
Reduction of a historical decision's model-context detail to its question,
meaningful output, and retained uncertainty.
_Avoid_: Ledger deletion

**Eviction**:
Removal of an optional record from a particular model context, with its ledger
record preserved.
_Avoid_: Ledger deletion

**Workflow**:
A caller-defined acyclic graph of decision turns with dependencies and explicit
conditions.

**Workflow node**:
One decision turn within a workflow.

**Workflow run**:
A particular execution of a workflow against fixed inputs and source points,
with recorded node outcomes.

**Skipped node**:
A workflow node intentionally excluded by the workflow's conditions.
_Avoid_: Failed node

**Blocked node**:
A workflow node unable to proceed because a required result is unavailable.
_Avoid_: Intentionally skipped node

**Refusal**:
A per-question outcome indicating that the provider declined to supply a
decision answer.
_Avoid_: Low confidence, transport failure
