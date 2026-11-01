# Nouli: Every decision has a history.

**Keep the evidence. Build on the answer.**

Keep the facts behind a decision, add what changes, and ask the next question without losing the earlier answer. Nouli is a terminal app, CLI, and Rust library for connected decision histories: a ledger-backed decision network. Use a compatible decision service, or follow a task-specific model’s local recipe. Use the CLI to run individual model requests with a recorded decision history.

[![CI](https://img.shields.io/github/actions/workflow/status/nouli-ai/nouli/ci.yml?branch=main&style=flat-square&logo=githubactions&logoColor=white)](https://github.com/nouli-ai/nouli/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/nouli-ai/nouli?include_prereleases&sort=semver&style=flat-square&color=blue&logo=github)](https://github.com/nouli-ai/nouli/releases) [![Total downloads across all release assets](https://img.shields.io/github/downloads/nouli-ai/nouli/total?style=flat-square&color=blueviolet)](https://github.com/nouli-ai/nouli/releases) [![License: MIT](https://img.shields.io/badge/license-MIT-yellow?style=flat-square&logo=opensourceinitiative&logoColor=white)](LICENSE) [![Rust 1.99.1+](https://img.shields.io/badge/rust-1.99.1%2B-dea584?style=flat-square&logo=rust&logoColor=white)](rust-toolchain.toml)

[Website](https://nouli.ai/) · [Models](#models-from-nouli) · [Quickstart](#quickstart) · [Examples](https://nouli.ai/examples/) · [User guide](https://nouli.ai/guide/)

## See Nouli in action

[![Nouli TUI replay: saved decisions, branches, and blends.](.github/readme/tui-replay.gif)](https://nouli.ai/#cli-replay)

*Recorded workshop replay using saved model answers, at 4× speed.*

## Quickstart

Install the CLI on Apple silicon macOS:

```sh
brew install nouli-ai/tap/nouli
```

On GNU/Linux, or Apple silicon macOS without Homebrew:

```sh
curl -fsSL https://nouli.ai/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"
```

Then choose a starting path:

- **Run a Nouli model locally:** choose a [task-specific model](#models-from-nouli) and follow its model card to set up the environment, load weights, start the local service, and run the supplied CLI workflow.
- **Ask broader decision questions:** connect an appropriate general-purpose decision service, choose a model, then open the terminal app:

```sh
nouli connect
nouli
```

In the app, add context and a question, then choose **Yes / No**, **Choose one**, or **Score**. **Shift-Tab** changes the answer type; **Tab** moves between fields. Press **Enter** to send from Context or Question, or **Enter twice** from Options or Levels. Use `/exit` to return to your shell. [Terminal guide](docs/cli.md#full-screen-view).

A service must support System One or OpenAI Decisions. Provider fees may apply to remote inference. [Connection setup](docs/configuration.md).

Prebuilt CLI binaries support Apple silicon macOS and ARM64/x86_64 GNU/Linux with glibc 2.38+. The download installer checks its checksum and installs into `~/.local/bin` by default. Each local model has its own runtime requirements. [Installation and upgrades](docs/release.md).

## Models from Nouli

We also publish five task-specific decision models in the [nouli-ai Hugging Face organization](https://huggingface.co/nouli-ai). Each includes weights, a local serving recipe, and a Nouli CLI workflow.

| Model | What it does |
| --- | --- |
| [Nouli Guard](https://huggingface.co/nouli-ai/nouli-guard) | English hate-speech classification: no, unclear, or yes. |
| [Nouli Route](https://huggingface.co/nouli-ai/nouli-route) | English banking intent classification across 77 BANKING77 labels. |
| [Nouli Clause](https://huggingface.co/nouli-ai/nouli-clause) | NDA evidence retrieval and a fixed 17-hypothesis checklist. |
| [Nouli Push](https://huggingface.co/nouli-ai/nouli-push) | XY actions from keypoints and history in the PushT simulator. |
| [Nouli Doom](https://huggingface.co/nouli-ai/nouli-doom) | Structured-state actions for ViZDoom Deadly Corridor. |

To try one, follow its model card’s **Quickstart with Nouli CLI**: prepare the documented input, start its local server, and run its supplied workflow. The recipes use Python 3.12 and the Hugging Face CLI and do not need a hosted inference API key. Installing Nouli alone does not download or load these models.

These models use task-specific inputs and answer schemas. Their cards include evaluation details, supported runtimes, and limits. Model distributions are **Apache-2.0** with upstream notices; the CLI and Rust library are **MIT**.

## Add one fact. Make the next decision.

After [setup](#quickstart), use an appropriate general-purpose decision service and a fresh session name. The five specialized local models use their own supplied workflows. Try this from your shell:

```sh
nouli new trip \
  'The train costs 40 USD. A taxi costs 70 USD. Both arrive on time. Prefer the cheaper option.' \
  -q 'travel: How should I get to the meeting? [Take the train, Take a taxi]'

nouli add trip 'The train has been cancelled.' \
  -q 'travel: How should I get to the meeting? [Take the train, Take a taxi]'

nouli view trip
```

The second command adds only the new fact and asks the question again. Nouli saves a new decision and preserves the first answer. Results depend on the configured model.

Run `nouli context trip` to inspect the context sent for the latest turn. [See the recorded meeting example](https://nouli.ai/examples/meeting/) for a three-turn walkthrough.

## Continue, branch, blend, inspect

| When you need to… | Use Nouli to… |
| --- | --- |
| Decide again after new evidence | **Continue** a saved session with what changed. |
| Explore an alternative | **Branch** from the same starting point and keep both paths. |
| Bring separate work together | **Blend** source histories into a new session. |
| Understand a saved decision’s inputs | **Inspect** its facts, actual model context, and session ancestry. |

With an appropriate decision service configured ([setup](#quickstart)), start with a workshop brief, branch into venue and program decisions, then blend their histories into a plan:

```mermaid
flowchart LR
  workshop[Workshop brief] --> venue[Venue decision]
  workshop --> program[Program decision]
  venue --> plan[Combined plan]
  program --> plan
```

<details>
<summary>Run the branch-and-blend example</summary>

```sh
nouli new workshop \
  'Plan a step-free workshop for 60 people. Budget: 6000 USD.' \
  -q 'feasible: Can we plan this workshop?'

nouli new venue --from workshop \
  'Riverside Hall is step-free, fits 80 and costs 1200 USD. The Loft costs 900 USD but has stairs.' \
  -q 'venue: Which venue should we choose? [Riverside Hall, The Loft]'

nouli new program --from workshop \
  'Two speakers are available. Include breaks and useful hands-on time. The program costs 500 USD.' \
  -q 'program: Which program should we run? [Talks and practice, Talks only]'

nouli new plan --from venue --from program \
  'The venue and speakers can confirm today. Total event costs are 3500 USD, including lunch, materials and transport.' \
  -q 'confirm: Should we confirm the plan? [Confirm the plan, Revise the plan]'

nouli view plan
```

These commands create the session structure above. Each model call saves its own answer; the diagram shows ancestry, not predicted results.

</details>

A branch or blend captures its sources at creation. Later source updates do not silently change it. In the terminal app, `@` also lets you reuse an exact saved answer from another session. [Branching and blending](https://nouli.ai/guide/#branch).

## Use the same history from scripts and agents

After the trip example, read its saved decisions and context as JSON:

```sh
nouli sessions export trip --format json
nouli context trip --json
```

Any coding agent that can run shell commands can use the CLI. Start it with the [agent reference](https://nouli.ai/llms.txt). Use [request files](docs/cli.md#request-files) for structured input and [workflows](docs/cli-workflows.md) for dependent decisions, or embed the [Rust library](docs/embedding.md) in your application.
The Hugging Face releases also include task-specific workflow files and local System One server adapters. Use each model’s documented input schema when running those workflows.

## Examples with recorded answers

- [Meeting travel](https://nouli.ai/examples/meeting/): continue as circumstances change.
- [Robotic picking](https://nouli.ai/examples/robot-picking/): combine item history with station rules.
- [Product recall](https://nouli.ai/examples/recall/): combine three source sessions through two layers of blends. [Inspect the recorded graph](docs/recall-example.md).
- [More walkthroughs](https://nouli.ai/examples/): investing, lending, credit limits, vulnerability response, and energy planning.

The walkthroughs preserve recorded model answers, including mistakes where they occur.

## Questions before you try it

### What kinds of answers can Nouli return?

A decision model answers against a defined answer space. **Yes / No** returns the probability of yes. **Choose one** returns an option and its distribution. **Score** returns an expected index on an ordered rubric, plus level probabilities. These are model judgments. [Question contracts](docs/contracts/decisions.md).

### How do the five Nouli models fit in?

Each release includes a local System One adapter and a workflow for its supported task. Follow the model card for input preparation, dependencies, hardware, and serving. For general examples such as the trip and workshop, use a decision service that supports those broader questions.

### Which other providers work?

Compatible `system-one` and `openai-decisions` APIs. `nouli connect` offers setup shortcuts for OpenAI, TypeSafe, OpenRouter, Cloudflare Workers AI, Ollama, llama.cpp, and custom endpoints. The chosen service and model need to support the decision protocol. [Configuration](docs/configuration.md).

### Where does my data go?

History is stored locally, by default under `~/.nouli`. Inference sends selected context to the provider you configure; the Hugging Face recipes run a local provider. Context budgets can limit what fits. `nouli context SESSION` shows the actual input. [Storage and connections](docs/configuration.md).

### Does Nouli rewrite earlier decisions?

Saved answers stay as recorded. New evidence and questions create new turns, even when you repeat the question. Branches and blends retain the source history they captured.

### What does it cost, and what are the licenses?

The Nouli CLI and Rust library are [MIT-licensed](LICENSE). The model distributions use Apache-2.0 with the upstream notices in their repositories. Local recipes need your own compute; remote providers may charge for inference. No model weights or API credit are bundled with the CLI.

## Documentation and contributing

[User guide](https://nouli.ai/guide/) · [CLI reference](docs/cli.md) · [Configuration](docs/configuration.md) · [Output formats](docs/output.md) · [Rust embedding](docs/embedding.md)

Use the pinned Rust toolchain and run:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

Tests use fixture providers and need no API keys. See [repository conventions](AGENTS.md), [architecture decisions](docs/adr/), and [website development](site/README.md).

---

If decision history would help something you’re building, [star Nouli](https://github.com/nouli-ai/nouli) to find it again, or [share your use case](https://github.com/nouli-ai/nouli/issues).

Made by [Nouli](https://nouli.ai/) · [CLI license: MIT](LICENSE) · [Models and their notices](https://huggingface.co/nouli-ai)
