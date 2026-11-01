# Recorded recall decision graph

The diagram below is generated from the recorded Mermaid graph in this document.

![Recorded recall decisions: three source sessions combine through two layers of blends.](../.github/readme/recall.svg)

A food distributor handles a peanut butter recall. It keeps the recall scope, its warehouse stock, and its customers in separate sessions, then blends them into one coordinated response. When a customer reports that 12 affected boxes were already sold, it adds that one fact with a new question about it, and builds a revised response in new sessions, so the first response stays as it was decided. The new turn:

```sh
nouli add recall-customers \
  'Illustrative update at 11:00 on May 23, 2022: Summit has now acknowledged the
notice. It isolated 18 SNACK-A boxes and reports that 12 were already sold to
end customers. Whether those buyers have been notified is unknown. This
replaces the earlier Summit nonresponse. Harbor still has all 30 boxes
isolated.' \
  -q 'customers: What is the next action for Summit? [Confirm acknowledgment only,
    Escalate the 12 sold boxes and verify downstream notification,
    Treat the customer response as complete]'
```

```text
  turn 2 · 1 fact · 1 question
  customers     Confirm acknowledgment only      ▎                    0.01
              ▶ Escalate the 12 sold boxes and … ███████████████████▋ 0.98
                Treat the customer response as … ▎                    0.01
```

### What each decision was built on

A blend starts a session with the history of every session it names, so a blend of blends stands on everything behind both. The three blends behind the final response:

| Decision | Layer | Built on directly | Behind those | Answer |
| --- | --- | --- | --- | --- |
| `recall-stock-plan` | 1 | `recall-scope`, `recall-stock` | None | Quarantine matching jars and hold unreadable codes for identification · **1.00** |
| `recall-outreach-updated` | 1 | `recall-scope`, `recall-customers` turn 2 | `recall-customers` turn 1 | Escalate reported sales to the recall lead and verify downstream notification · **1.00** |
| `recall-response-updated` | 2 | `recall-stock-plan`, `recall-outreach-updated` | `recall-scope`, `recall-stock`, `recall-customers` turns 1 and 2 | Coordinate warehouse holds and escalation of sales to end customers · **1.00** |

<sub>Layers and inputs from `nouli view recall-response-updated`, answers as recorded. `recall-response-updated` stands on six earlier decisions: two directly, four behind them.</sub>

`nouli view recall-response-updated --mermaid` prints the whole history behind the final response, and GitHub draws it:

```mermaid
%% Nouli session: recall-response-updated (01a187b5-e29d-775b-be9b-429228ad43ff)
%%{init: {"flowchart": {"wrappingWidth": 320}}}%%
flowchart TD
  t0["<b>recall-scope · 01a187b5-e23</b><br/><br/>How should a jar be matched to this recall?<br/><br/><b>▶ Match the product and both lot-code conditions · 1</b><br/>Use the product name alone · 0<br/>Request a clearer recall notice · 0"]
  t1["<b>recall-stock · 01a187b5-e24</b><br/><br/>Can every jar in this inventory be matched to an identifiable lot?<br/><br/><b>▶ P(yes)=0.07</b>"]
  t2["<b>recall-stock-plan · 01a187b5-e26</b><br/><br/>What should the warehouse do with its stock?<br/><br/><b>▶ Quarantine matching jars and hold unreadable codes for identification · 1</b><br/>Hold every jar pending further review · 0<br/>Identify unreadable codes before choosing any stock action · 0"]
  t3["<b>recall-customers · 01a187b5-e25</b><br/><br/>Which customer needs a response follow-up first?<br/><br/>Contact Harbor · 0<br/><b>▶ Contact Summit · 1</b><br/>Contact both customers again · 0"]
  t4["<b>recall-customers · 01a187b5-e28</b><br/><br/>What is the next action for Summit?<br/><br/>Confirm acknowledgment only · 0.01<br/><b>▶ Escalate the 12 sold boxes and verify downstream notification · 0.98</b><br/>Treat the customer response as complete · 0.01"]
  t5["<b>recall-outreach-updated · 01a187b5-e29</b><br/><br/>What customer follow-up is needed now?<br/><br/>Ask Summit to acknowledge the notice and account for its boxes · 0<br/>Verify that both customers have isolated all affected boxes · 0<br/><b>▶ Escalate reported sales to the recall lead and verify downstream notification · 1</b>"]
  t6["<b>recall-response-updated · 01a187b5-e2a [latest]</b><br/><br/>Which response should the recall lead coordinate?<br/><br/>Coordinate warehouse holds only · 0<br/>Coordinate warehouse holds and missing customer responses · 0<br/><b>▶ Coordinate warehouse holds and escalation of sales to end customers · 1</b>"]
  t0 --> t2
  t1 --> t2
  t3 --> t4
  t0 --> t5
  t4 --> t5
  t2 --> t6
  t5 --> t6
  classDef latest stroke:#0b6e63,stroke-width:3px
  class t6 latest
```

The [recall walkthrough](https://nouli.ai/examples/recall/) goes through every step, and more [examples](https://nouli.ai/examples/) cover robotic picking, seed investing, lending, credit limits, vulnerability response, and energy planning. Each one shows its steps at the prompt or as commands.
