//! Launching a turn: the form's question or a typed one becomes a draft, the
//! draft gains the attachments and references its text uses, and it runs
//! until it finishes or is cancelled.
use super::{App, Kind, Row, Running, Status};
use crate::{evaluate, render, turns::Draft};
use nouli_core::{
    context::{ContextUpdate, evidence::marker},
    decision::{AnswerSpace, Criterion, TurnRequest},
    history::{ModelSelection, SessionSelection},
    identity::Id,
};
use nouli_core::{
    decision::Question,
    protocol::{ErrorCategory, ErrorReport, Report, ResultEnvelope},
};
use std::{io, time::Instant};
use tokio_util::sync::CancellationToken;

impl<'a> App<'a> {
    /// Sends the form's question with the queue, or says what to fix and
    /// moves there.
    pub(super) fn send_form(&mut self, ephemeral: bool) -> io::Result<Option<Running<'a>>> {
        if !self.prepare_input() {
            return Ok(None);
        }
        self.settle_context();
        let inherited = self.inherited_answers().cloned();
        let prompt = if self.form.question.trim().is_empty() {
            self.prior_for_kind()
                .map_or(self.form.question.as_str(), |previous| {
                    previous.question.prompt.as_str()
                })
        } else {
            self.form.question.as_str()
        };
        let question = if let Some(previous) = &inherited {
            let mut question = previous.question.clone();
            prompt.trim().clone_into(&mut question.prompt);
            Ok(question)
        } else if prompt == self.form.question {
            self.form.build_question()
        } else {
            self.form.build_question_with_prompt(prompt)
        };
        match question {
            Ok(question) => {
                // Include context still being edited when sending.
                if !self.form.input.trim().is_empty() {
                    self.repl
                        .queue
                        .facts
                        .push(std::mem::take(&mut self.form.input));
                    if self.form.row == Row::Input {
                        self.form.cursor = 0;
                    }
                }
                self.literal_input = false;
                if !ephemeral {
                    self.repl.queue.question = None;
                }
                self.echo_turn(&question.prompt, ephemeral);
                let mut draft = self.repl.draft_question(question, ephemeral)?;
                if let (Some(previous), Some(draft)) = (&inherited, &mut draft) {
                    let created = new_record_ids(&draft.request.updates);
                    require(
                        &mut draft.request.required_records,
                        std::iter::once(previous.turn).chain(created),
                    );
                }
                Ok(self.launch_draft(draft, ephemeral, true))
            }
            Err(problem) => {
                self.status = Some(Status::error(problem.message));
                self.form.focus(problem.section);
                Ok(None)
            }
        }
    }

    pub(super) fn launch(
        &mut self,
        question: &str,
        ephemeral: bool,
        from_form: bool,
    ) -> io::Result<Option<Running<'a>>> {
        let draft = self.repl.draft(question, ephemeral)?;
        Ok(self.launch_draft(draft, ephemeral, from_form))
    }

    /// Completes the draft with the saved answers and images its text refers
    /// to, requires those records and the latest question's turn, and starts
    /// it. A saved reference that cannot be resolved stops it with a status.
    pub(super) fn launch_draft(
        &mut self,
        draft: Option<Draft>,
        ephemeral: bool,
        from_form: bool,
    ) -> Option<Running<'a>> {
        self.drain();
        let mut draft = draft?;
        let texts = self
            .repl
            .queue
            .facts
            .iter()
            .map(String::as_str)
            .chain(question_texts(&draft.request.turn))
            .collect::<Vec<_>>();
        match self.saved_reference_updates(&texts) {
            Ok((updates, required)) => {
                draft.request.updates.extend(updates);
                require(&mut draft.request.required_records, required);
            }
            Err(message) => {
                self.status = Some(Status::error(render::clean(&message)));
                return None;
            }
        }
        for update in self.image_updates(&texts) {
            draft
                .request
                .required_records
                .push(update.new_record().expect("image record").id);
            draft.request.updates.push(update);
        }
        let created = new_record_ids(&draft.request.updates);
        require(
            &mut draft.request.required_records,
            self.latest_question
                .iter()
                .map(|latest| latest.turn)
                .chain(created),
        );
        let model = self.turn_model(&draft);
        self.status = None;
        Some(self.start(draft, ephemeral, from_form, model))
    }

    /// The evidence for every `[decision:…]` reference in `texts`, and the
    /// records it requires.
    fn saved_reference_updates(
        &self,
        texts: &[&str],
    ) -> Result<(Vec<ContextUpdate>, Vec<Id>), String> {
        if !texts
            .iter()
            .any(|text| text.contains(marker::DECISION_MARKER_PREFIX))
        {
            return Ok((Vec::new(), Vec::new()));
        }
        self.repl
            .ledger()
            .map_err(|e| e.message)
            .and_then(|ledger| self.reference_updates(&ledger, texts))
    }

    /// A record for each attached image that `texts` mention.
    fn image_updates(&self, texts: &[&str]) -> Vec<ContextUpdate> {
        self.images
            .iter()
            .filter(|image| texts.iter().any(|text| text.contains(&image.reference)))
            .map(super::attachments::update)
            .collect()
    }

    /// The model the turn will use: the draft's, then the session's, then
    /// the configured default.
    fn turn_model(&self, draft: &Draft) -> ModelSelection {
        let session = if draft.ephemeral {
            ModelSelection::default()
        } else {
            match &draft.request.session {
                SessionSelection::Continue { session } => self
                    .repl
                    .ledger()
                    .and_then(|ledger| ledger.session(session).map_err(|error| error.report()))
                    .map(|session| session.defaults)
                    .unwrap_or_default(),
                _ => ModelSelection::default(),
            }
        };
        crate::selection::model(
            Some(&self.repl.settings.config),
            &[&draft.common_model, &draft.request.model, &session],
        )
    }

    /// The turn as a future the loop polls, cancelled through its token.
    fn start(
        &self,
        draft: Draft,
        ephemeral: bool,
        from_form: bool,
        model: ModelSelection,
    ) -> Running<'a> {
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let settings = self.repl.settings.clone().into_owned();
        Running {
            turn: Box::pin(async move {
                let interrupt = async move {
                    stop.cancelled().await;
                    Ok::<(), io::Error>(())
                };
                let envelope = evaluate::run_until(&draft, &settings, interrupt).await;
                (envelope, draft)
            }),
            cancel,
            ephemeral,
            from_form,
            started: Instant::now(),
            model,
        }
    }
}

