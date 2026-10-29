use super::loopback::Server;
use nouli_core::{
    context::ContextUpdate,
    data::Input,
    decision::TurnRequest,
    history::{EvaluationRequest, ModelSelection, SessionSelection},
    identity::Id,
    protocol::{Report, ResultEnvelope, TurnReport},
};
use std::{
    path::PathBuf,
    process::{Command, Output},
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
    pub fn configure(&self, server: &Server, protocol: &str, credential: bool) {
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
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_nouli"));
        cmd.env_clear()
            .current_dir(&self.0)
            .env("HOME", &self.0)
            .arg("--config")
            .arg(self.0.join("config.toml"))
            .arg("--store")
            .arg(store);
        cmd
    }
    pub fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
    pub fn request(
        &self,
        path: &str,
        selection: SessionSelection,
        marker: &str,
        updates: Vec<ContextUpdate>,
    ) -> PathBuf {
        let mut turn = turn();
        turn.state.push(Input::Text {
            text: marker.into(),
        });
        let request = EvaluationRequest {
            standing: Default::default(),
            turn,
            session: selection,
            updates,
            required_records: vec![],
            model: ModelSelection::default(),
            request_id: None,
        };
        let path = self.0.join(path);
        std::fs::write(&path, serde_json::to_vec_pretty(&request).unwrap()).unwrap();
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
pub fn turn() -> TurnRequest {
    let mut turn: TurnRequest = serde_json::from_str(include_str!(
        "../../../fixtures/contracts/decision-request.json"
    ))
    .unwrap();
    turn.questions.truncate(3);
    turn
}
pub fn report(output: Output) -> TurnReport {
    assert!(
        output.status.success(),
        "exit={:?},stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let result: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Turn(turn) = result.result else {
        panic!("turn report")
    };
    *turn
}
pub fn questions() -> String {
    serde_json::to_string(&turn().questions).unwrap()
}
pub fn workflow() -> nouli_core::workflow::WorkflowSpec {
    let mut definition: nouli_core::workflow::WorkflowSpec =
        serde_json::from_str(include_str!("../../../fixtures/contracts/workflow.json")).unwrap();
    for node in &mut definition.nodes {
        node.turn.questions.truncate(3);
        node.turn.state.push(Input::Text {
            text: format!("NODE:{}", node.id),
        });
    }
    definition
}
pub fn run_report(output: Output, expected: u8) -> nouli_core::protocol::RunReport {
    assert_eq!(
        output.status.code(),
        Some(i32::from(expected)),
        "stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let envelope: ResultEnvelope = serde_json::from_slice(&output.stdout).unwrap();
    let Report::Run(report) = envelope.result else {
        panic!("run report")
    };
    report
}
