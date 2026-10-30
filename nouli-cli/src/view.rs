//! Read-only session graph presentation; complete records remain in inspection.
mod card;
mod edges;
mod graph;
mod mermaid;
mod question;

use crate::{
    render::{Format, OutputArgs, RenderError},
    selection::selector,
};
use nouli_core::{
    history::FinalizedTurn,
    identity::{Id, SessionSelector},
    outcome::QuestionOutcome,
    storage::inspection::SessionExport,
};
use question::Lines;
use std::{
    collections::{HashMap, HashSet},
    io::Write,
    ops::Range,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Saved session name or ID whose ancestry should be shown.
    #[arg(value_name = "SESSION", value_parser = selector)]
    pub session: SessionSelector,
    /// Print a Mermaid flowchart for Markdown instead of the terminal graph.
    #[arg(long)]
    pub mermaid: bool,
}

pub(crate) fn format(args: &OutputArgs, mermaid: bool) -> Result<Format, RenderError> {
    // Global output flags can precede the subcommand, beyond clap's conflict checks.
    if mermaid && (args.json || args.jsonl || args.format.is_some()) {
        return Err(RenderError::Modes);
    }
    if args.json || args.jsonl || !matches!(args.format, None | Some(Format::Auto | Format::Text)) {
        return Err(RenderError::Modes);
    }
    // Graphs stay text in pipes and regardless of the configured report format.
    Ok(Format::Text)
}

pub(crate) use mermaid::write as write_mermaid;

fn abbreviations(ids: impl Iterator<Item = Id>) -> HashMap<Id, String> {
    let mut ids: Vec<_> = ids.map(|id| (id, id.to_string())).collect();
    ids.sort_unstable_by(|a, b| a.1.cmp(&b.1));
    let width = ids.windows(2).fold(8, |width, pair| {
        width.max(
            pair[0]
                .1
                .bytes()
                .zip(pair[1].1.bytes())
                .take_while(|(a, b)| a == b)
                .count()
                + 1,
        )
    });
    ids.into_iter()
        .map(|(id, text)| (id, text[..width.min(text.len())].to_owned()))
        .collect()
}

/// One finalized turn in canonical order, with parents as indices of earlier nodes.
struct Node<'a> {
    title: String,
    latest: bool,
    parents: Vec<usize>,
    turn: &'a FinalizedTurn,
}

fn nodes<'a>(exports: &[&'a SessionExport], frontier: &[Id]) -> Result<Vec<Node<'a>>, RenderError> {
    let mut seen = HashSet::new();
    let turns: Vec<_> = exports
        .iter()
        .flat_map(|export| &export.turns)
        .filter(|turn| seen.insert(turn.id))
        .collect();
    let labels = abbreviations(turns.iter().map(|turn| turn.id));
    let sessions: HashMap<_, _> = exports
        .iter()
        .flat_map(|export| &export.provenance)
        .map(|session| (session.id, session))
        .collect();
    let session_labels = abbreviations(sessions.keys().copied());
    let indices: HashMap<_, _> = turns
        .iter()
        .enumerate()
        .map(|(index, turn)| (turn.id, index))
        .collect();
    turns
        .into_iter()
        .map(|turn| {
            let session = sessions.get(&turn.session).ok_or(RenderError::Protocol)?;
            let session_label = session
                .name
                .as_ref()
                .map(|name| name.as_str().to_owned())
                .unwrap_or_else(|| format!("(unnamed {})", session_labels[&session.id]));
            let latest = frontier.contains(&turn.id);
            Ok(Node {
                title: format!(
                    "{} · {}{}",
                    session_label,
                    labels.get(&turn.id).ok_or(RenderError::Protocol)?,
                    if latest { " [latest]" } else { "" }
                ),
                latest,
                parents: turn
                    .parents
                    .iter()
                    .map(|id| indices.get(id).copied().ok_or(RenderError::Protocol))
                    .collect::<Result<_, _>>()?,
                turn,
            })
        })
        .collect()
}

