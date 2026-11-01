//! Literal inline @ discovery across the entire selected local ledger.
use super::{App, Row, Status};
use crate::render;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nouli_core::{
    context::{ContextRecord, ContextUpdate},
    identity::Id,
    outcome::{Answer, QuestionOutcome},
    storage::{
        Ledger,
        decisions::{DecisionSource, SavedDecision},
    },
};
use std::{
    collections::HashSet,
    ops::Range,
    path::{Path, PathBuf},
};

pub(super) const COMPACT: usize = 3;

#[derive(Default)]
pub(super) struct References {
    pub catalog: Vec<SavedDecision>,
    pub root: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    known: HashSet<Id>,
    pub picker: Option<Picker>,
    pub bindings: Vec<Binding>,
    pub inspected: Option<usize>,
    pub preview: Option<Preview>,
    pub preview_scroll: usize,
    pub preview_page: usize,
    dismissed: Option<(Row, usize)>,
}

pub(super) struct Preview {
    pub source: DecisionSource,
    pub lines: Vec<String>,
}

pub(super) struct Picker {
    pub row: Row,
    pub range: Range<usize>,
    pub query: String,
    pub matches: Vec<usize>,
    pub choice: usize,
    pub expanded: bool,
}

pub(super) struct Binding {
    pub record: ContextRecord,
    pub reference: String,
    pub label: String,
}

impl Binding {
    pub fn read(record: &ContextRecord) -> Option<Self> {
        let value = &record.value.value()["nouli_decision_reference"];
        Some(Self {
            record: record.clone(),
            reference: value["reference"].as_str()?.to_owned(),
            label: render::image_labels(&render::clean(value["label"].as_str()?)),
        })
    }
}

impl References {
    pub fn in_directory(root: Option<PathBuf>) -> Self {
        Self {
            root,
            ..Self::default()
        }
    }
    pub fn close(&mut self) {
        if let Some(picker) = self.picker.take() {
            self.dismissed = Some((picker.row, picker.range.start));
        }
        self.inspected = None;
        self.preview = None;
        self.preview_scroll = 0;
    }