/// Every question's prompt and its options' or levels' descriptions.
fn question_texts(turn: &TurnRequest) -> impl Iterator<Item = &str> {
    turn.questions.iter().flat_map(|question| {
        let mut text = vec![question.prompt.as_str()];
        match &question.answer_space {
            AnswerSpace::Choice { options } => {
                text.extend(options.iter().map(|option| option.description.as_str()));
            }
            AnswerSpace::Score { levels } => {
                text.extend(levels.iter().map(Criterion::description));
            }
            AnswerSpace::Noul => {}
        }
        text
    })
}

/// The ids of the records `updates` create.
fn new_record_ids(updates: &[ContextUpdate]) -> Vec<Id> {
    updates
        .iter()
        .filter_map(|update| update.new_record().map(|record| record.id))
        .collect()
}

/// Requires each of `ids` once, keeping the order they first appear.
fn require(required: &mut Vec<Id>, ids: impl IntoIterator<Item = Id>) {
    for id in ids {
        if !required.contains(&id) {
            required.push(id);
        }
    }
}

#[derive(Clone)]
pub(super) struct PreviousQuestion {
    pub(super) turn: Id,
    pub(super) question: Question,
}

impl PreviousQuestion {
    pub(super) fn kind(&self) -> Kind {
        Kind::from(&self.question.answer_space)
    }
}

