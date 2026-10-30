//! Layered, ancestor-first placement of decision cards and skipped-level wires.
use super::{card, edges};
use crate::render::RenderError;
use std::io::Write;
use unicode_width::UnicodeWidthStr;

pub(super) struct Card {
    pub parents: Vec<usize>,
    pub rows: Vec<card::Row>,
}

struct Vertex {
    card: Option<usize>,
    parents: Vec<usize>,
    column: usize,
}

struct Layout {
    vertices: Vec<Vertex>,
    layers: Vec<Vec<usize>>,
}

impl Layout {
    fn new(cards: &[Card]) -> Result<Self, RenderError> {
        let mut layout = Self {
            vertices: vec![],
            layers: vec![],
        };
        let mut card_vertices = Vec::<(usize, usize)>::new();
        for (index, card) in cards.iter().enumerate() {
            // Canonical exports put every parent before its children.
            if card.parents.iter().any(|parent| *parent >= index) {
                return Err(RenderError::Protocol);
            }
            let level = card
                .parents
                .iter()
                .map(|parent| card_vertices[*parent].1 + 1)
                .max()
                .unwrap_or(0);
            let mut parents = vec![];
            for parent in &card.parents {
                let (mut vertex, parent_level) = card_vertices[*parent];
                // Dedicated waypoints keep long edges outside intermediate cards.
                for skipped in parent_level + 1..level {
                    vertex = layout.push(skipped, None, vec![vertex]);
                }
                parents.push(vertex);
            }
            let vertex = layout.push(level, Some(index), parents);
            card_vertices.push((vertex, level));
        }
        for layer in &mut layout.layers {
            layer.sort_by_key(|id| {
                let vertex = &layout.vertices[*id];
                if vertex.parents.is_empty() {
                    usize::MAX
                } else {
                    vertex
                        .parents
                        .iter()
                        .map(|parent| layout.vertices[*parent].column)
                        .sum::<usize>()
                        / vertex.parents.len()
                }
            });
            let mut next = 0;
            for id in layer {
                let vertex = &layout.vertices[*id];
                let desired = if vertex.parents.is_empty() {
                    next
                } else {
                    vertex
                        .parents
                        .iter()
                        .map(|parent| layout.vertices[*parent].column)
                        .sum::<usize>()
                        / vertex.parents.len()
                };
                layout.vertices[*id].column = desired.max(next);
                next = layout.vertices[*id].column + 2;
            }
        }
        // Center a sole branching ancestor above its children without scanning
        // the whole history once per layer (long chains must remain linear).
        let mut children = vec![(0, 0); layout.vertices.len()];
        for vertex in &layout.vertices {
            for parent in &vertex.parents {
                children[*parent].0 += vertex.column;
                children[*parent].1 += 1;
            }
        }
        for layer in &layout.layers {
            if let [id] = layer.as_slice()
                && children[*id].1 > 1
            {
                layout.vertices[*id].column = children[*id].0 / children[*id].1;
            }
        }
        Ok(layout)
    }

    fn push(&mut self, level: usize, card: Option<usize>, parents: Vec<usize>) -> usize {
        while self.layers.len() <= level {
            self.layers.push(vec![]);
        }
        let id = self.vertices.len();
        self.vertices.push(Vertex {
            card,
            parents,
            column: 0,
        });
        self.layers[level].push(id);
        id
    }

    fn center(&self, id: usize) -> usize {
        self.vertices[id].column * card::STEP + card::CENTER
    }
}

