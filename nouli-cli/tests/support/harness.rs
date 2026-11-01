use super::loopback::Server;
use nouli_core::{
    context::ContextUpdate,
    data::Input,
    decision::{AnswerSpace, Criterion, Question, TurnRequest},
    history::{EvaluationRequest, ModelSelection, SessionSelection, StandingChanges},
    identity::Id,
    protocol::{EngineEvent, ErrorReport, EventKind, Report, ResultEnvelope, TurnReport},
};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
pub struct Home(pub PathBuf);
impl Home {
    pub fn create() -> Self {
        let home = Self(std::env::temp_dir().join(format!("nouli-cli-eval-{}", Id::generate())));
        std::fs::create_dir_all(&home.0).unwrap();
        home
    }
    pub fn store(&self) -> PathBuf {
        self.0.join("ledger")
    }
    pub fn ledger_path(&self) -> PathBuf {
        self.store().join("ledger.sqlite3")
    }
    pub fn bare_command(&self, store: bool) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_nouli"));
        command
            .env_clear()
            .current_dir(&self.0)
            .env("HOME", &self.0);
        if store {
            command.arg("--store").arg(self.store());
        }
        command
    }
    pub fn configure(&self, server: &Server, protocol: &str, credential: bool) {
        let protocol = match protocol {
            "typesafe" | "openrouter-decisions" => "system-one",
            other => other,
        };
        let config = format!(
            "default_provider = \"local\"\ndefault_model = \"offline\"\n[execution]\nmax_attempts = 1\nattempt_timeout_seconds = 5\n[providers.local]\nprotocol = \"{protocol}\"\nbase_url = \"{}\"\n{}\n[providers.local.models.offline]\ncontext_budget_tokens = 100000\ncontext_window_tokens = 100000\nreserved_output_tokens = 128\n",
            server.url,
            if credential {
                "api_key_env = \"NOULI_FIXTURE_KEY\"\n"
            } else {
                ""
            }
        );
        std::fs::write(self.0.join("config.toml"), config).unwrap();
    }
    pub fn command(&self) -> Command {
        self.command_at(&self.store())
    }
    pub fn command_at(&self, store: &std::path::Path) -> Command {
        let mut cmd = self.bare_command(false);
        cmd.arg("--config")
            .arg(self.0.join("config.toml"))
            .arg("--store")
            .arg(store);
        cmd
    }
    pub fn run<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        self.command().args(args).output().unwrap()
    }
    pub fn run_with_stdin<I, S>(&self, args: I, input: &[u8]) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    }
    /// Writes an `--input` request file. It names no session: the verb supplies it.
    pub fn request(&self, path: &str, marker: &str, updates: Vec<ContextUpdate>) -> PathBuf {
        let mut turn = turn();
        turn.state.push(Input::Text {
            text: marker.into(),
        });
        let request = EvaluationRequest {
            standing: StandingChanges::default(),
            turn,
            session: SessionSelection::New { name: None },
            updates,
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: None,
        };
        let mut value = serde_json::to_value(&request).unwrap();
        value.as_object_mut().unwrap().remove("session");
        let path = self.0.join(path);
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        path
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let result = std::fs::remove_dir_all(&self.0);
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}
/// The `TypeSafe` response for [`turn`] and [`QUESTIONS`]: score levels are plain
/// labels, which is all the compact question grammar can write.
pub const TYPESAFE: &str =
    include_str!("../../../fixtures/providers/typesafe/compact-response.json");

/// A `TypeSafe` provider that answers exactly the questions it is asked: from
/// [`TYPESAFE`] when it has them, and yes/no otherwise.
pub fn typesafe(body: &serde_json::Value) -> (u16, String) {
    let mut response: serde_json::Value = serde_json::from_str(TYPESAFE).unwrap();
    let asked = body["questions"].as_object().unwrap();
    let answers = response["answers"].as_object_mut().unwrap();
    answers.retain(|id, _| asked.contains_key(id));
    for id in asked.keys() {
        answers
            .entry(id.clone())
            .or_insert_with(|| serde_json::json!({"type": "noul", "noul": 0.5}));
    }
    (200, response.to_string())
}

