//! Read-only session graph presentation; complete records remain in inspection.
mod card;
mod edges;
mod graph;
mod mermaid;
mod question;
mod references;

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
pub(crate) use references::include as include_references;

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

/// A turn, or one question when exact reference endpoints need separate cards.
struct Node<'a> {
    title: String,
    latest: bool,
    parents: Vec<usize>,
    turn: &'a FinalizedTurn,
    question: Option<usize>,
    references: Vec<usize>,
}

fn nodes<'a>(exports: &[&'a SessionExport], frontier: &[Id]) -> Result<Vec<Node<'a>>, RenderError> {
    let mut seen = HashSet::new();
    let turns = exports
        .iter()
        .flat_map(|export| &export.turns)
        .filter(|turn| seen.insert(turn.id))
        .collect::<Vec<_>>();
    let links = references::links(&turns)?;
    let dependencies = Dependencies::new(&turns, &links)?;
    let mut builder = NodeBuilder {
        links: &links,
        split: &dependencies.split,
        frontier,
        turn_labels: abbreviations(turns.iter().map(|turn| turn.id)),
        session_labels: session_labels(exports),
        nodes: Vec::new(),
        groups: HashMap::new(),
    };
    for index in dependencies.order()? {
        builder.add_turn(turns[index])?;
    }
    Ok(builder.nodes)
}

/// Which turns each turn depends on, through its parents or the answers it references.
struct Dependencies {
    /// For each turn index, the indexes of the turns it depends on.
    parents: Vec<HashSet<usize>>,
    /// Turns at either end of a reference, whose questions may need separate cards.
    split: HashSet<Id>,
}

impl Dependencies {
    fn new(turns: &[&FinalizedTurn], links: &[references::Link]) -> Result<Self, RenderError> {
        let by_id = turns
            .iter()
            .enumerate()
            .map(|(index, turn)| (turn.id, index))
            .collect::<HashMap<_, _>>();
        let mut parents = vec![HashSet::new(); turns.len()];
        for (index, turn) in turns.iter().enumerate() {
            for parent in &turn.parents {
                parents[index].insert(*by_id.get(parent).ok_or(RenderError::Protocol)?);
            }
        }
        let mut split = HashSet::new();
        for link in links {
            let source = *by_id.get(&link.source.turn).ok_or(RenderError::Protocol)?;
            if turns[source].session != link.source.session
                || !turns[source]
                    .request
                    .questions
                    .iter()
                    .any(|q| q.id == link.source.question)
            {
                return Err(RenderError::Protocol);
            }
            parents[*by_id.get(&link.target).ok_or(RenderError::Protocol)?].insert(source);
            split.insert(link.source.turn);
            split.insert(link.target);
        }
        Ok(Self { parents, split })
    }

    /// Turn indexes with every turn after the turns it depends on, breaking
    /// ties by index. A cycle is a protocol error.
    fn order(&self) -> Result<Vec<usize>, RenderError> {
        let mut children = vec![Vec::new(); self.parents.len()];
        let mut remaining = self.parents.iter().map(HashSet::len).collect::<Vec<_>>();
        for (child, parents) in self.parents.iter().enumerate() {
            for parent in parents {
                children[*parent].push(child);
            }
        }
        let mut ready = remaining
            .iter()
            .enumerate()
            .filter(|(_, count)| **count == 0)
            .map(|(index, _)| index)
            .collect::<std::collections::BTreeSet<_>>();
        let mut order = Vec::new();
        while let Some(index) = ready.pop_first() {
            order.push(index);
            for child in &children[index] {
                remaining[*child] -= 1;
                if remaining[*child] == 0 {
                    ready.insert(*child);
                }
            }
        }
        if order.len() != self.parents.len() {
            return Err(RenderError::Protocol);
        }
        Ok(order)
    }
}

/// Each session's display name, or its abbreviated ID when it is unnamed.
fn session_labels(exports: &[&SessionExport]) -> HashMap<Id, String> {
    let sessions = exports
        .iter()
        .flat_map(|export| &export.provenance)
        .map(|session| (session.id, session))
        .collect::<HashMap<_, _>>();
    let abbreviated = abbreviations(sessions.keys().copied());
    sessions
        .into_iter()
        .map(|(id, session)| {
            let label = session.name.as_ref().map_or_else(
                || format!("(unnamed {})", abbreviated[&id]),
                |name| name.as_str().to_owned(),
            );
            (id, label)
        })
        .collect()
}