/// Writes the cards in layers with the wires between them, and returns each
/// card's first row and left column, in the order of `cards`.
pub(super) fn write(
    cards: &[Card],
    sink: &mut impl Write,
    color: bool,
) -> Result<Vec<(usize, usize)>, RenderError> {
    let layout = Layout::new(cards)?;
    let mut outgoing = vec![false; layout.vertices.len()];
    for vertex in &layout.vertices {
        for parent in &vertex.parents {
            outgoing[*parent] = true;
        }
    }
    let mut placed = vec![(0, 0); cards.len()];
    let mut top = 0;
    for (level, layer) in layout.layers.iter().enumerate() {
        let height = layer
            .iter()
            .filter_map(|id| layout.vertices[*id].card.map(|card| cards[card].rows.len()))
            .max()
            .unwrap_or(1);
        for id in layer {
            let vertex = &layout.vertices[*id];
            if let Some(index) = vertex.card {
                placed[index] = (top, vertex.column * card::STEP);
            }
        }
        top += height;
        for row in 0..height {
            let mut line = String::new();
            let mut position = 0;
            for id in layer {
                let vertex = &layout.vertices[*id];
                let left = vertex.column * card::STEP;
                let card = vertex.card.map(|card| &cards[card]);
                let (offset, content, highlighted) = match card.and_then(|card| card.rows.get(row))
                {
                    Some(content) => {
                        let highlighted = content.highlighted;
                        let mut content = content.text.clone();
                        if outgoing[*id] && card.is_some_and(|card| row + 1 == card.rows.len()) {
                            content = content
                                .chars()
                                .enumerate()
                                .map(|(index, ch)| if index == card::CENTER { '┬' } else { ch })
                                .collect();
                        }
                        (left, content, highlighted)
                    }
                    None if outgoing[*id] => (layout.center(*id), "│".into(), false),
                    None => continue,
                };
                line.push_str(&" ".repeat(offset - position));
                position = offset + content.width();
                if color && highlighted {
                    // Bold teal, the product accent; terminals without 256 colors fall back to cyan.
                    line.push_str("\x1b[1;36;38;5;37m");
                }
                line.push_str(&content);
                if color && highlighted {
                    line.push_str("\x1b[0m");
                }
            }
            writeln!(sink, "{}", line.trim_end())?;
        }
        if let Some(next) = layout.layers.get(level + 1) {
            let connections: Vec<_> = next
                .iter()
                .flat_map(|id| {
                    layout.vertices[*id]
                        .parents
                        .iter()
                        .map(|parent| (layout.center(*parent), layout.center(*id)))
                })
                .collect();
            // Wires take as many rows as their crossings need.
            let mut wires = vec![];
            edges::write(&connections, &mut wires)?;
            top += wires.iter().filter(|byte| **byte == b'\n').count();
            sink.write_all(&wires)?;
        }
    }
    Ok(placed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cards(parents: &[&[usize]]) -> Vec<Card> {
        parents
            .iter()
            .enumerate()
            .map(|(i, parents)| {
                let mut content = vec![];
                card::text(&mut content, "Q: ", &format!("Question {}?", i + 1));
                card::text(&mut content, "A: ", &format!("Answer {}", i + 1));
                Card {
                    parents: parents.to_vec(),
                    rows: card::frame(&format!("T{}", i + 1), content),
                }
            })
            .collect()
    }

    fn text(cards: &[Card]) -> String {
        let mut output = vec![];
        write(cards, &mut output, false).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn independent_chains_are_side_by_side_and_blend_below_them() {
        let text = text(&cards(&[&[], &[0], &[], &[2], &[1, 3]]));
        let rows: Vec<_> = text.lines().filter(|line| line.contains("┌─ ")).collect();
        assert!(rows[0].contains("T1") && rows[0].contains("T3"));
        assert!(rows[1].contains("T2") && rows[1].contains("T4"));
        assert!(rows[2].starts_with(&format!("{}┌─ T5", " ".repeat(card::STEP))));
        assert!(text.contains("└───────────────────┬───────────────────┘"));
    }

    #[test]
    fn shared_ancestor_is_centered_above_a_diamond_and_printed_once() {
        let output = text(&cards(&[&[], &[0], &[0], &[1, 2]]));
        let rows: Vec<_> = output.lines().filter(|line| line.contains("┌─ ")).collect();
        assert!(rows[0].starts_with(&format!("{}┌─ T1", " ".repeat(card::STEP))));
        assert!(rows[1].contains("T2") && rows[1].contains("T3"));
        assert_eq!(output.matches("┌─ T1").count(), 1);
        assert_eq!(output.matches("┌─ T4").count(), 1);
    }

    #[test]
    fn unequal_chains_keep_skipped_level_wires_outside_boxes() {
        let cards = cards(&[&[], &[0], &[1], &[], &[3], &[2, 4]]);
        let layout = Layout::new(&cards).unwrap();
        assert_eq!(
            layout
                .vertices
                .iter()
                .filter(|vertex| vertex.card.is_none())
                .count(),
            1
        );
        let output = text(&cards);
        let row = output.lines().find(|line| line.contains("┌─ T3")).unwrap();
        assert_eq!(row.chars().nth(2 * card::STEP + card::CENTER), Some('│'));
        assert_eq!(output.matches("┌─ ").count(), 6);
    }

    #[test]
    fn different_card_heights_continue_each_wire_from_its_own_bottom() {
        let mut cards = cards(&[&[], &[], &[0, 1]]);
        let mut content = vec![];
        card::text(&mut content, "Q: ", &"A long question ".repeat(15));
        card::text(&mut content, "A: ", "Answer");
        cards[0].rows = card::frame("T1", content);
        let short_height = cards[1].rows.len();
        let output = text(&cards);
        assert_eq!(
            output
                .lines()
                .nth(short_height)
                .unwrap()
                .chars()
                .nth(2 * card::STEP + card::CENTER),
            Some('│')
        );
    }

    #[test]
    fn three_parent_blend_and_several_terminal_heads_remain_distinct() {
        let output = text(&cards(&[&[], &[], &[], &[0, 1, 2], &[3], &[3]]));
        let first = output.lines().next().unwrap();
        for label in ["T1", "T2", "T3"] {
            assert!(first.contains(label));
        }
        let final_row = output.lines().rfind(|line| line.contains("┌─ ")).unwrap();
        assert!(final_row.contains("T5") && final_row.contains("T6"));
    }

    #[test]
    fn long_linear_histories_use_one_column_and_no_recursion() {
        let mut cards = cards(&[&[]]);
        for parent in 0..1999 {
            cards.push(Card {
                parents: vec![parent],
                rows: card::frame("Turn", {
                    let mut lines = vec![];
                    card::text(&mut lines, "", "Q: Next?");
                    card::text(&mut lines, "", "A: Yes");
                    lines
                }),
            });
        }
        let output = text(&cards);
        assert_eq!(output.matches("┌─ ").count(), 2000);
        assert!(output.lines().all(|line| line.width() <= card::WIDTH));
    }

    #[test]
    fn non_topological_input_returns_an_error() {
        assert!(matches!(
            Layout::new(&cards(&[&[1], &[]])),
            Err(RenderError::Protocol)
        ));
    }

    #[test]
    fn color_changes_only_highlighted_rows_and_preserves_graph_geometry() {
        let mut first = vec![];
        card::selected(&mut first, "Train");
        let mut second = vec![];
        card::text(&mut second, "", "Other card");
        let cards = vec![
            Card {
                parents: vec![],
                rows: card::frame("First", first),
            },
            Card {
                parents: vec![],
                rows: card::frame("Second", second),
            },
        ];
        let plain = text(&cards);
        let mut styled = vec![];
        write(&cards, &mut styled, true).unwrap();
        let styled = String::from_utf8(styled).unwrap();
        assert_eq!(styled.matches("\u{1b}[1;36;38;5;37m").count(), 1);
        assert_eq!(styled.matches("\u{1b}[0m").count(), 1);
        assert_eq!(
            styled
                .replace("\u{1b}[1;36;38;5;37m", "")
                .replace("\u{1b}[0m", ""),
            plain
        );
        let selected = styled.lines().find(|line| line.contains('▶')).unwrap();
        assert!(selected.find("\u{1b}[0m").unwrap() < selected.find("Other card").unwrap());
    }
}