    pub fn refresh(&mut self, ledger: &Ledger) -> Result<(), String> {
        let (ids, entries) = ledger
            .saved_decisions(&self.known)
            .map_err(|e| e.to_string())?;
        self.known.extend(ids);
        self.catalog.extend(entries);
        self.catalog.sort_by_key(|e| std::cmp::Reverse(e.sequence));
        let names = ledger
            .list_sessions()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|s| {
                (
                    s.id,
                    s.name
                        .map_or_else(|| s.id.to_string(), |name| name.as_str().to_owned()),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        for entry in &mut self.catalog {
            if let Some(name) = names.get(&entry.source.session) {
                entry.session_name = name.clone();
            }
        }
        Ok(())
    }

    pub fn selected(&self) -> Option<&SavedDecision> {
        let picker = self.picker.as_ref()?;
        if !picker.expanded && picker.choice >= COMPACT {
            return None;
        }
        self.catalog.get(*picker.matches.get(picker.choice)?)
    }

    pub fn file(&self, index: usize) -> Option<&Path> {
        self.files
            .get(index.checked_sub(self.catalog.len())?)
            .map(PathBuf::as_path)
    }

    pub fn selected_file(&self) -> Option<&Path> {
        let picker = self.picker.as_ref()?;
        if !picker.expanded && picker.choice >= COMPACT {
            return None;
        }
        self.file(*picker.matches.get(picker.choice)?)
    }

    pub fn file_label(&self, path: &Path) -> String {
        render::clean(
            &path
                .strip_prefix(self.root.as_deref().unwrap_or(Path::new("")))
                .unwrap_or(path)
                .to_string_lossy(),
        )
    }
}

/// Only a boundary @ under the cursor opens search. Emails, escaped/path
/// fragments, and already-bound labels remain ordinary text/attachments.
fn query(text: &str, cursor: usize) -> Option<(Range<usize>, String)> {
    let before = text.get(..cursor)?;
    let start = before.rfind('@')?;
    if start > 0 && !before[..start].chars().next_back()?.is_whitespace() {
        return None;
    }
    let query = &before[start + 1..];
    if query.contains('\n') {
        return None;
    }
    Some((start..cursor, query.to_owned()))
}

pub(super) fn answer(entry: &SavedDecision) -> String {
    describe_answer(&entry.question, &entry.outcome)
}

pub(super) fn describe_answer(
    question: &nouli_core::decision::Question,
    outcome: &QuestionOutcome,
) -> String {
    match outcome {
        QuestionOutcome::Refused { .. } => "refused · no saved answer".into(),
        QuestionOutcome::Answered { answer, .. } => match answer {
            Answer::Noul { probability } => format!("P(yes)={probability:.2}"),
            Answer::Score { value, .. } => {
                let max = match &question.answer_space {
                    nouli_core::decision::AnswerSpace::Score { levels } => {
                        levels.len().saturating_sub(1)
                    }
                    _ => 0,
                };
                format!("{value:.2} / {max}")
            }
            Answer::Choice {
                selected,
                distribution,
            } => {
                let label = match &question.answer_space {
                    nouli_core::decision::AnswerSpace::Choice { options } => options
                        .iter()
                        .find(|o| &o.id == selected)
                        .map_or(selected.as_str(), |o| o.description.as_str()),
                    _ => selected,
                };
                let probability = distribution
                    .iter()
                    .find(|o| &o.option_id == selected)
                    .map_or(0.0, |o| o.probability);
                format!("{label} · {probability:.2}")
            }
        },
    }
}

impl App<'_> {
    pub(super) fn sync_references(&mut self) {
        if self.graph_focused
            || self.sessions.is_some()
            || self.form.text(&self.repl.queue.facts).starts_with('/')
        {
            self.references.close();
            return;
        }
        let text = self.form.text(&self.repl.queue.facts);
        let Some((range, query)) = query(text, self.form.cursor) else {
            self.references.picker = None;
            self.references.dismissed = None;
            return;
        };
        if self.references.dismissed == Some((self.form.row, range.start)) {
            return;
        }
        // Ignore @ characters belonging to an existing atomic attachment,
        // including an inner @ in its label followed by more ordinary text.
        if self
            .attachment_ranges(text)
            .iter()
            .any(|attachment| attachment.contains(&range.start))
        {
            self.references.picker = None;
            return;
        }
        let old = self.references.picker.take();
        if old.is_none() {
            self.references.files = self
                .references
                .root
                .as_deref()
                .map(super::attachments::discover)
                .unwrap_or_default();
            match self
                .repl
                .ledger()
                .map_err(|e| e.message)
                .and_then(|l| self.references.refresh(&l))
            {
                Ok(()) => {}
                Err(message) => {
                    self.status = Some(Status::error(render::clean(&message)));
                }
            }
        }
        let mut found = self
            .references
            .catalog
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.matches(&query))
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let query_path = query
            .trim()
            .strip_prefix("./")
            .unwrap_or(query.trim())
            .to_lowercase();
        found.extend(
            self.references
                .files
                .iter()
                .enumerate()
                .filter(|(_, path)| {
                    let label = self.references.file_label(path).to_lowercase();
                    query_path
                        .split_whitespace()
                        .all(|word| label.contains(word))
                })
                .map(|(i, _)| self.references.catalog.len() + i),
        );
        let (choice, expanded) = old
            .filter(|p| p.row == self.form.row && p.range.start == range.start)
            .map_or((0, false), |p| {
                (if p.query == query { p.choice } else { 0 }, p.expanded)
            });
        self.references.picker = Some(Picker {
            row: self.form.row,
            range,
            query,
            matches: found,
            choice,
            expanded,
        });
        self.hidden = true;
        self.refresh_reference_preview();
    }

    fn refresh_reference_preview(&mut self) {
        let source = self
            .references
            .picker
            .as_ref()
            .filter(|picker| picker.expanded)
            .and_then(|_| self.references.selected())
            .map(|entry| entry.source.clone());
        let Some(source) = source else {
            self.references.preview = None;
            self.references.preview_scroll = 0;
            return;
        };
        if self
            .references
            .preview
            .as_ref()
            .is_some_and(|preview| preview.source == source)
        {
            return;
        }
        let lines = self
            .repl
            .ledger()
            .map_err(|error| error.message)
            .and_then(|ledger| {
                ledger
                    .decision_state(&source)
                    .map_err(|error| error.to_string())
            })
            .unwrap_or_else(|message| {
                vec![format!(
                    "Cannot read saved state: {}",
                    render::clean(&message)
                )]
            });
        self.references.preview = Some(Preview { source, lines });
        self.references.preview_scroll = 0;
    }