/// Builds graph nodes from turns given in dependency order.
struct NodeBuilder<'a, 'g> {
    links: &'g [references::Link],
    split: &'g HashSet<Id>,
    frontier: &'g [Id],
    turn_labels: HashMap<Id, String>,
    session_labels: HashMap<Id, String>,
    nodes: Vec<Node<'a>>,
    /// The nodes added for each turn so far.
    groups: HashMap<Id, Vec<usize>>,
}

impl<'a> NodeBuilder<'a, '_> {
    /// Adds a turn's node, or one node per question when a reference splits a
    /// turn with several questions.
    fn add_turn(&mut self, turn: &'a FinalizedTurn) -> Result<(), RenderError> {
        let session_label = self
            .session_labels
            .get(&turn.session)
            .ok_or(RenderError::Protocol)?;
        let turn_label = self
            .turn_labels
            .get(&turn.id)
            .ok_or(RenderError::Protocol)?;
        let latest = self.frontier.contains(&turn.id);
        let count = turn.request.questions.len();
        let questions = if self.split.contains(&turn.id) && count > 1 {
            (0..count).map(Some).collect::<Vec<_>>()
        } else {
            vec![None]
        };
        let mut group = Vec::new();
        for question in questions {
            let mut parents = turn
                .parents
                .iter()
                .flat_map(|parent| self.groups.get(parent).into_iter().flatten().copied())
                .collect::<Vec<_>>();
            let referenced = self.referenced(turn, question)?;
            parents.extend(&referenced);
            parents.sort_unstable();
            parents.dedup();
            let question_label =
                question.map_or_else(String::new, |q| format!(" · {}/{count}", q + 1));
            let latest_label = if latest { " [latest]" } else { "" };
            group.push(self.nodes.len());
            self.nodes.push(Node {
                title: format!("{session_label} · {turn_label}{question_label}{latest_label}"),
                latest,
                parents,
                turn,
                question,
                references: referenced,
            });
        }
        self.groups.insert(turn.id, group);
        Ok(())
    }

    /// The nodes holding the answers that `turn` references, limited to one
    /// question's references when `question` is given; sorted and deduplicated.
    fn referenced(
        &self,
        turn: &FinalizedTurn,
        question: Option<usize>,
    ) -> Result<Vec<usize>, RenderError> {
        let mut referenced = Vec::new();
        for link in self.links.iter().filter(|link| {
            link.target == turn.id
                && question.is_none_or(|q| turn.request.questions[q].id == link.question)
        }) {
            let source = self
                .groups
                .get(&link.source.turn)
                .ok_or(RenderError::Protocol)?;
            let parent = *source
                .iter()
                .find(|node| {
                    let source: &Node<'_> = &self.nodes[**node];
                    source.question.is_none_or(|index| {
                        source.turn.request.questions[index].id == link.source.question
                    })
                })
                .ok_or(RenderError::Protocol)?;
            referenced.push(parent);
        }
        referenced.sort_unstable();
        referenced.dedup();
        Ok(referenced)
    }
}

/// Summarize a turn's questions in caller order with their typed results.
fn describe(lines: &mut impl Lines, turn: &FinalizedTurn) -> Result<(), RenderError> {
    describe_question(lines, turn, None)
}

fn describe_node(lines: &mut impl Lines, node: &Node<'_>) -> Result<(), RenderError> {
    match node.question {
        Some(question) => describe_question(lines, node.turn, Some(question)),
        None => describe(lines, node.turn),
    }
}