/// Writes score levels as plain labels so the question matches the compact grammar.
fn plain(questions: &mut [Question]) {
    for question in questions {
        if let AnswerSpace::Score { levels } = &mut question.answer_space {
            for level in levels {
                if let Criterion::Detailed { label, .. } = level {
                    *level = Criterion::Text(std::mem::take(label));
                }
            }
        }
    }
}
pub fn turn() -> TurnRequest {
    let mut turn: TurnRequest = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/decision-request.json"
    ))
    .unwrap();
    turn.questions.truncate(3);
    plain(&mut turn.questions);
    turn
}
pub fn report(output: &Output) -> TurnReport {
    assert!(
        output.status.success(),
        "exit={:?},stdout={},stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let result: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Turn(turn) = result.result else {
        panic!("turn report")
    };
    *turn
}
/// The first three fixture questions in the compact grammar: a choice, a
/// yes/no question, and a score. A CLI turn asks one of them.
pub const QUESTIONS: [[&str; 2]; 3] = [
    ["-q", "travel: Which route? [train, flight]"],
    ["-q", "on_time: Will the train arrive on time?"],
    [
        "-q",
        "quality: Rate the route quality <Poor, Adequate, Excellent>",
    ],
];
pub const QUESTION: [&str; 2] = QUESTIONS[0];
pub fn workflow() -> nouli_core::workflow::WorkflowSpec {
    let mut definition: nouli_core::workflow::WorkflowSpec =
        serde_json::from_str(include_str!("../../../fixtures/contracts/workflow.json")).unwrap();
    for node in &mut definition.nodes {
        node.turn.questions.truncate(3);
        plain(&mut node.turn.questions);
        node.turn.state.push(Input::Text {
            text: format!("NODE:{}", node.id),
        });
    }
    definition
}
pub fn run_report(output: &Output, expected: u8) -> nouli_core::protocol::RunReport {
    assert_eq!(
        output.status.code(),
        Some(i32::from(expected)),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Run(report) = envelope.result else {
        panic!("run report")
    };
    report
}
/// The ledger database's bytes, to show an operation left the store unchanged.
pub fn ledger_bytes(home: &Home) -> Vec<u8> {
    std::fs::read(home.ledger_path()).unwrap()
}
pub fn context_record(
    key: &str,
    value: serde_json::Value,
    kind: nouli_core::context::ContextKind,
) -> nouli_core::context::ContextRecord {
    nouli_core::context::ContextRecord {
        id: Id::generate(),
        key: key.into(),
        value: nouli_core::data::Data::new(value).unwrap(),
        kind,
    }
}
/// The engine events a `--jsonl` command wrote, checked to form a valid
/// stream. Callers check the exit status themselves.
pub fn jsonl_events(output: &Output) -> Vec<nouli_core::protocol::EngineEvent> {
    jsonl_events_from(&output.stdout)
}

pub fn jsonl_events_from(bytes: &[u8]) -> Vec<EngineEvent> {
    let events = std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect::<Vec<_>>();
    nouli_core::protocol::validate_events(&events).unwrap();
    events
}

pub fn terminal(events: &[EngineEvent]) -> &ResultEnvelope {
    let EventKind::Terminal { result } = &events.last().unwrap().event else {
        panic!("terminal event")
    };
    result
}

pub fn error_report(output: &Output, exit: i32, code: Option<&str>) -> ErrorReport {
    assert_eq!(output.status.code(), Some(exit));
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Error(error) = envelope.result else {
        panic!("error report")
    };
    if let Some(code) = code {
        assert_eq!(error.code, code);
    }
    error
}

pub fn session_count(home: &Home) -> usize {
    nouli_core::storage::Ledger::open(
        &home.store(),
        nouli_core::storage::OpenMode::ReadOnlyExisting,
    )
    .unwrap()
    .list_sessions()
    .unwrap()
    .len()
}