/// Summarize a turn's questions in caller order with their typed results.
fn describe(lines: &mut impl Lines, turn: &FinalizedTurn) -> Result<(), RenderError> {
    if let Some(node) = &turn.workflow_node {
        lines.text(&format!("Workflow node: {node}"));
    }
    if turn.runtime_only {
        lines.text("Workflow context record");
    }
    let outcomes: HashMap<_, _> = turn
        .outcome
        .outcomes
        .iter()
        .map(|outcome| {
            let (QuestionOutcome::Answered { question_id, .. }
            | QuestionOutcome::Refused { question_id, .. }) = outcome;
            (question_id.as_str(), outcome)
        })
        .collect();
    for (index, question) in turn.request.questions.iter().enumerate() {
        if index > 0 {
            lines.text("");
        }
        let outcome = outcomes
            .get(question.id.as_str())
            .ok_or(RenderError::Protocol)?;
        question::append(lines, question, outcome)?;
    }
    Ok(())
}

pub(crate) fn write(
    export: &SessionExport,
    sink: &mut impl Write,
    color: bool,
) -> Result<(), RenderError> {
    let selected = &export.session;
    writeln!(
        sink,
        "Session: {} ({})",
        selected.name.as_ref().map_or("(unnamed)", |n| n.as_str()),
        selected.id
    )?;
    writeln!(
        sink,
        "Earlier decisions at the top; arrows lead to later decisions.\n"
    )?;
    if export.turns.is_empty() {
        writeln!(
            sink,
            "No finalized turns. Use nouli sessions show for saved attempt details."
        )?;
    } else {
        cards(export, sink, color)?;
        writeln!(
            sink,
            "\nUse nouli sessions show or nouli context for complete details."
        )?;
    }
    sink.flush()?;
    Ok(())
}

/// A card's first row and its columns in the drawn graph.
pub(crate) type Placement = (usize, Range<usize>);

/// The decision cards and the wires between them, without the session
/// header, for a pane beside other content. A session with no finalized
/// turns writes nothing. Returns where the latest card is.
pub(crate) fn cards(
    export: &SessionExport,
    sink: &mut impl Write,
    color: bool,
) -> Result<Option<Placement>, RenderError> {
    draw_cards(nodes(&[export], &export.session.frontier)?, sink, color)
}

/// A pane can retain several independent sessions without inventing parent edges.
/// Shared ancestry is drawn once, and the selected session supplies the heads.
pub(crate) fn cards_many(
    exports: &[SessionExport],
    selected: Option<Id>,
    sink: &mut impl Write,
    color: bool,
) -> Result<Option<Placement>, RenderError> {
    let frontier = exports
        .iter()
        .find(|export| Some(export.session.id) == selected)
        .map_or(&[][..], |export| export.session.frontier.as_slice());
    draw_cards(
        nodes(&exports.iter().collect::<Vec<_>>(), frontier)?,
        sink,
        color,
    )
}

