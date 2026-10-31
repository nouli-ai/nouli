//! Mermaid flowchart source for Markdown; renderers such as GitHub draw the boxes.
use super::question::Lines;
use crate::render::{RenderError, clean};
use nouli_core::storage::inspection::SessionExport;
use std::io::Write;

/// Label width in pixels; Mermaid wraps long lines itself, so labels only break lines.
const WIDTH: usize = 320;

/// Mermaid entity codes keep caller text from closing labels, injecting markup, or
/// forming `%%` comments and directives, which Mermaid matches anywhere in the source.
fn escape(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '#' => "#35;".to_owned(),
            '%' => "#37;".to_owned(),
            '\u{2028}' => "#8232;".to_owned(),
            '\u{2029}' => "#8233;".to_owned(),
            '"' => "#quot;".to_owned(),
            '&' => "#amp;".to_owned(),
            '<' => "#lt;".to_owned(),
            '>' => "#gt;".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

#[derive(Default)]
struct Label(Vec<String>);

impl Label {
    fn push(&mut self, value: &str, bold: bool) {
        let line = escape(&clean(value));
        self.0
            .push(if bold { format!("<b>{line}</b>") } else { line });
    }

    fn finish(self) -> String {
        let mut lines: Vec<String> = vec![];
        for line in self.0 {
            if !(line.is_empty() && lines.last().is_none_or(String::is_empty)) {
                lines.push(line);
            }
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("<br/>")
    }
}

impl Lines for Label {
    fn text(&mut self, value: &str) {
        if value.is_empty() {
            self.0.push(String::new());
        } else {
            self.push(value, false);
        }
    }

    fn selected(&mut self, value: &str) {
        self.push(&format!("▶ {value}"), true);
    }

    fn table(&mut self, _: &str, entries: &[(String, Option<f64>, bool)]) {
        for (label, probability, selected) in entries {
            let probability = probability.map_or_else(|| "n/a".into(), |value| value.to_string());
            if *selected {
                self.selected(&format!("{label} · {probability}"));
            } else {
                self.push(&format!("{label} · {probability}"), false);
            }
        }
    }
}

/// Write a top-down flowchart, separating questions at exact reference endpoints.
pub(crate) fn write(export: &SessionExport, sink: &mut impl Write) -> Result<(), RenderError> {
    let selected = &export.session;
    writeln!(
        sink,
        "%% Nouli session: {} ({})",
        clean(selected.name.as_ref().map_or("(unnamed)", |n| n.as_str())),
        selected.id
    )?;
    writeln!(
        sink,
        "%%{{init: {{\"flowchart\": {{\"wrappingWidth\": {WIDTH}}}}}}}%%"
    )?;
    writeln!(sink, "flowchart TD")?;
    if export.turns.is_empty() {
        writeln!(sink, "  empty[\"No finalized turns\"]")?;
    } else {
        let nodes = super::nodes(&[export], &export.session.frontier)?;
        for (index, node) in nodes.iter().enumerate() {
            let mut label = Label::default();
            label.push(&node.title, true);
            label.text("");
            super::describe_node(&mut label, node)?;
            writeln!(sink, "  t{index}[\"{}\"]", label.finish())?;
        }
        for (index, node) in nodes.iter().enumerate() {
            for parent in &node.parents {
                writeln!(
                    sink,
                    "  t{parent} {} t{index}",
                    if node.references.contains(parent) {
                        "-.->"
                    } else {
                        "-->"
                    }
                )?;
            }
        }
        let latest: Vec<_> = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.latest)
            .map(|(index, _)| format!("t{index}"))
            .collect();
        if !latest.is_empty() {
            writeln!(sink, "  classDef latest stroke:#0b6e63,stroke-width:3px")?;
            writeln!(sink, "  class {} latest", latest.join(","))?;
        }
    }
    sink.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::tests::fixture;
    use nouli_core::outcome::QuestionOutcome;

    fn text(export: &SessionExport) -> String {
        let mut sink = vec![];
        write(export, &mut sink).unwrap();
        String::from_utf8(sink).unwrap()
    }

    #[test]
    fn turns_become_nodes_with_parent_edges_and_marked_selections() {
        let output = text(&fixture());
        assert!(output.starts_with("%% Nouli session: Combined ("));
        assert!(
            output
                .lines()
                .nth(1)
                .unwrap()
                .contains("\"wrappingWidth\": 320")
        );
        assert_eq!(output.lines().nth(2), Some("flowchart TD"));
        for index in 0..4 {
            assert_eq!(output.matches(&format!("  t{index}[\"")).count(), 1);
        }
        let edges = output.lines().filter(|line| line.contains(" --> ")).count();
        assert_eq!(
            edges,
            fixture()
                .turns
                .iter()
                .map(|turn| turn.parents.len())
                .sum::<usize>()
        );
        assert!(output.contains("<b>▶ Rail · 0.7</b>"));
        assert!(output.contains("Air · 0.3"));
        assert!(output.contains("<b>▶ P(yes)=1</b>"));
        assert!(output.contains("<b>▶ Expected score: 1.43</b>"));
        assert!(output.contains("Refused:"));
        assert!(output.contains("  classDef latest stroke:#0b6e63,stroke-width:3px\n"));
        assert!(output.contains("  class t3 latest"));
        assert!(!output.contains("Probability"));
    }

    #[test]
    fn caller_text_cannot_form_mermaid_directives_or_comments() {
        let mut export = fixture();
        export.turns[3].request.questions[0].prompt =
            "a %%{init: {'theme':'forest'}}%% b \u{2028}%% x %%{".into();
        let output = text(&export);
        assert!(output.lines().skip(2).all(|line| !line.contains('%')));
        assert!(output.contains("a #37;#37;{init: {'theme':'forest'}}#37;#37; b #8232;#37;#37; x"));
        assert!(!output.contains('\u{2028}'));
        let label = output
            .lines()
            .find(|line| line.starts_with("  t3["))
            .unwrap();
        assert!(label.ends_with("\"]"));
    }

    #[test]
    fn refused_choices_keep_options_without_probabilities_or_selection() {
        let mut export = fixture();
        let id = export.turns[3].request.questions[0].id.clone();
        let refused = QuestionOutcome::Refused {
            question_id: id.clone(),
            reason: "Insufficient evidence".into(),
        };
        let outcome = export.turns[3]
            .outcome
            .outcomes
            .iter_mut()
            .find(|outcome| {
                matches!(outcome, QuestionOutcome::Answered { question_id, .. } if *question_id == id)
            })
            .unwrap();
        *outcome = refused;
        let output = text(&export);
        let label = output
            .lines()
            .find(|line| line.starts_with("  t3["))
            .unwrap();
        assert!(
            label.contains("<br/>Rail · n/a<br/>Air · n/a<br/><br/>Refused: Insufficient evidence")
        );
        assert!(!label.contains("▶ Rail") && !label.contains("▶ Air"));
    }

    #[test]
    fn caller_text_cannot_close_labels_or_add_markup() {
        let mut export = fixture();
        export.turns[3].request.questions[0].prompt =
            "Pick \"A\" <script>x</script> & #1\n%% t0 --> t1".into();
        let output = text(&export);
        assert!(output.contains("Pick #quot;A#quot; #lt;script#gt;x#lt;/script#gt; #amp; #35;1"));
        assert!(output.contains("#35;1\\n#37;#37; t0 --#gt; t1"));
        assert!(
            !output
                .lines()
                .any(|line| line.trim_start().starts_with("%% t0"))
        );
        assert!(!output.contains("<script>"));
        for line in output.lines().filter(|line| line.contains("[\"")) {
            assert_eq!(line.matches('"').count(), 2, "{line}");
            assert!(line.ends_with("\"]"));
        }
    }

    #[test]
    fn long_text_is_left_for_mermaid_to_wrap_and_blank_lines_collapse() {
        let mut export = fixture();
        let prompt = format!("{}FINAL_WORD", "A long question ".repeat(10));
        export.turns[3].request.questions[0].prompt = prompt.clone();
        let output = text(&export);
        let label = output.lines().find(|line| line.contains(&prompt)).unwrap();
        assert!(label.contains(&format!("<br/><br/>{prompt}<br/><br/>")));
        assert!(!label.contains("<br/><br/><br/>"));
        assert!(!label.contains("<br/>\"]"));
    }

    #[test]
    fn empty_histories_and_write_failures_are_reported() {
        let mut export = fixture();
        export.turns.clear();
        let output = text(&export);
        assert!(output.contains("empty[\"No finalized turns\"]"));
        assert!(!output.contains("classDef"));
        let mut export = fixture();
        export.session.frontier.clear();
        assert!(!text(&export).contains("classDef"));
        struct Broken;
        impl std::io::Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(matches!(
            write(&fixture(), &mut Broken),
            Err(RenderError::Write(_))
        ));
    }
}
