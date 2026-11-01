//! Application output policy over explicit sinks, with no logging subscriber.
mod media;
mod text;
pub(crate) mod turn;
use clap::{Args, ValueEnum};
pub(crate) use media::{image_label_projection, image_labels};
use nouli_core::{
    codec,
    config::OutputFormat,
    decision::CONTRACT_VERSION,
    identity::Id,
    protocol::{
        EngineEvent, ErrorCategory, ErrorReport, EventKind, Report, ResultEnvelope,
        validate_terminal,
    },
    workflow::RunStatus,
};
use serde::Serialize;
use std::io::Write;
pub(crate) use text::clean;
use thiserror::Error;
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
    Auto,
    Text,
    Json,
    Jsonl,
    Toon,
}
#[derive(Debug, Default, Args)]
pub(crate) struct OutputArgs {
    /// Emit a version-1 final JSON report.
    #[arg(long,global=true,conflicts_with_all=["format","jsonl"])]
    pub json: bool,
    /// Select text, json, jsonl, toon, or auto (text in a terminal, JSON otherwise).
    #[arg(long, global = true, value_enum, conflicts_with = "jsonl")]
    pub format: Option<Format>,
    /// Emit JSONL events ending with one result or error.
    #[arg(long, global = true)]
    pub jsonl: bool,
}
#[derive(Debug, Error)]
pub(crate) enum RenderError {
    #[error("conflicting output modes")]
    Modes,
    #[error("invalid report or event stream")]
    Protocol,
    #[error(transparent)]
    Evidence(#[from] nouli_core::context::evidence::EvidenceError),
    #[error("cannot serialize report")]
    Codec(#[from] nouli_core::codec::CodecError),
    #[error("cannot write output")]
    Write(#[from] std::io::Error),
    #[error("cannot encode JSON")]
    Json(#[from] serde_json::Error),
}
pub(crate) fn resolve(
    args: &OutputArgs,
    configured: OutputFormat,
    terminal: bool,
) -> Result<Format, RenderError> {
    if usize::from(args.json) + usize::from(args.jsonl) + usize::from(args.format.is_some()) > 1 {
        return Err(RenderError::Modes);
    }
    let selected = if args.json {
        Format::Json
    } else if args.jsonl {
        Format::Jsonl
    } else {
        args.format.unwrap_or(match configured {
            OutputFormat::Auto => Format::Auto,
            OutputFormat::Text => Format::Text,
            OutputFormat::Json => Format::Json,
        })
    };
    Ok(if selected == Format::Auto {
        if terminal { Format::Text } else { Format::Json }
    } else {
        selected
    })
}
fn input(code: &str, message: &str) -> ErrorReport {
    ErrorReport {
        category: ErrorCategory::Input,
        code: code.into(),
        message: message.into(),
        retryable: false,
    }
}
/// Resolves the format of a command that may stream events, reporting
/// conflicting output flags as an input error.
pub(crate) fn resolve_stream_format(
    args: &OutputArgs,
    configured: OutputFormat,
    terminal: bool,
) -> Result<Format, ErrorReport> {
    resolve(args, configured, terminal).map_err(|_| {
        input(
            "conflicting_output_options",
            "Choose only one of --json, --jsonl, or --format",
        )
    })
}
/// Resolves the format of a command that reports once rather than streaming
/// events, so JSONL is refused with an input error.
pub(crate) fn resolve_report_format(
    args: &OutputArgs,
    configured: OutputFormat,
    terminal: bool,
) -> Result<Format, ErrorReport> {
    match resolve_stream_format(args, configured, terminal)? {
        Format::Jsonl => Err(input(
            "unsupported_output_format",
            "This command supports text, JSON, or TOON output",
        )),
        format => Ok(format),
    }
}
/// Color only on a terminal, with `NO_COLOR` unset or empty and `TERM` not `dumb`.
pub(crate) fn color_enabled(terminal: bool) -> bool {
    color_for(
        terminal,
        std::env::var_os("NO_COLOR").as_deref(),
        std::env::var_os("TERM").as_deref(),
    )
}
fn color_for(
    terminal: bool,
    no_color: Option<&std::ffi::OsStr>,
    term: Option<&std::ffi::OsStr>,
) -> bool {
    terminal && no_color.is_none_or(std::ffi::OsStr::is_empty) && !plain_terminal(term)
}
/// Whether `TERM` names a terminal that takes no escape codes, such as an
/// Emacs shell buffer.
pub(crate) fn plain_terminal(term: Option<&std::ffi::OsStr>) -> bool {
    term.is_some_and(|term| ["dumb", "cons25", "emacs"].iter().any(|name| term == *name))
}
pub(crate) fn exit_code(result: &ResultEnvelope) -> u8 {
    match &result.result {
        Report::Turn(_) | Report::Session(_) => 0,
        Report::Run(run) => match run.status {
            RunStatus::Completed => 0,
            RunStatus::Failed | RunStatus::Partial => 4,
            RunStatus::Pending | RunStatus::Running | RunStatus::Interrupted => 130,
        },
        Report::Error(error) => match error.category {
            ErrorCategory::Input | ErrorCategory::Configuration => 2,
            ErrorCategory::Authentication
            | ErrorCategory::Capability
            | ErrorCategory::Contract
            | ErrorCategory::Capacity
            | ErrorCategory::Transport => 3,
            ErrorCategory::Conflict => 5,
            ErrorCategory::Storage | ErrorCategory::Internal => 6,
            ErrorCategory::Interrupted => 130,
        },
    }
}
pub(crate) fn diagnostic(sink: &mut impl Write, error: &ErrorReport) -> Result<(), RenderError> {
    writeln!(
        sink,
        "{}: {}",
        text::clean(&error.code),
        text::clean(&error.message)
    )?;
    Ok(())
}
pub(crate) fn labelled(
    label: &str,
    value: &impl Serialize,
    sink: &mut impl Write,
    format: Format,
) -> Result<(), RenderError> {
    // JSON keeps declaration/insertion order; TOON canonicalizes keys itself.
    match format {
        Format::Json => {
            serde_json::to_writer(&mut *sink, value)?;
            sink.write_all(b"\n")?;
        }
        Format::Toon | Format::Text => {
            if format == Format::Text {
                writeln!(sink, "{label}")?;
            }
            sink.write_all(codec::encode_toon(value)?.as_bytes())?;
            sink.write_all(b"\n")?;
        }
        Format::Auto | Format::Jsonl => return Err(RenderError::Modes),
    }
    sink.flush()?;
    Ok(())
}

pub(crate) fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

fn json_value(sink: &mut impl Write, value: &serde_json::Value) -> Result<(), RenderError> {
    serde_json::to_writer(&mut *sink, &value)?;
    sink.write_all(b"\n")?;
    sink.flush()?;
    Ok(())
}
pub(crate) fn result(
    stdout: &mut impl Write,
    stderr: &mut impl Write,
    format: Format,
    result: &ResultEnvelope,
) -> Result<u8, RenderError> {
    if result.version != CONTRACT_VERSION {
        return Err(RenderError::Protocol);
    }
    let encoded = codec::to_value(result)?;
    match format {
        Format::Auto => return Err(RenderError::Modes),
        Format::Text => match &result.result {
            Report::Error(error) => diagnostic(stderr, error)?,
            _ => text::result(stdout, result)?,
        },
        Format::Json => json_value(stdout, &encoded)?,
        Format::Toon => {
            stdout.write_all(codec::encode_toon(&encoded)?.as_bytes())?;
            stdout.write_all(b"\n")?;
            stdout.flush()?;
        }
        Format::Jsonl => {
            let run = match &result.result {
                Report::Run(run) => run.run,
                _ => result.recovery.as_ref().and_then(|reference| reference.run),
            };
            let mut writer = JsonlWriter::new(stdout);
            writer.event(&EngineEvent {
                version: CONTRACT_VERSION,
                sequence: 0,
                run,
                event: EventKind::Started,
            })?;
            writer.event(&EngineEvent {
                version: CONTRACT_VERSION,
                sequence: 1,
                run,
                event: EventKind::Terminal {
                    result: Box::new(result.clone()),
                },
            })?;
        }
    }
    Ok(exit_code(result))
}
/// One invocation owns a writer. Failed writes poison it so retries cannot
/// duplicate a partial line or a terminal event.
pub(crate) struct JsonlWriter<W> {
    sink: W,
    next: u64,
    run: Option<Id>,
    terminal: bool,
    failed: bool,
}
impl<W: Write> JsonlWriter<W> {
    pub(crate) fn new(sink: W) -> Self {
        Self {
            sink,
            next: 0,
            run: None,
            terminal: false,
            failed: false,
        }
    }
    pub(crate) fn event(&mut self, event: &EngineEvent) -> Result<Option<u8>, RenderError> {
        if self.failed
            || self.terminal
            || event.version != CONTRACT_VERSION
            || event.sequence != self.next
        {
            return Err(RenderError::Protocol);
        }
        if self.next == 0 {
            if !matches!(event.event, EventKind::Started) {
                return Err(RenderError::Protocol);
            }
            self.run = event.run;
        } else if event.run != self.run || matches!(event.event, EventKind::Started) {
            return Err(RenderError::Protocol);
        }
        let code = if let EventKind::Terminal { result } = &event.event {
            validate_terminal(result, self.run).map_err(|_| RenderError::Protocol)?;
            self.terminal = true;
            Some(exit_code(result))
        } else {
            None
        };
        let value = codec::to_value(event)?;
        self.next = self.next.checked_add(1).ok_or(RenderError::Protocol)?;
        if let Err(error) = json_value(&mut self.sink, &value) {
            self.failed = true;
            return Err(error);
        }
        Ok(code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nouli_core::{
        decision::TurnRequest,
        history::FinalizedTurn,
        outcome::{Answer, QuestionOutcome, TurnOutcome},
        protocol::{TurnReport, validate_events},
    };
    #[test]
    fn report_formats_refuse_jsonl_and_keep_the_others() {
        let resolve = |args: OutputArgs, configured| {
            resolve_report_format(&args, configured, false).map_err(|error| error.code)
        };
        assert_eq!(
            resolve(
                OutputArgs {
                    jsonl: true,
                    ..Default::default()
                },
                OutputFormat::Auto
            ),
            Err("unsupported_output_format".into())
        );
        assert_eq!(
            resolve(
                OutputArgs {
                    format: Some(Format::Jsonl),
                    ..Default::default()
                },
                OutputFormat::Text
            ),
            Err("unsupported_output_format".into())
        );
        assert_eq!(
            resolve(OutputArgs::default(), OutputFormat::Auto),
            Ok(Format::Json)
        );
        assert_eq!(
            resolve(OutputArgs::default(), OutputFormat::Text),
            Ok(Format::Text)
        );
        assert_eq!(
            resolve(
                OutputArgs {
                    format: Some(Format::Toon),
                    ..Default::default()
                },
                OutputFormat::Auto
            ),
            Ok(Format::Toon)
        );
        assert_eq!(
            resolve(
                OutputArgs {
                    json: true,
                    jsonl: true,
                    ..Default::default()
                },
                OutputFormat::Auto
            ),
            Err("conflicting_output_options".into())
        );
    }
    #[test]
    fn color_needs_a_terminal_a_clear_no_color_and_a_capable_term() {
        use std::ffi::OsStr;
        let on = |terminal, no_color: Option<&str>, term: Option<&str>| {
            color_for(terminal, no_color.map(OsStr::new), term.map(OsStr::new))
        };
        assert!(on(true, None, None));
        assert!(on(true, Some(""), Some("xterm-256color")));
        assert!(!on(false, None, Some("xterm")));
        assert!(!on(true, Some("1"), Some("xterm")));
        assert!(!on(true, None, Some("dumb")));
        assert!(!on(true, None, Some("emacs")));
        assert!(plain_terminal(Some(OsStr::new("cons25"))));
        assert!(!plain_terminal(Some(OsStr::new("xterm-256color"))));
        assert!(!plain_terminal(None));
    }
    fn turn() -> ResultEnvelope {
        let history: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/contracts/history.json")).unwrap();
        let turn: FinalizedTurn = serde_json::from_value(history["turns"][0].clone()).unwrap();
        let request: TurnRequest = serde_json::from_str(include_str!(
            "../../fixtures/contracts/decision-request.json"
        ))
        .unwrap();
        let outcome: TurnOutcome = serde_json::from_str(include_str!(
            "../../fixtures/contracts/decision-outcome.json"
        ))
        .unwrap();
        ResultEnvelope::new(Report::Turn(Box::new(TurnReport {
            questions: request.questions,
            session: Some(turn.session),
            turn: Some(turn.id),
            outcome,
            manifest: turn.manifest,
            replayed: true,
        })))
    }
    fn error(category: ErrorCategory) -> ResultEnvelope {
        ResultEnvelope::new(Report::Error(ErrorReport {
            category,
            code: "fixture_error".into(),
            message: "Fixture error".into(),
            retryable: false,
        }))
    }
    fn events() -> Vec<EngineEvent> {
        serde_json::from_str(include_str!("../../fixtures/contracts/events.json")).unwrap()
    }
    #[test]
    fn injected_terminal_detection_and_explicit_overrides_resolve_without_environment_reads() {
        for terminal in [false, true] {
            assert_eq!(
                resolve(&OutputArgs::default(), OutputFormat::Auto, terminal).unwrap(),
                if terminal { Format::Text } else { Format::Json }
            );
            assert_eq!(
                resolve(&OutputArgs::default(), OutputFormat::Json, terminal).unwrap(),
                Format::Json
            );
            assert_eq!(
                resolve(
                    &OutputArgs {
                        format: Some(Format::Text),
                        ..Default::default()
                    },
                    OutputFormat::Json,
                    terminal
                )
                .unwrap(),
                Format::Text
            );
            assert_eq!(
                resolve(
                    &OutputArgs {
                        json: true,
                        ..Default::default()
                    },
                    OutputFormat::Text,
                    terminal
                )
                .unwrap(),
                Format::Json
            );
            assert_eq!(
                resolve(
                    &OutputArgs {
                        jsonl: true,
                        ..Default::default()
                    },
                    OutputFormat::Text,
                    terminal
                )
                .unwrap(),
                Format::Jsonl
            );
        }
        assert!(
            resolve(
                &OutputArgs {
                    json: true,
                    jsonl: true,
                    ..Default::default()
                },
                OutputFormat::Auto,
                false
            )
            .is_err()
        );
        assert!(
            <super::super::Args as clap::Parser>::try_parse_from(["nouli", "--json", "--jsonl"])
                .is_err()
        );
    }
    #[test]
    fn all_primitives_mixed_refusal_and_native_confidence_have_self_describing_text() {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            result(&mut stdout, &mut stderr, Format::Text, &turn()).unwrap(),
            0
        );
        let text = String::from_utf8(stdout).unwrap();
        assert_eq!(String::from_utf8_lossy(&stderr), "");
        for expected in [
            "train (Rail)",
            "P(selected)=0.7",
            "noul: P(yes)=1",
            "expected score: 1.43",
            "scale 0..2",
            "0: Poor",
            "1: Adequate (Meets minimum requirements)",
            "2: Excellent",
            "refused:",
            "Native confidence (fixture/offline, fixture native certainty)",
            "value=0.8",
            "(replayed)",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        assert!(!text.contains('\u{1b}'));
    }
    #[test]
    fn final_json_and_jsonl_are_parseable_v1_and_keep_diagnostics_separate() {
        for format in [Format::Json, Format::Jsonl] {
            let report = turn();
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            assert_eq!(
                result(&mut stdout, &mut stderr, format, &report).unwrap(),
                0
            );
            assert_eq!(String::from_utf8_lossy(&stderr), "");
            diagnostic(
                &mut stderr,
                &ErrorReport {
                    category: ErrorCategory::Internal,
                    code: "test_diagnostic".into(),
                    message: "separate diagnostic".into(),
                    retryable: false,
                },
            )
            .unwrap();
            assert!(
                !stdout
                    .windows(b"separate diagnostic".len())
                    .any(|part| part == b"separate diagnostic")
            );
            if format == Format::Json {
                assert_eq!(
                    serde_json::from_slice::<ResultEnvelope>(&stdout).unwrap(),
                    report
                );
            } else {
                let events = String::from_utf8(stdout)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str::<EngineEvent>(line).unwrap())
                    .collect::<Vec<_>>();
                validate_events(&events).unwrap();
                assert_eq!(events.len(), 2);
            }
        }
    }
    #[test]
    fn partial_interruption_capacity_conflict_storage_and_input_have_truthful_exits() {
        let mut events = events();
        let mut stdout = Vec::new();
        {
            let mut writer = JsonlWriter::new(&mut stdout);
            let mut terminal = None;
            for event in &events {
                terminal = writer.event(event).unwrap().or(terminal);
            }
            assert_eq!(terminal, Some(4));
            let bytes = writer.sink.len();
            assert!(writer.event(events.last().unwrap()).is_err());
            assert_eq!(writer.sink.len(), bytes);
        }
        let rendered = String::from_utf8(stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<EngineEvent>(line).unwrap())
            .collect::<Vec<_>>();
        validate_events(&rendered).unwrap();
        let EventKind::Terminal { result: report } = &mut events.last_mut().unwrap().event else {
            panic!("terminal")
        };
        let Report::Run(run) = &mut report.result else {
            panic!("run")
        };
        run.status = RunStatus::Interrupted;
        let mut stderr = Vec::new();
        let mut stdout = Vec::new();
        assert_eq!(
            result(&mut stdout, &mut stderr, Format::Json, report).unwrap(),
            130
        );
        for (category, code) in [
            (ErrorCategory::Input, 2),
            (ErrorCategory::Capacity, 3),
            (ErrorCategory::Conflict, 5),
            (ErrorCategory::Storage, 6),
            (ErrorCategory::Interrupted, 130),
        ] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            assert_eq!(
                result(&mut stdout, &mut stderr, Format::Json, &error(category)).unwrap(),
                code
            );
            assert_eq!(String::from_utf8_lossy(&stderr), "");
            assert!(matches!(
                serde_json::from_slice::<ResultEnvelope>(&stdout)
                    .unwrap()
                    .result,
                Report::Error(_)
            ));
        }
    }
    #[test]
    fn safe_errors_do_not_echo_secret_paths_and_text_escapes_control_characters() {
        let report =
            nouli_core::engine::EngineError::Config(nouli_core::config::ConfigError::Read {
                path: "SECRET_CREDENTIAL_VALUE".into(),
                source: std::io::Error::other("SECRET_RESPONSE_BODY"),
            })
            .report();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        assert_eq!(
            result(
                &mut stdout,
                &mut stderr,
                Format::Text,
                &ResultEnvelope::new(Report::Error(report))
            )
            .unwrap(),
            2
        );
        assert_eq!(String::from_utf8_lossy(&stdout), "");
        assert!(!String::from_utf8(stderr).unwrap().contains("SECRET_"));
        let mut report = turn();
        let Report::Turn(turn) = &mut report.result else {
            panic!("turn")
        };
        let nouli_core::decision::AnswerSpace::Choice { options } =
            &mut turn.questions[0].answer_space
        else {
            panic!("choice")
        };
        options[0].description = "Rail\u{1b}[31m".into();
        let mut stdout = Vec::new();
        result(&mut stdout, &mut Vec::new(), Format::Text, &report).unwrap();
        assert!(!stdout.contains(&0x1b));
        let Report::Turn(turn) = &mut report.result else {
            panic!("turn")
        };
        let QuestionOutcome::Answered {
            answer: Answer::Noul { probability },
            ..
        } = &mut turn.outcome.outcomes[1]
        else {
            panic!("noul")
        };
        *probability = f64::NAN;
        for format in [Format::Text, Format::Json, Format::Jsonl] {
            let mut stdout = Vec::new();
            assert!(result(&mut stdout, &mut Vec::new(), format, &report).is_err());
            assert_eq!(String::from_utf8_lossy(&stdout), "");
        }
    }
    struct Broken {
        remaining: usize,
    }
    impl Write for Broken {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            let count = bytes.len().min(self.remaining);
            self.remaining -= count;
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn partial_write_failure_poisoning_prevents_a_repeated_terminal_attempt() {
        let mut writer = JsonlWriter::new(Broken { remaining: 1 });
        let events = events();
        assert!(writer.event(&events[0]).is_err());
        assert!(writer.failed);
        assert!(writer.event(&events[0]).is_err());
        assert!(writer.event(events.last().unwrap()).is_err());
    }
    /// The contract workflow with outputs on a skipped node, so every output
    /// falls back to its caller-supplied default.
    fn workflow_with_defaulted_outputs() -> nouli_core::workflow::WorkflowSpec {
        use nouli_core::{
            data::Data,
            workflow::{
                OutputBinding, WorkflowSpec, compile,
                reference::{MissingPolicy, ResultField, ResultReference},
            },
        };
        let mut definition: WorkflowSpec =
            serde_json::from_str(include_str!("../../fixtures/contracts/workflow.json")).unwrap();
        definition.outputs = vec![
            (
                "fallback",
                "private",
                ResultField::Outcome,
                serde_json::json!({"unavailable":true}),
            ),
            (
                "probability",
                "on_time",
                ResultField::Probability,
                serde_json::json!(0.2),
            ),
            (
                "score",
                "quality",
                ResultField::Score,
                serde_json::json!(2.0),
            ),
        ]
        .into_iter()
        .map(|(name, question, field, value)| OutputBinding {
            name: name.into(),
            reference: ResultReference {
                node: "low".into(),
                question: question.into(),
                field,
                pointer: None,
                on_missing: MissingPolicy::Default {
                    value: Data::new(value).unwrap(),
                },
            },
        })
        .collect();
        compile::compile(&definition).unwrap();
        definition
    }
    /// A completed run of `definition` whose third node was skipped and whose
    /// outputs all resolved to their defaults.
    fn defaulted_run(definition: &nouli_core::workflow::WorkflowSpec) -> ResultEnvelope {
        use nouli_core::{
            engine::workflow::WorkflowResult,
            engine::workflow::reporting as projection,
            workflow::{
                NodeStatus, resolve,
                schedule::{NodeOutput, NodeProgress, ResolvedBinding, ScheduleResult},
            },
        };
        let Report::Turn(turn) = turn().result else {
            panic!("turn")
        };
        let nodes = (0..5)
            .map(|index| NodeProgress {
                status: if index == 2 {
                    NodeStatus::Skipped
                } else {
                    NodeStatus::Completed
                },
                output: (index != 2).then(|| NodeOutput {
                    outcome: turn.outcome.clone(),
                    source: None,
                    evaluation: None,
                }),
                error: None,
                ancestors: vec![],
                inputs: vec![],
            })
            .collect::<Vec<_>>();
        let outputs = definition
            .outputs
            .iter()
            .map(|binding| {
                let raw = resolve::select(&binding.reference, NodeStatus::Skipped, None).unwrap();
                ResolvedBinding {
                    name: binding.name.clone(),
                    reference: binding.reference.clone(),
                    value: resolve::apply(&binding.reference, raw).unwrap(),
                    defaulted: true,
                    required_missing: false,
                }
            })
            .collect();
        let report = projection::workflow(
            definition,
            &WorkflowResult {
                request_id: None,
                run: None,
                session: None,
                persisted: false,
                replayed: false,
                sources: vec![],
                frontier: None,
                result: ScheduleResult {
                    status: RunStatus::Completed,
                    nodes,
                    start_order: vec![],
                    completion_order: vec![],
                    outputs,
                },
            },
            None,
        )
        .unwrap();
        ResultEnvelope::new(Report::Run(report))
    }
    #[test]
    fn compiled_caller_defaults_render_as_data_consistently_without_native_answer_labels() {
        let report = defaulted_run(&workflow_with_defaulted_outputs());
        for format in [Format::Text, Format::Json, Format::Jsonl] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            assert_eq!(
                result(&mut stdout, &mut stderr, format, &report).unwrap(),
                0
            );
            assert_eq!(String::from_utf8_lossy(&stderr), "");
            match format {
                Format::Text => {
                    let text = String::from_utf8(stdout).unwrap();
                    assert!(text.contains("fallback (default): {\"unavailable\":true}"));
                    assert!(text.contains("probability (default): 0.2"));
                    assert!(text.contains("score (default): 2.0"));
                    assert!(!text.contains("P(yes)"));
                    assert!(!text.contains("expected score"));
                    assert!(!text.contains("Native confidence"));
                }
                Format::Json => {
                    assert_eq!(
                        serde_json::from_slice::<ResultEnvelope>(&stdout).unwrap(),
                        report
                    );
                }
                Format::Jsonl => {
                    let events = String::from_utf8(stdout)
                        .unwrap()
                        .lines()
                        .map(|line| serde_json::from_str::<EngineEvent>(line).unwrap())
                        .collect::<Vec<_>>();
                    validate_events(&events).unwrap();
                }
                Format::Auto | Format::Toon => unreachable!(),
            }
        }
    }
    #[test]
    fn labelled_values_preserve_historical_json_toon_and_text_bytes() {
        let connection = serde_json::json!({"version":1,"configured":true,"provider":"gateway","model":"user/model"});
        // workflow show has already passed its inspection through codec::to_value.
        let workflow = codec::to_value(&serde_json::json!({"version":1,"header":{"status":"completed","id":"run"},"nodes":[{"node":"check","status":"completed"}]})).unwrap();
        for value in [connection, workflow] {
            for format in [Format::Json, Format::Toon, Format::Text] {
                let mut expected = Vec::new();
                if format == Format::Json {
                    serde_json::to_writer(&mut expected, &value).unwrap();
                } else {
                    if format == Format::Text {
                        expected.extend_from_slice(b"Saved output\n");
                    }
                    expected.extend_from_slice(codec::encode_toon(&value).unwrap().as_bytes());
                }
                expected.push(b'\n');
                let mut actual = Vec::new();
                labelled("Saved output", &value, &mut actual, format).unwrap();
                assert_eq!(actual, expected, "{format:?}");
            }
        }
    }
}