fn draw_cards(
    nodes: Vec<Node<'_>>,
    sink: &mut impl Write,
    color: bool,
) -> Result<Option<Placement>, RenderError> {
    let mut cards = vec![];
    let mut latest = None;
    for (index, node) in nodes.into_iter().enumerate() {
        // A blend can leave several heads; the last is the newest.
        if node.latest {
            latest = Some(index);
        }
        let mut content = vec![];
        describe(&mut content, node.turn)?;
        cards.push(graph::Card {
            parents: node.parents,
            rows: card::frame(&node.title, content),
        });
    }
    let placed = graph::write(&cards, sink, color)?;
    Ok(latest.map(|index| {
        let (row, left) = placed[index];
        (row, left..left + card::WIDTH)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nouli_core::{outcome::Answer, storage::inspection::SessionProvenance};

    pub(super) fn fixture() -> SessionExport {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/contracts/history.json")).unwrap();
        let mut turns: Vec<FinalizedTurn> =
            serde_json::from_value(fixture["turns"].clone()).unwrap();
        turns.truncate(4);
        let session = nouli_core::storage::sessions::Session {
            id: turns[3].session,
            name: Some("Combined".to_owned().try_into().unwrap()),
            defaults: Default::default(),
            frontier: vec![turns[3].id],
            revision: 1,
        };
        let provenance = turns
            .iter()
            .map(|turn| SessionProvenance {
                id: turn.session,
                name: Some(
                    format!("Session{}", turn.id.to_string().chars().last().unwrap())
                        .try_into()
                        .unwrap(),
                ),
                defaults: Default::default(),
                sources: vec![],
            })
            .collect();
        SessionExport {
            version: 1,
            session,
            turns,
            provenance,
            sources: vec![],
            attempts: vec![],
            requests: vec![],
            workflow_definitions: vec![],
            workflows: vec![],
            node_intents: vec![],
            context: nouli_core::storage::inspection::ContextView::Resolved { records: vec![] },
        }
    }

    fn text(export: &SessionExport) -> String {
        let mut sink = vec![];
        write(export, &mut sink, false).unwrap();
        String::from_utf8(sink).unwrap()
    }

    #[test]
    fn several_sessions_keep_independent_roots_and_deduplicate_shared_ancestry() {
        let original = fixture();
        let mut new = original.clone();
        new.session.id = Id::generate();
        new.session.name = Some("Independent".to_owned().try_into().unwrap());
        new.turns.truncate(1);
        new.turns[0].id = Id::generate();
        new.turns[0].session = new.session.id;
        new.turns[0].parents.clear();
        new.session.frontier = vec![new.turns[0].id];
        new.provenance = vec![SessionProvenance {
            id: new.session.id,
            name: new.session.name.clone(),
            defaults: Default::default(),
            sources: vec![],
        }];
        let nodes = nodes(&[&original, &original, &new], &new.session.frontier).unwrap();
        assert_eq!(nodes.len(), original.turns.len() + 1);
        assert!(nodes.last().unwrap().parents.is_empty());
        assert!(nodes.last().unwrap().latest);
        let mut text = Vec::new();
        assert!(
            cards_many(
                &[original, new.clone()],
                Some(new.session.id),
                &mut text,
                false
            )
            .unwrap()
            .is_some()
        );
        assert!(String::from_utf8(text).unwrap().contains("Independent"));
    }

    #[test]
    fn summaries_preserve_question_order_and_typed_result_meanings() {
        let mut export = fixture();
        export.turns[3].outcome.outcomes.reverse();
        let text = text(&export);
        assert!(text.contains("Which route?"));
        assert!(text.contains("▶ Rail"));
        assert!(text.contains("Air"));
        assert!(text.contains("Probability"));
        assert!(text.contains("▶ P(yes)=1"));
        assert!(text.contains("▶ Expected score: 1.43"));
        assert!(text.contains("0 Poor"));
        assert!(text.contains("1 Adequate"));
        assert!(text.contains("2 Excellent"));
        assert!(text.contains("Refused:"));
        assert!(text.contains("information"));
        assert!(text.find("Which route?").unwrap() < text.find("Will the train").unwrap());
        assert_eq!(text.matches("[latest]").count(), 1);
        for name in ["Session1", "Session2", "Session3", "Session4"] {
            assert_eq!(text.matches(name).count(), 1);
        }
        assert!(!text.contains("Choose travel using the supplied evidence"));
        assert!(!text.contains("fixture native certainty"));
    }

    #[test]
    fn labels_are_unique_even_when_uuid_prefixes_collide() {
        let export = fixture();
        let labels = abbreviations(export.turns.iter().map(|turn| turn.id));
        let unique: std::collections::HashSet<_> = labels.values().collect();
        assert_eq!(unique.len(), 4);
        assert!(labels.values().all(|label| label.len() > 8));
        let only = abbreviations(std::iter::once(export.turns[0].id));
        assert_eq!(only[&export.turns[0].id].len(), 8);
    }

    #[test]
    fn full_questions_wrap_and_terminal_controls_are_escaped() {
        let mut export = fixture();
        export.turns[3].request.questions[0].prompt =
            format!("{}FINAL_WORD", "A long question ".repeat(30));
        let output = text(&export);
        assert!(output.contains("FINAL_WORD"));
        assert!(!output.contains("..."));
        export.turns[3].request.questions[0].prompt = "Q\n\u{1b}[31m\t\r?".into();
        let output = text(&export);
        assert!(output.contains("Q\\n\\u{1b}[31m\\t\\r?"));
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn very_small_probabilities_are_not_rounded_to_zero() {
        let mut export = fixture();
        let QuestionOutcome::Answered {
            answer: Answer::Noul { probability },
            ..
        } = &mut export.turns[3].outcome.outcomes[1]
        else {
            panic!("noul")
        };
        *probability = 0.00000000001;
        assert!(text(&export).contains("P(yes)=0.00000000001"));
    }

    #[test]
    fn cards_say_where_the_latest_card_is_even_when_its_title_wraps() {
        let mut export = fixture();
        let head = export.turns[3].session;
        for session in &mut export.provenance {
            if session.id == head {
                session.name = Some("merged-picking-plan".to_owned().try_into().unwrap());
            }
        }
        let mut sink = vec![];
        let (row, columns) = cards(&export, &mut sink, false).unwrap().unwrap();
        let text = String::from_utf8(sink).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let card = |row: usize| -> String {
            lines[row]
                .chars()
                .skip(columns.start)
                .take(columns.len())
                .collect()
        };
        assert!(card(row).starts_with("┌─ merged-picking-plan"), "{text}");
        assert!(card(row).ends_with('┐'), "{text}");
        // The long name pushes the tag below the card's top row.
        assert!(!card(row).contains("[latest]"), "{text}");
        assert!(
            (row + 1..row + 4).any(|row| card(row).contains("[latest]")),
            "{text}"
        );
        export.session.frontier.clear();
        assert_eq!(cards(&export, &mut vec![], false).unwrap(), None);
    }

    #[test]
    fn empty_and_inherited_only_sessions_have_no_invented_head() {
        let mut export = fixture();
        export.session.frontier.clear();
        assert!(!text(&export).contains("[latest]"));
        export.turns.clear();
        let output = text(&export);
        assert!(output.contains("No finalized turns"));
        assert!(!output.contains("┌─ "));
    }

    #[test]
    fn workflow_context_records_are_labeled_without_invented_questions() {
        let mut export = fixture();
        export.turns[3].workflow_node = Some("prepare".into());
        export.turns[3].runtime_only = true;
        export.turns[3].request.questions.clear();
        export.turns[3].outcome.outcomes.clear();
        let output = text(&export);
        assert!(output.contains("Workflow node: prepare"));
        assert!(output.contains("Workflow context record"));
        assert_eq!(output.matches("▶ ").count(), 9);
    }

    #[test]
    fn multiple_selected_heads_and_unnamed_sources_are_visible() {
        let mut export = fixture();
        export.turns.truncate(3);
        export.turns[1].session = export.session.id;
        export.turns[2].session = export.session.id;
        export.session.frontier = vec![export.turns[1].id, export.turns[2].id];
        export.provenance[0].name = None;
        let output = text(&export);
        assert_eq!(output.matches("[latest]").count(), 2);
        assert!(output.contains("(unnamed"));
    }

    #[test]
    fn write_failures_propagate() {
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
            write(&fixture(), &mut Broken, false),
            Err(RenderError::Write(_))
        ));
    }
}