    /// Return true only for keys the picker consumes. Tab/Shift-Tab remain
    /// navigation/type keys, never implicit acceptance.
    pub(super) fn reference_key(&mut self, key: &KeyEvent) -> bool {
        let control = key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        if control && key.code == KeyCode::Char('o') {
            let text = self.form.text(&self.repl.queue.facts);
            let cursor = self.form.cursor;
            self.references.inspected = self
                .references
                .bindings
                .iter()
                .enumerate()
                .flat_map(|(i, b)| {
                    text.match_indices(&b.reference).map(move |(at, _)| {
                        let start = text[..at]
                            .strip_suffix(' ')
                            .and_then(|prefix| prefix.strip_suffix(&b.label))
                            .map_or(at, str::len);
                        let end = at + b.reference.len();
                        let distance = if cursor < start {
                            start - cursor
                        } else {
                            cursor.saturating_sub(end)
                        };
                        (i, distance)
                    })
                })
                .min_by_key(|(_, distance)| *distance)
                .map(|(i, _)| i);
            if self.references.inspected.is_none() {
                self.status = Some(Status::info(
                    "Move to a field with a reference to inspect it.",
                ));
            }
            return true;
        }
        if self.references.inspected.is_some() {
            if key.code == KeyCode::Esc {
                self.references.inspected = None;
                return true;
            }
            self.references.inspected = None;
        }
        let Some(picker) = &mut self.references.picker else {
            return false;
        };
        let limit = if picker.expanded {
            picker.matches.len()
        } else {
            picker.matches.len().min(COMPACT) + 1
        };
        match key.code {
            KeyCode::Up => picker.choice = picker.choice.saturating_sub(1),
            KeyCode::Down => picker.choice = (picker.choice + 1).min(limit.saturating_sub(1)),
            KeyCode::PageUp if picker.expanded => {
                self.references.preview_scroll = self
                    .references
                    .preview_scroll
                    .saturating_sub(self.references.preview_page.max(1));
            }
            KeyCode::PageDown if picker.expanded => {
                self.references.preview_scroll = self
                    .references
                    .preview_scroll
                    .saturating_add(self.references.preview_page.max(1));
            }
            KeyCode::Enter => {
                if !picker.expanded && picker.choice == picker.matches.len().min(COMPACT) {
                    picker.expanded = true;
                    picker.choice = 0;
                } else {
                    self.insert_reference();
                }
            }
            KeyCode::Esc => self.references.close(),
            KeyCode::Tab | KeyCode::BackTab => {
                self.references.close();
                return false;
            }
            KeyCode::Char('s') if control => {
                self.status = Some(Status::info("Choose a result, or Esc to keep the text."));
            }
            KeyCode::F(5) => {
                self.status = Some(Status::info("Choose a result, or Esc to keep the text."));
            }
            _ => return false,
        }
        self.refresh_reference_preview();
        true
    }

    fn insert_reference(&mut self) {
        if let Some(path) = self.references.selected_file().map(Path::to_path_buf) {
            // Load successfully before editing the query. A failed conversion
            // keeps the picker, selection and complete draft available.
            match super::attachments::load(&path) {
                Ok(mut file) => {
                    let label = self.references.file_label(&path);
                    if file.image.is_none() {
                        file.text = format!("File: {label}\n{}", file.text);
                    }
                    let picker = self.references.picker.take().expect("selected file picker");
                    self.form
                        .remove_range(picker.range, &mut self.repl.queue.facts);
                    if self.form.row == Row::Input {
                        self.literal_input = true;
                    }
                    // An imported document occupies one option, even if its
                    // extracted text has many lines. Ordinary list paste still splits.
                    self.form.paste(&file.text, &mut self.repl.queue.facts);
                    if let Some(image) = file.image {
                        self.images.push(image);
                    }
                    self.references.dismissed = None;
                    self.option_enter = false;
                    self.status = Some(Status::info(format!("Added {label} · review, then Send")));
                }
                Err(error) => self.status = Some(Status::error(error.to_string())),
            }
            return;
        }
        let Some(entry) = self.references.selected().cloned() else {
            return;
        };
        if !entry.answered() {
            self.status = Some(Status::error(
                "This question was refused. Choose a saved answer.",
            ));
            return;
        }
        let Some(picker) = self.references.picker.take() else {
            return;
        };
        let existing = self.references.bindings.iter().position(|b| {
            b.record.value.value()["nouli_decision_reference"]["source"]
                == serde_json::to_value(&entry.source).expect("source IDs serialize")
                && b.record.value.value()["nouli_decision_reference"]
                    .get("input_state")
                    .is_some()
        });
        let binding = if let Some(i) = existing {
            i
        } else {
            let record = match self.repl.ledger().map_err(|e| e.message).and_then(|l| {
                l.decision_reference(&entry.source, Id::generate())
                    .map_err(|e| e.to_string())
            }) {
                Ok(record) => record,
                Err(message) => {
                    self.status = Some(Status::error(render::clean(&message)));
                    self.references.picker = Some(picker);
                    return;
                }
            };
            let Some(binding) = Binding::read(&record) else {
                self.status = Some(Status::error("Cannot read this saved reference."));
                return;
            };
            self.references.bindings.push(binding);
            self.references.bindings.len() - 1
        };
        let bound = &self.references.bindings[binding];
        let inserted = format!("{} {}", bound.label, bound.reference);
        self.form
            .remove_range(picker.range, &mut self.repl.queue.facts);
        self.form.paste(&inserted, &mut self.repl.queue.facts);
        self.references.dismissed = None;
        self.option_enter = false;
        self.status = Some(Status::info(
            "Reference added · Ctrl-O inspects · Backspace removes",
        ));
    }

