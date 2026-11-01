//! Literal paste, attachment insertion, and streamed file drops.
use super::{
    App, Status, attachments,
    form::{self, Kind, Row},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nouli_core::context::evidence::marker::MarkerKind;

impl App<'_> {
    pub(super) fn paste_text(&mut self, text: &str) {
        self.option_enter = false;
        if let Row::Option(mut index) = self.form.row {
            let mut lines = text.lines().filter(|line| !line.trim().is_empty());
            if let Some(first) = lines.next() {
                self.form.paste(first, &mut self.repl.queue.facts);
                for line in lines {
                    self.form.enter_option(index);
                    index += 1;
                    self.form.paste(line, &mut self.repl.queue.facts);
                }
            }
        } else {
            self.form.paste(text, &mut self.repl.queue.facts);
        }
        self.status = None;
    }

    /// Shared attachment spans for editing and @-anchor detection.
    pub(super) fn attachment_ranges(&self, text: &str) -> Vec<std::ops::Range<usize>> {
        let mut ranges: Vec<_> = MarkerKind::Image
            .ranges(text)
            .into_iter()
            .map(|tag| {
                let start = self
                    .images
                    .iter()
                    .find(|image| image.reference == text[tag.clone()])
                    .and_then(|image| {
                        text[..tag.start]
                            .strip_suffix(' ')
                            .and_then(|prefix| prefix.strip_suffix(&image.name))
                    })
                    .map_or(tag.start, str::len);
                start..tag.end
            })
            .collect();
        ranges.extend(MarkerKind::Decision.ranges(text).into_iter().map(|tag| {
            let start = self
                .references
                .bindings
                .iter()
                .find(|b| b.reference() == &text[tag.clone()])
                .and_then(|b| {
                    text[..tag.start]
                        .strip_suffix(' ')
                        .and_then(|prefix| prefix.strip_suffix(&b.label))
                })
                .map_or(tag.start, str::len);
            start..tag.end
        }));
        ranges
    }

    /// Imported files and saved references are edited as complete attachments.
    pub(super) fn edit_form(&mut self, key: &KeyEvent) -> bool {
        let text = self.form.text(&self.repl.queue.facts);
        let ranges = self.attachment_ranges(text);
        let cursor = self.form.cursor;
        let control = key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT);
        let backwards = key.code == KeyCode::Backspace
            || control && matches!(key.code, KeyCode::Char('h' | 'w'));
        let word_backwards = key.code == KeyCode::Backspace
            && key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || control && key.code == KeyCode::Char('w');
        if word_backwards {
            let start = form::word_start(text, cursor);
            let touched: Vec<_> = ranges
                .iter()
                .filter(|range| start < range.end && cursor > range.start)
                .collect();
            if !touched.is_empty() {
                let start = touched
                    .iter()
                    .fold(start, |start, range| start.min(range.start));
                let end = touched.iter().fold(cursor, |end, range| end.max(range.end));
                self.form
                    .remove_range(start..end, &mut self.repl.queue.facts);
                return true;
            }
        }
        if let Some(range) = ranges.iter().find(|range| {
            backwards && cursor == range.end || key.code == KeyCode::Delete && cursor == range.start
        }) {
            self.form
                .remove_range(range.clone(), &mut self.repl.queue.facts);
            return true;
        }
        if key.code == KeyCode::Left
            && let Some(range) = ranges
                .iter()
                .find(|range| range.start < cursor && cursor <= range.end)
        {
            self.form.cursor = range.start;
            return true;
        }
        if key.code == KeyCode::Right
            && let Some(range) = ranges
                .iter()
                .find(|range| range.start <= cursor && cursor < range.end)
        {
            self.form.cursor = range.end;
            return true;
        }
        let edited = self.form.edit(key, &mut self.repl.queue.facts);
        if matches!(key.code, KeyCode::Left | KeyCode::Right)
            && let Some(range) = ranges
                .iter()
                .find(|range| range.start < self.form.cursor && self.form.cursor < range.end)
        {
            self.form.cursor = if key.code == KeyCode::Left {
                range.start
            } else {
                range.end
            };
        }
        edited
    }

    pub(super) fn attach(&mut self, paths: &[std::path::PathBuf]) {
        let loaded = paths
            .iter()
            .map(|path| attachments::load(path))
            .collect::<Result<Vec<_>, _>>();
        match loaded {
            Ok(loaded) => self.insert_files(loaded),
            Err(error) => self.status = Some(Status::error(error.to_string())),
        }
    }

    pub(super) fn insert_files(&mut self, loaded: Vec<attachments::Loaded>) {
        let count = loaded.len();
        if self.form.row == Row::Input {
            self.literal_input = true;
        }
        for (index, file) in loaded.into_iter().enumerate() {
            if index > 0 {
                if matches!(self.form.row, Row::Option(_)) {
                    self.form.add_option(&mut self.repl.queue.facts);
                } else {
                    self.form.paste(" ", &mut self.repl.queue.facts);
                }
            }
            self.option_enter = false;
            self.form.paste(&file.text, &mut self.repl.queue.facts);
            if let Some(image) = file.image {
                self.images.push(image);
            }
        }
        self.status = Some(Status::info(format!(
            "Inserted {count} file(s) here · review, then Send"
        )));
    }

    pub(super) fn prepare_input(&mut self) -> bool {
        if !self.consume_drop() {
            return false;
        }
        if let Some((row, text)) = &self.failed_drop {
            if self.form.row_text(*row, &self.repl.queue.facts) == text
                && !(matches!(row, Row::Option(_)) && self.form.kind == Kind::Noul)
            {
                self.status = Some(Status::error(
                    "Edit or remove the file path that could not be imported.",
                ));
                return false;
            }
            self.failed_drop = None;
        }
        true
    }

    pub(super) fn consume_drop(&mut self) -> bool {
        let Some((row, start, _)) = self.drop_start.take() else {
            return true;
        };
        if row != self.form.row {
            return true;
        }
        let end = self.form.cursor;
        let text = self.form.text(&self.repl.queue.facts);
        let Some(candidate) = text.get(start..end) else {
            return true;
        };
        let Some(paths) = attachments::paths(candidate) else {
            return true;
        };
        match paths
            .iter()
            .map(|path| attachments::load(path))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(loaded) => {
                self.form
                    .remove_range(start..end, &mut self.repl.queue.facts);
                self.insert_files(loaded);
            }
            Err(error) => {
                self.status = Some(Status::error(error.to_string()));
                self.failed_drop = Some((
                    self.form.row,
                    self.form.text(&self.repl.queue.facts).to_owned(),
                ));
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repl::tui::form::Section;
    use crate::repl::tui::{tests::*, *};

    #[test]
    fn image_labels_edit_as_attachments_without_exposing_reference_characters() {
        let settings = settings();
        let mut app = app(&settings);
        let name = "photo \"中文\".png";
        let reference = MarkerKind::Image.marker(Id::generate());
        let text = format!("{name} {reference}");
        app.insert_files(vec![attachments::Loaded {
            text: text.clone(),
            image: Some(nouli_core::media::Image {
                reference: reference.clone(),
                name: name.into(),
                format: "png".into(),
                data_base64: String::new(),
            }),
        }]);
        assert_eq!(app.form.input, text);
        for character in ['h', 'w'] {
            app.form.input = text.clone();
            app.form.cursor = text.len();
            press(
                &mut app,
                KeyCode::Char(character),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            );
            assert_eq!(
                app.form.input,
                format!("{text}{character}"),
                "AltGr characters must preserve the attachment"
            );
        }
        app.form.input = text.clone();
        app.form.cursor = text.len();
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(app.form.cursor, 0);
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert_eq!(app.form.cursor, text.len());
        press(&mut app, KeyCode::Backspace, KeyModifiers::NONE);
        assert_eq!(app.form.input, "");
        app.form.input = text.clone();
        app.form.cursor = 0;
        press(&mut app, KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(
            app.form.input.is_empty(),
            "Ctrl-D also removes the full attachment"
        );
        for (key, modifiers) in [
            (KeyCode::Char('w'), KeyModifiers::CONTROL),
            (KeyCode::Backspace, KeyModifiers::CONTROL),
            (KeyCode::Backspace, KeyModifiers::ALT),
        ] {
            app.form.input = format!("prefix {text} ");
            app.form.cursor = app.form.input.len();
            press(&mut app, key, modifiers);
            assert_eq!(
                app.form.input, "prefix ",
                "word deletion must include the attachment, not just its hidden reference"
            );
        }
        app.form.input.clear();
        app.repl
            .out
            .extend_from_slice(format!("photo \"中文\".png {reference}\n").as_bytes());
        app.drain();
        assert!(
            !transcript(&app).contains(MarkerKind::Image.prefix()),
            "all human transcript output hides image metadata"
        );
        app.form.question = "A text-only question?".into();
        let draft = draft_from(app.send_form(false).unwrap().unwrap());
        assert!(
            draft.request.updates.is_empty(),
            "deleted attachments are not sent"
        );
    }

    #[test]
    fn raw_drop_is_resolved_before_immediate_send_and_only_submitted_images_are_required() {
        let settings = settings();
        let mut app = app(&settings);
        let dir = std::env::temp_dir().join(format!("nouli-drop-send-{}", Id::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("photo \"test\" 中文.png");
        image::RgbImage::new(2, 2).save(&path).unwrap();
        app.form.kind = Kind::Choice;
        app.form.question = "Which image: [this] or <that>?".into();
        app.form.options = vec![String::new(), "Other".into()];
        app.form.row = Row::Option(0);
        let quoted = shlex::try_quote(path.to_str().unwrap()).unwrap();
        typed(&mut app, &quoted);
        let mut running = None;
        app.key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            &mut running,
        )
        .unwrap();
        let draft = draft_from(running.unwrap());
        assert_eq!(draft.request.turn.questions[0].prompt, app.form.question);
        let record = draft.request.updates.last().unwrap().new_record().unwrap();
        assert_eq!(draft.request.required_records, [record.id]);
        let image = record.as_image().unwrap().unwrap();
        assert!(image.name.contains("\"test\""));
        assert!(app.form.options[0].contains(MarkerKind::Image.prefix()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn options_paste_as_separate_rows_and_shift_tab_cycles_answer_types() {
        let settings = settings();
        let mut app = app(&settings);
        app.form.kind = Kind::Choice;
        app.form.focus(Section::Options, &mut app.repl.queue.facts);
        app.event(
            Event::Paste("Palm Springs\nDenver\nLos Angeles\nSan Diego\nSan Francisco".into()),
            &mut None,
        )
        .unwrap();
        assert_eq!(app.form.options.len(), 5);
        assert_eq!(app.form.options[3], "San Diego");
        press(&mut app, KeyCode::BackTab, KeyModifiers::NONE);
        assert_eq!((app.form.kind, app.form.row), (Kind::Score, Row::Option(4)));
        press(&mut app, KeyCode::Tab, KeyModifiers::SHIFT);
        assert_eq!((app.form.kind, app.form.row), (Kind::Noul, Row::Question));
        press(&mut app, KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!((app.form.kind, app.form.row), (Kind::Choice, Row::Question));
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Option(0));
        assert_eq!(app.form.kind, Kind::Choice);
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Option(1));
        for _ in 0..4 {
            press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        }
        assert_eq!(app.form.row, Row::Option(5));
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Input);
    }

    #[test]
    fn an_idle_drop_failure_blocks_both_enter_and_send_until_corrected() {
        let settings = settings();
        let mut app = app(&settings);
        let path = std::env::temp_dir().join(format!("nouli-binary-{}", Id::generate()));
        std::fs::write(&path, [0_u8, 255, 0]).unwrap();
        typed(&mut app, path.to_str().unwrap());
        assert!(!app.consume_drop()); // the idle-timer path
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(app.repl.queue.facts, [] as [std::string::String; 0]);
        app.form.question = "Is this relevant?".into();
        assert!(app.send_form(false).unwrap().is_none());
        app.form.clear_row(&mut app.repl.queue.facts);
        assert!(app.send_form(false).unwrap().is_some());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_failed_file_paste_cancels_the_pending_second_enter() {
        let settings = settings();
        let mut app = app(&settings);
        app.form.kind = Kind::Choice;
        app.form.question = "Which fits?".into();
        app.form.options = vec!["First".into(), "Second".into()];
        app.form.row = Row::Option(1);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.option_enter);
        let path = std::env::temp_dir().join(format!("nouli-list-drop-{}", Id::generate()));
        std::fs::write(&path, [0, 255]).unwrap();
        app.event(Event::Paste(path.to_string_lossy().into_owned()), &mut None)
            .unwrap();
        assert!(app.status.as_ref().unwrap().error);
        assert!(!app.option_enter);
        press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
        assert!(
            app.option_enter,
            "the next Enter arms again instead of sending"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn changing_fields_immediately_after_a_raw_drop_keeps_the_original_destination() {
        let settings = settings();
        let mut app = app(&settings);
        let path = std::env::temp_dir().join(format!("nouli-tab-drop-{}", Id::generate()));
        std::fs::write(&path, "Imported context.").unwrap();
        typed(&mut app, path.to_str().unwrap());
        press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(app.form.row, Row::Question);
        assert_eq!(app.form.input, "Imported context.");
        std::fs::remove_file(path).unwrap();
    }
}
