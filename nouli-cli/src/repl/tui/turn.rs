//! Launching a turn: the form's question or a typed one becomes a draft, the
//! draft gains the attachments and references its text uses, and it runs
//! until it finishes or is cancelled.
use super::{App, Row, Running, Status};
use crate::{evaluate, render, turns::Draft};
use nouli_core::{
    context::ContextUpdate,
    decision::{AnswerSpace, Criterion, TurnRequest},
    history::{ModelSelection, SessionSelection},
    identity::Id,
};
use std::{io, time::Instant};
use tokio_util::sync::CancellationToken;

impl<'a> App<'a> {
    /// Sends the form's question with the queue, or says what to fix and
    /// moves there.
    pub(super) fn send_form(&mut self, ephemeral: bool) -> io::Result<Option<Running<'a>>> {
        self.option_enter = false;
        if !self.prepare_input() {
            return Ok(None);
        }
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
                self.form.focus(problem.section, &mut self.repl.queue.facts);
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
        if !texts.iter().any(|text| text.contains("[decision:")) {
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
        let settings = &self.repl.settings;
        let defaults = ModelSelection {
            provider: settings.config.default_provider.clone(),
            model: settings.config.default_model.clone(),
        };
        nouli_core::config::resolve::select_model(&[
            &draft.common_model,
            &draft.request.model,
            &session,
            &defaults,
        ])
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
                text.extend(levels.iter().map(|level| match level {
                    Criterion::Text(text) => text.as_str(),
                    Criterion::Detailed { description, .. } => description.as_str(),
                }));
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

#[cfg(test)]
mod tests {
    use super::super::{
        Kind,
        tests::{app, draft_from, failed, previous, settings},
    };
    use nouli_core::{decision::AnswerSpace, history::ModelSelection, identity::Id};

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
}