    pub(super) fn reference_updates(
        &self,
        ledger: &Ledger,
        texts: &[&str],
    ) -> Result<(Vec<ContextUpdate>, Vec<Id>), String> {
        for text in texts {
            for tag in render::decision_tags(text) {
                if !self
                    .references
                    .bindings
                    .iter()
                    .any(|b| b.reference == text[tag.clone()])
                {
                    return Err("A saved reference is unavailable. Remove it and choose the answer again with @.".into());
                }
            }
        }
        let records = ledger
            .context_at(
                &ledger
                    .context_sources(self.repl.session.id)
                    .map_err(|e| e.to_string())?,
                &[],
                self.repl.session.id,
                Id::generate(),
            )
            .map_err(|e| e.to_string())?;
        let mut updates = Vec::new();
        let mut required = Vec::new();
        for binding in &self.references.bindings {
            if !texts.iter().any(|text| text.contains(&binding.reference)) {
                continue;
            }
            if let Some(existing) = records.iter().find(|r| {
                r.record.value.value()["nouli_decision_reference"]["reference"].as_str()
                    == Some(&binding.reference)
            }) {
                required.push(existing.record.id);
            } else {
                // The binding can have been carried from a different session.
                // Evidence records are globally unique even across unrelated
                // histories; copy the snapshot under a new destination ID.
                let mut record = binding.record.clone();
                record.id = Id::generate();
                record.key = format!("decision:{}", record.id);
                required.push(record.id);
                updates.push(ContextUpdate::Append { record });
            }
        }
        Ok((updates, required))
    }