fn describe_question(
    lines: &mut impl Lines,
    turn: &FinalizedTurn,
    selected: Option<usize>,
) -> Result<(), RenderError> {
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
        if selected.is_some_and(|selected| selected != index) {
            continue;
        }
        if selected.is_none() && index > 0 {
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
        describe_node(&mut content, &node)?;
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
    use nouli_core::{
        history::ModelSelection, outcome::Answer, storage::inspection::SessionProvenance,
    };

    pub(super) fn fixture() -> SessionExport {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/contracts/history.json")).unwrap();
        let mut turns: Vec<FinalizedTurn> =
            serde_json::from_value(fixture["turns"].clone()).unwrap();
        turns.truncate(4);
        let session = nouli_core::storage::sessions::Session {
            id: turns[3].session,
            name: Some("Combined".to_owned().try_into().unwrap()),
            defaults: ModelSelection::default(),
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
                defaults: ModelSelection::default(),
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

    fn referenced_questions() -> SessionExport {
        use nouli_core::{
            context::{
                ContextUpdate,
                evidence::{DecisionReference, DecisionSource, marker},
            },
            history::{RecordProjection, Representation},
        };
        let mut export = fixture();
        let mut source = export.turns[0].clone();
        source.request.questions.truncate(2);
        source.outcome.outcomes.truncate(2);
        let mut target = source.clone();
        target.id = Id::generate();
        target.session = export.session.id;
        target.parents.clear();
        let id = Id::generate();
        let reference = marker::decision_marker(id);
        target.request.questions[1].prompt = format!("Reconsider {reference}");
        let snapshot = DecisionReference::snapshot(
            id,
            DecisionSource {
                session: source.session,
                turn: source.id,
                question: source.request.questions[1].id.clone(),
            },
            source.request.questions[1].clone(),
            source.outcome.outcomes[1].clone(),
            source.outcome.provider.clone(),
            Vec::new(),
            serde_json::json!({}),
        );
        target.updates = vec![ContextUpdate::Append {
            record: snapshot.record(id).unwrap(),
        }];
        target.manifest.records = vec![RecordProjection {
            record: id,
            representation: Representation::Full,
            protected: true,
        }];
        export.session.frontier = vec![target.id];
        // Dependency discovery must work even when the source is loaded later.
        export.turns = vec![target, source];
        export
    }

    #[test]
    fn a_malformed_reference_fails_the_graph_instead_of_dropping_its_link() {
        let mut export = referenced_questions();
        let nouli_core::context::ContextUpdate::Append { record } = &mut export.turns[0].updates[0]
        else {
            panic!("the fixture appends its reference");
        };
        record.value =
            nouli_core::data::Data::new(serde_json::json!({"nouli_decision_reference": {}}))
                .unwrap();
        let Err(error) = nodes(&[&export], &export.session.frontier) else {
            panic!("a malformed reference must fail the graph");
        };
        assert!(matches!(error, RenderError::Evidence(_)), "{error}");
        assert!(error.to_string().contains("saved-answer reference"));
    }

    #[test]
    fn references_connect_exact_questions_once_and_sort_sources_before_destinations() {
        let mut export = referenced_questions();
        let binding = export.turns[0].updates[0].clone();
        export.turns[0].updates.push(binding);
        let nodes = nodes(&[&export], &export.session.frontier).unwrap();
        assert_eq!(nodes.len(), 4);
        assert_eq!(nodes[0].turn.id, export.turns[1].id);
        assert_eq!(nodes[1].question, Some(1));
        assert_eq!(nodes[0].parents, [] as [usize; 0]);
        assert_eq!(nodes[1].parents, [] as [usize; 0]);
        assert_eq!(nodes[2].parents, [] as [usize; 0]);
        assert_eq!(nodes[3].parents, vec![1]);
        assert_eq!(nodes[3].references, vec![1]);
        let mut mermaid = Vec::new();
        write_mermaid(&export, &mut mermaid).unwrap();
        let mermaid = String::from_utf8(mermaid).unwrap();
        assert_eq!(mermaid.matches("-.->").count(), 1);
        assert!(mermaid.contains("t1 -.-> t3"));
    }

    #[test]
    fn shared_references_connect_all_questions_but_omitted_evidence_creates_no_edge() {
        let mut export = referenced_questions();
        let reference = export.turns[0].request.questions[1].prompt.clone();
        export.turns[0].request.state = serde_json::from_value(serde_json::json!([
            {"type": "text", "text": reference}
        ]))
        .unwrap();
        assert_eq!(nodes(&[&export], &[]).unwrap()[2].references, vec![1]);
        export.turns[0].manifest.records[0].representation =
            nouli_core::history::Representation::Omitted;
        let nodes = nodes(&[&export], &[]).unwrap();
        assert_eq!(
            nodes.len(),
            2,
            "unreferenced turns keep their existing cards"
        );
        assert!(nodes.iter().all(|node| node.parents.is_empty()));
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
            defaults: ModelSelection::default(),
            sources: vec![],
        }];
        let nodes = nodes(&[&original, &original, &new], &new.session.frontier).unwrap();
        assert_eq!(nodes.len(), original.turns.len() + 1);
        assert_eq!(nodes.last().unwrap().parents, [] as [usize; 0]);
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
        *probability = 0.000_000_000_01;
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