/// Whether the same request could succeed if sent again. This goes by the
/// failure's kind because `retryable` is only kept on saved attempts.
fn resending_can_help(error: &ErrorReport) -> bool {
    matches!(error.category, ErrorCategory::Transport) || error.code == "provider_capacity"
}

impl App<'_> {
    /// Shows a finished turn as the line loop would. A saved turn has taken
    /// the queue, and its question when it came from the form.
    pub(super) fn finish(
        &mut self,
        envelope: ResultEnvelope,
        updates: &[ContextUpdate],
        ephemeral: bool,
        from_form: bool,
    ) -> io::Result<()> {
        let saved = !ephemeral && matches!(envelope.result, Report::Turn(_));
        // Nothing said while the turn ran still applies.
        self.status = None;
        let next_latest = match &envelope.result {
            Report::Turn(report) if report.session == Some(self.repl.session.id) => report
                .turn
                .zip(report.questions.last())
                .map(|(turn, question)| PreviousQuestion {
                    turn,
                    question: question.clone(),
                }),
            _ => None,
        };
        let next_previous = match &envelope.result {
            Report::Turn(report) if report.session == Some(self.repl.session.id) => {
                match (report.turn, report.questions.as_slice()) {
                    (Some(turn), [question])
                        if !matches!(question.answer_space, AnswerSpace::Noul) =>
                    {
                        Some(PreviousQuestion {
                            turn,
                            question: question.clone(),
                        })
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        if let Report::Turn(report) = &envelope.result {
            let provider = &report.outcome.provider;
            self.model = Some(format!(
                "{} · {}",
                render::clean(&provider.model),
                render::clean(&provider.provider)
            ));
        }
        if let Report::Error(error) = &envelope.result {
            self.status = Some(if error.code == "evaluation_canceled" {
                Status::info("Cancelled · your input is kept")
            } else {
                let message = render::clean(&error.message);
                if resending_can_help(error) {
                    Status::failed(message)
                } else {
                    Status::error(message)
                }
            });
        }
        self.repl.finish_display(
            envelope.result,
            updates,
            ephemeral,
            Some(self.transcript_width),
        )?;
        self.drain();
        if saved {
            self.previous_question = next_previous;
            self.latest_question = next_latest;
            self.form.sent(from_form);
            self.images.clear();
            self.restore_references();
            self.refresh_graph();
        }
        Ok(())
    }

    pub(super) fn prior_for_kind(&self) -> Option<&PreviousQuestion> {
        self.previous_question
            .as_ref()
            .filter(|previous| previous.kind() == self.form.kind && self.form.kind != Kind::Noul)
    }

    pub(super) fn inherited_answers(&self) -> Option<&PreviousQuestion> {
        if self.form.labels().next().is_some() {
            None
        } else {
            self.prior_for_kind()
        }
    }

    pub(super) fn refresh_previous_question(&mut self) {
        self.restore_references();
        self.previous_question = None;
        self.latest_question = None;
        let Some(export) = self
            .repl
            .ledger()
            .ok()
            .and_then(|ledger| ledger.export_session(&self.repl.selector()).ok())
        else {
            return;
        };
        self.latest_question = export.turns.iter().rev().find_map(|turn| {
            turn.request
                .questions
                .last()
                .map(|question| PreviousQuestion {
                    turn: turn.id,
                    question: question.clone(),
                })
        });
        let [head] = export.session.frontier.as_slice() else {
            return;
        };
        let Some(turn) = export
            .turns
            .iter()
            .find(|turn| turn.id == *head && turn.session == self.repl.session.id)
        else {
            return;
        };
        let [question] = turn.request.questions.as_slice() else {
            return;
        };
        let previous = PreviousQuestion {
            turn: turn.id,
            question: question.clone(),
        };
        if self.form.is_empty() {
            self.form.kind = previous.kind();
        }
        if previous.kind() != Kind::Noul {
            self.previous_question = Some(previous);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        Kind,
        tests::{app, draft_from, failed, previous, settings},
    };
    use crate::repl::tui::{tests::*, *};
    use nouli_core::{decision::AnswerSpace, history::ModelSelection, identity::Id};

    #[test]
    fn only_failures_a_resend_could_fix_offer_a_retry() {
        use nouli_core::protocol::{ErrorCategory as Category, ErrorReport, Report};
        for (category, code, retry) in [
            (Category::Configuration, "providers_required", false),
            (
                Category::Authentication,
                "provider_authentication_failed",
                false,
            ),
            (Category::Capacity, "context_capacity", false),
            (Category::Transport, "provider_transport_failed", true),
            (Category::Capacity, "provider_capacity", true),
        ] {
            let settings = settings();
            let mut app = app(&settings);
            // The final report never carries `retryable`.
            let report = Report::Error(ErrorReport {
                category,
                code: code.into(),
                message: "It failed".into(),
                retryable: false,
            });
            app.finish(ResultEnvelope::new(report), &[], false, true)
                .unwrap();
            assert_eq!(app.status.as_ref().unwrap().retry, retry, "{code}");
        }
    }

    #[test]
    fn a_finished_turn_clears_what_was_said_while_it_ran() {
        let settings = settings();
        let mut app = app(&settings);
        app.status = Some(Status::info("a turn is running · Ctrl-C cancels it"));
        app.finish(
            ResultEnvelope::new(crate::repl::tests::saved_turn()),
            &[],
            false,
            true,
        )
        .unwrap();
        assert!(app.status.is_none());
    }

    #[test]
    fn empty_lists_reuse_exact_prior_answers_and_require_new_evidence() {
        for kind in [Kind::Choice, Kind::Score] {
            let settings = settings();
            let mut app = app(&settings);
            let previous = previous(kind);
            app.form.kind = kind;
            app.previous_question = Some(previous.clone());
            assert!(app.form.is_empty(), "inherited values are not unsent input");
            app.form.options.push("  ".into());
            app.form.input = "I now have a smaller budget.".into();
            let draft = draft_from(app.send_form(false).unwrap().unwrap());
            assert_eq!(
                draft.request.turn.questions.as_slice(),
                std::slice::from_ref(&previous.question)
            );
            assert!(draft.request.required_records.contains(&previous.turn));
            assert_eq!(draft.request.updates.len(), 1);
            let evidence = draft.request.updates[0].new_record().unwrap();
            assert!(draft.request.required_records.contains(&evidence.id));
            assert_eq!(evidence.value.value(), "I now have a smaller budget.");
            // Updating the wording still compares with the preceding response.
            app.form.question = "Which destination is affordable now?".into();
            let draft = draft_from(app.send_form(false).unwrap().unwrap());
            let question = &draft.request.turn.questions[0];
            assert_eq!(question.id, previous.question.id);
            assert_eq!(question.prompt, app.form.question);
            assert_eq!(question.answer_space, previous.question.answer_space);
            assert_eq!(question.criteria, previous.question.criteria);
        }
    }

    #[test]
    fn explicit_lists_replace_inheritance_and_yes_no_does_not_inherit() {
        let settings = settings();
        let mut app = app(&settings);
        let previous = previous(Kind::Choice);
        app.form.kind = Kind::Choice;
        app.previous_question = Some(previous.clone());
        app.form.options = vec!["One new option".into()];
        assert!(app.send_form(false).unwrap().is_none());
        assert!(app.status.as_ref().unwrap().text.contains("at least two"));
        assert!(app.inherited_answers().is_none());
        app.form.options.push("Another new option".into());
        let draft = draft_from(app.send_form(false).unwrap().unwrap());
        let question = &draft.request.turn.questions[0];
        assert_ne!(question.id, previous.question.id);
        assert_eq!(question.prompt, previous.question.prompt);
        assert_eq!(
            question.criteria,
            [] as [nouli_core::decision::Criterion; 0]
        );
        assert!(!draft.request.required_records.contains(&previous.turn));
        app.form.kind = Kind::Score;
        assert!(app.prior_for_kind().is_none());
        app.form.kind = Kind::Noul;
        app.form.question.clear();
        assert!(app.prior_for_kind().is_none());
        assert!(app.send_form(false).unwrap().is_none());
        app.form.question = "Will it be sunny?".into();
        let draft = draft_from(app.send_form(false).unwrap().unwrap());
        assert_eq!(
            draft.request.turn.questions[0].answer_space,
            AnswerSpace::Noul
        );
    }

    #[test]
    fn the_latest_question_and_new_evidence_stay_in_context_for_a_fresh_yes_no() {
        let settings = settings();
        let mut app = app(&settings);
        let latest = previous(Kind::Noul);
        app.latest_question = Some(latest.clone());
        app.form.question = "Are you sure about that?".into();
        app.form.input = "Here is new information.".into();
        assert!(
            app.prior_for_kind().is_none(),
            "Yes/No still uses a fresh prompt"
        );
        let draft = draft_from(app.send_form(false).unwrap().unwrap());
        assert_eq!(draft.request.turn.questions[0].prompt, app.form.question);
        assert!(draft.request.required_records.contains(&latest.turn));
        let evidence = draft.request.updates[0].new_record().unwrap();
        assert!(draft.request.required_records.contains(&evidence.id));
        app.finish(failed(), &[], false, true).unwrap();
        assert_eq!(app.latest_question.as_ref().unwrap().turn, latest.turn);
        app.repl.session.id = Id::generate();
        app.refresh_previous_question();
        assert!(
            app.latest_question.is_none(),
            "independent sessions start without that context"
        );
    }

    #[test]
    fn running_model_uses_the_draft_mode_instead_of_a_stale_ready_selection() {
        let mut settings = settings();
        settings.config.default_provider = Some("configured".into());
        settings.config.default_model = Some("configured-model".into());
        let mut app = app(&settings);
        app.selected_model = ModelSelection {
            provider: Some("session".into()),
            model: Some("session-model".into()),
        };
        app.form.question = "Is this a what-if?".into();
        let mut draft = app
            .repl
            .draft_question(app.form.build_question().unwrap(), false)
            .unwrap()
            .unwrap();
        draft.ephemeral = true;
        draft.request.session = nouli_core::history::SessionSelection::From {
            sources: vec![nouli_core::history::SourceSelection {
                session: app.repl.selector(),
                frontier: None,
            }],
            name: None,
        };
        let running = app.launch_draft(Some(draft), true, true).unwrap();
        assert_eq!(running.model.provider.as_deref(), Some("configured"));
        assert_eq!(running.model.model.as_deref(), Some("configured-model"));
    }

    #[test]
    fn only_saved_answers_replace_the_previous_question() {
        let settings = settings();
        let mut app = app(&settings);
        let previous = previous(Kind::Score);
        app.previous_question = Some(previous.clone());
        app.form.kind = Kind::Score;
        app.form.question = "A new judgment?".into();
        app.finish(failed(), &[], false, true).unwrap();
        assert_eq!(app.previous_question.as_ref().unwrap().turn, previous.turn);
        assert_eq!(app.form.question, "A new judgment?");
        let Report::Turn(mut report) = crate::repl::tests::saved_turn() else {
            unreachable!()
        };
        report.session = Some(app.repl.session.id);
        report.questions = vec![previous.question.clone()];
        app.finish(
            ResultEnvelope::new(Report::Turn(report.clone())),
            &[],
            true,
            true,
        )
        .unwrap();
        assert_eq!(app.previous_question.as_ref().unwrap().turn, previous.turn);
        assert_eq!(app.form.question, "A new judgment?");
        app.finish(
            ResultEnvelope::new(Report::Turn(report.clone())),
            &[],
            false,
            true,
        )
        .unwrap();
        assert_eq!(
            app.previous_question.as_ref().unwrap().turn,
            report.turn.unwrap()
        );
        assert_eq!(app.form.kind, Kind::Score);
        assert!(app.form.is_empty());
        report.questions[0].answer_space = AnswerSpace::Noul;
        app.finish(ResultEnvelope::new(Report::Turn(report)), &[], false, true)
            .unwrap();
        assert!(app.previous_question.is_none());
        app.previous_question = Some(previous);
        // Missing/empty sessions cannot retain another session's definition.
        app.repl.session.id = Id::generate();
        app.refresh_previous_question();
        assert!(app.previous_question.is_none());
    }

    #[test]
    fn a_saved_turn_empties_the_form_and_a_failed_one_keeps_it() {
        let settings = settings();
        let mut app = app(&settings);
        let fill = |app: &mut App| {
            app.repl.queue.facts.push("a fact".into());
            app.form.question = "q: Is it?".into();
            app.form.row = Row::Question;
        };
        fill(&mut app);
        app.finish(failed(), &[], false, true).unwrap();
        assert_eq!(app.form.question, "q: Is it?");
        assert_eq!(app.repl.queue.facts.len(), 1);
        assert!(transcript(&app).contains("provider_transport"));
        let saved = ResultEnvelope::new(crate::repl::tests::saved_turn());
        app.finish(saved, &[], false, true).unwrap();
        assert_eq!(app.form.question, "");
        assert!(app.repl.queue.is_empty());
        assert_eq!(app.form.row, Row::Input);
        assert!(app.model.is_some());
    }

    #[test]
    fn a_question_sent_from_the_form_replaces_one_a_failed_turn_left() {
        let settings = settings();
        let mut app = app(&settings);
        app.repl.queue.question = Some("a: Is the first one still waiting?".into());
        app.form.question = "b: Is this the one to send again?".into();
        app.form.row = Row::Question;
        assert!(app.send_form(false).unwrap().is_some());
        assert_eq!(app.repl.queue.question, None);
        app.finish(failed(), &[], false, true).unwrap();
        assert_eq!(app.form.question, "b: Is this the one to send again?");
        // Ctrl-S from Context resends the current form after a failure.
        press(&mut app, KeyCode::Up, KeyModifiers::NONE);
        let mut running = None;
        app.key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut running,
        )
        .unwrap();
        let draft = draft_from(running.unwrap());
        assert_eq!(draft.request.turn.questions[0].prompt, app.form.question);
    }

    #[test]
    fn ctrl_c_ends_a_turn_whose_provider_never_answers() {
        // A provider that takes the connection and never answers.
        let provider = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut settings = settings();
        settings.config = nouli_core::config::Config::parse(&format!(
            "default_provider = 'local'\ndefault_model = 'offline'\n\
             [providers.local]\nprotocol = 'system-one'\nbase_url = 'http://{}'\n\
             [providers.local.models.offline]\ncontext_budget_tokens = 100000\n\
             context_window_tokens = 100000\nreserved_output_tokens = 128\n",
            provider.local_addr().unwrap()
        ))
        .unwrap();
        let creation = crate::turns::Creation {
            name: "s".to_owned().try_into().unwrap(),
            sources: vec![],
            defaults: ModelSelection::default(),
        };
        let session = crate::repl::sessions::create(&settings, &creation)
            .unwrap()
            .id;
        let mut app = app(&settings);
        app.repl.session.id = session;
        app.form.question = "late: Is the train late?".into();
        app.form.row = Row::Question;
        let mut running = None;
        let key = |code, modifiers| KeyEvent::new(code, modifiers);
        app.key(key(KeyCode::Enter, KeyModifiers::NONE), &mut running)
            .unwrap();
        app.key(key(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut running)
            .unwrap();
        let turn = running.expect("Enter on the question sends it").turn;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ended =
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(30), turn).await });
        let (envelope, _) = ended.expect("Ctrl-C ends the turn before the provider answers");
        let Report::Error(error) = envelope.result else {
            panic!("a cancelled turn saves nothing");
        };
        assert_eq!(error.code, "evaluation_canceled");
        let _ = std::fs::remove_dir_all(&settings.store);
    }
}