    pub(super) fn restore_references(&mut self) {
        self.references.close();
        let used = std::iter::once(self.form.input.as_str())
            .chain(std::iter::once(self.form.question.as_str()))
            .chain(self.form.options.iter().map(String::as_str))
            .chain(self.repl.queue.facts.iter().map(String::as_str))
            .chain(self.repl.queue.question.iter().map(String::as_str))
            .chain(
                self.repl
                    .queue
                    .replaces
                    .iter()
                    .map(|(_, text)| text.as_str()),
            )
            .collect::<Vec<_>>();
        self.references
            .bindings
            .retain(|b| used.iter().any(|text| text.contains(&b.reference)));
        self.references.dismissed = None;
        if let Ok(ledger) = self.repl.ledger()
            && let Ok(sources) = ledger.context_sources(self.repl.session.id)
            && let Ok(records) =
                ledger.context_at(&sources, &[], self.repl.session.id, Id::generate())
        {
            for binding in records.iter().filter_map(|r| Binding::read(&r.record)) {
                if !self
                    .references
                    .bindings
                    .iter()
                    .any(|b| b.reference == binding.reference)
                {
                    self.references.bindings.push(binding);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_a_document_is_one_unsent_option_and_errors_preserve_the_query() {
        let root = std::env::temp_dir().join(format!("nouli-select-file-{}", Id::generate()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("table.csv");
        std::fs::write(&file, "city,fare\nSeattle,42").unwrap();
        let settings = crate::settings::Settings::for_test(root.clone());
        let mut app = super::super::tests::app(&settings);
        app.references.root = Some(root.clone());
        app.form.kind = super::super::form::Kind::Choice;
        app.form.row = Row::Option(0);
        app.form.question = "Which table?".into();
        app.form.options = vec!["@table.csv".into()];
        app.form.cursor = app.form.options[0].len();
        app.sync_references();
        assert_eq!(app.references.selected_file(), Some(file.as_path()));
        std::fs::remove_file(&file).unwrap();
        app.insert_reference();
        assert!(app.references.picker.is_some());
        assert_eq!(app.form.options, vec!["@table.csv"]);
        std::fs::write(&file, "city,fare\nSeattle,42").unwrap();
        app.insert_reference();
        assert!(app.references.picker.is_none());
        assert_eq!(app.form.options.len(), 1);
        assert!(app.form.options[0].contains("Seattle"));
        assert!(app.form.options[0].contains("42"));
        assert!(app.form.options[0].contains("File: table.csv"));
        assert_eq!(app.form.question, "Which table?");
        assert_eq!(app.form.kind, super::super::form::Kind::Choice);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn search_uses_literal_text_and_boundary_at_without_email_or_unicode_rules() {
        assert_eq!(
            query("Reconsider @Where should I go?", 30),
            Some((11..30, "Where should I go?".into()))
        );
        assert!(query("me@example.com", 14).is_none());
        assert_eq!(query("@旅行", 7), Some((0..7, "旅行".into())));
        assert!(query("@old\nnew", 8).is_none());
        let literal = "@Rail [late]";
        assert_eq!(
            query(literal, literal.len()),
            Some((0..literal.len(), "Rail [late]".into()))
        );
    }

    #[test]
    fn at_signs_inside_attachments_never_reopen_search_after_appending_text() {
        use crate::settings::Settings;
        use nouli_core::{context::ContextKind, data::Data, media::Image};
        let settings = Settings::for_test(
            std::env::temp_dir().join(format!("nouli-at-attachments-{}", Id::generate())),
        );
        let mut app = super::super::tests::app(&settings);
        app.form.row = Row::Question;
        for name in ["@photo.png", "my @photo.png"] {
            let reference = format!("[image:{}]", Id::generate());
            app.images = vec![Image {
                reference: reference.clone(),
                name: name.into(),
                format: "png".into(),
                data_base64: String::new(),
            }];
            app.form.question = format!("{name} {reference} more ordinary text");
            app.form.cursor = app.form.question.len();
            app.sync_references();
            assert!(app.references.picker.is_none());
            assert!(app.status.is_none());
        }
        let id = Id::generate();
        let reference = format!("[decision:{id}]");
        let label = "@Question about @old · source / 1";
        app.references.bindings = vec![Binding {
            record: ContextRecord {
                id,
                key: format!("decision:{id}"),
                kind: ContextKind::Evidence,
                value: Data::new(serde_json::json!({})).unwrap(),
            },
            reference: reference.clone(),
            label: label.into(),
        }];
        app.form.question = format!("{label} {reference} more ordinary text");
        app.form.cursor = app.form.question.len();
        app.sync_references();
        assert!(app.references.picker.is_none());
        assert!(app.status.is_none());
    }

    /// A ledger with one finalized turn saved in `source` and an empty
    /// `target` session; the source's guard stays held for the whole test.
    struct SavedElsewhere {
        ledger: Ledger,
        source: Id,
        target: Id,
        saved: Id,
        guard: nouli_core::storage::guards::SessionGuard,
    }

    fn saved_elsewhere(settings: &crate::settings::Settings) -> SavedElsewhere {
        use nouli_core::{
            history::{FinalizedTurn, ModelSelection, StandingChanges},
            storage::{OpenMode, turns::AttemptInput},
        };
        let mut ledger = Ledger::open(&settings.store, OpenMode::ReadWriteCreate).unwrap();
        let source = ledger
            .create_session(
                Some("other-project".to_owned().try_into().unwrap()),
                ModelSelection::default(),
            )
            .unwrap();
        let target = ledger
            .create_session(
                Some("current-project".to_owned().try_into().unwrap()),
                ModelSelection::default(),
            )
            .unwrap();
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/contracts/history.json"))
                .unwrap();
        let template: FinalizedTurn = serde_json::from_value(fixture["turns"][0].clone()).unwrap();
        let guard = ledger.session_guard(source.id).unwrap();
        let intent = ledger
            .prepare_attempt(
                &guard,
                AttemptInput {
                    request: template.request,
                    updates: vec![],
                    standing: StandingChanges::default(),
                    manifest: template.manifest,
                    request_id: None,
                },
            )
            .unwrap();
        let saved = ledger
            .finalize_attempt(&guard, intent.id, template.outcome)
            .unwrap();
        SavedElsewhere {
            ledger,
            source: source.id,
            target: target.id,
            saved: saved.id,
            guard,
        }
    }

    /// Sends one key, asserting that it never launches a decision turn.
    fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
        let mut running = None;
        app.key(KeyEvent::new(code, modifiers), &mut running)
            .unwrap();
        assert!(
            running.is_none(),
            "selecting a reference never sends a decision"
        );
    }

    #[test]
    fn store_wide_selection_is_atomic_preserves_drafts_and_requires_bound_evidence() {
        let settings = crate::settings::Settings::for_test(
            std::env::temp_dir().join(format!("nouli-reference-ui-{}", Id::generate())),
        );
        let SavedElsewhere {
            mut ledger,
            source,
            target,
            saved,
            guard,
        } = saved_elsewhere(&settings);
        let mut app = super::super::tests::app(&settings);
        app.repl.session.id = target;
        app.form.row = Row::Question;
        app.form.kind = super::super::Kind::Choice;
        app.form.options = vec!["Keep".into(), "Change".into()];
        for ch in "Reconsider @Which route".chars() {
            press(&mut app, KeyCode::Char(ch), KeyModifiers::NONE);
        }
        assert_eq!(app.references.picker.as_ref().unwrap().matches.len(), 1);
        assert_eq!(app.references.selected().unwrap().source.session, source);
        ledger
            .rename_session(source, "renamed-source".to_owned().try_into().unwrap())
            .unwrap();
        app.references.refresh(&ledger).unwrap();
        assert!(
            app.references
                .catalog
                .iter()
                .all(|entry| entry.session_name == "renamed-source")
        );
        assert!(
            app.references
                .catalog
                .iter()
                .all(|entry| entry.matches("renamed-source"))
        );
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.references.picker.as_ref().unwrap().expanded);
        press(&mut app, KeyCode::Char('?'), KeyModifiers::NONE);
        assert!(app.references.picker.as_ref().unwrap().expanded);
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert!(
            app.references.picker.as_ref().unwrap().expanded,
            "editing a search keeps Browse history open"
        );
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.references.picker.is_none());
        assert_eq!(app.references.bindings.len(), 1);
        assert_eq!(app.form.kind, super::super::Kind::Choice);
        assert_eq!(app.form.options, vec!["Keep", "Change"]);
        let text = app.form.question.clone();
        let (updates, required) = app.reference_updates(&ledger, &[&text]).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(required.len(), 1);
        let record = updates[0].new_record().unwrap();
        assert_eq!(
            record.value.value()["nouli_decision_reference"]["source"]["turn"],
            saved.to_string()
        );
        assert_ne!(record.id, app.references.bindings[0].record.id);
        let range = render::decision_tags(&text).pop().unwrap();
        assert!(!render::image_labels(&text).contains("[decision:"));
        // Session switching must preserve a still-unsent reference binding.
        app.repl.session.id = source;
        app.restore_references();
        assert_eq!(app.references.bindings.len(), 1);
        assert!(app.reference_updates(&ledger, &[&text]).is_ok());
        app.form.cursor = range.end;
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(app.form.cursor, "Reconsider ".len());
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(app.form.cursor, range.end);
        app.form.paste(" ", &mut app.repl.queue.facts);
        press(&mut app, KeyCode::Char('w'), KeyModifiers::CONTROL);
        assert_eq!(app.form.question, "Reconsider ");
        assert_eq!(
            app.reference_updates(&ledger, &[&app.form.question])
                .unwrap()
                .0,
            [] as [nouli_core::context::ContextUpdate; 0]
        );
        let orphan = format!("Unknown [decision:{}]", Id::generate());
        assert!(app.reference_updates(&ledger, &[&orphan]).is_err());
        drop(guard);
        drop(ledger);
        drop(app);
        std::fs::remove_dir_all(&settings.store).unwrap();
    }
}
