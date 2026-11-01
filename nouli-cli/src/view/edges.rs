//! Orthogonal connections between adjacent card layers; crossings never join.
use std::{
    collections::{BTreeSet, HashMap},
    io::{self, Write},
};

#[expect(
    clippy::fn_params_excessive_bools,
    reason = "each flag is one direction of a box-drawing junction"
)]
fn junction(up: bool, down: bool, left: bool, right: bool) -> char {
    match (up, down, left, right) {
        (true, true, true, true) => '┼',
        (true, true, true, false) => '┤',
        (true, true, false, true) => '├',
        (true, false, true, true) => '┴',
        (false, true, true, true) => '┬',
        (true, false, true, false) => '┘',
        (true, false, false, true) => '└',
        (false, true, true, false) => '┐',
        (false, true, false, true) => '┌',
        _ => '│',
    }
}

fn rails(columns: &BTreeSet<usize>, symbol: char) -> Vec<char> {
    let mut line = vec![' '; columns.last().map_or(0, |last| last + 1)];
    for column in columns {
        line[*column] = symbol;
    }
    line
}

fn emit(line: Vec<char>, sink: &mut impl Write) -> io::Result<()> {
    writeln!(sink, "{}", line.into_iter().collect::<String>().trim_end())
}

fn route(
    active: &BTreeSet<usize>,
    from: usize,
    to: usize,
    from_continues: bool,
    to_started: bool,
    sink: &mut impl Write,
) -> io::Result<()> {
    let mut line = rails(active, '│');
    line.resize(line.len().max(from.max(to) + 1), ' ');
    let left = from.min(to);
    let right = from.max(to);
    for cell in &mut line[left + 1..right] {
        *cell = if *cell == '│' { '╳' } else { '─' };
    }
    line[from] = junction(true, from_continues, to < from, to > from);
    line[to] = junction(to_started, true, from < to, from > to);
    emit(line, sink)
}

pub(super) fn write(edges: &[(usize, usize)], sink: &mut impl Write) -> io::Result<()> {
    if edges.is_empty() {
        return Ok(());
    }
    let sources: BTreeSet<_> = edges.iter().map(|edge| edge.0).collect();
    let targets: BTreeSet<_> = edges.iter().map(|edge| edge.1).collect();
    if edges.iter().all(|(source, target)| source == target) {
        emit(rails(&sources, '│'), sink)?;
    } else if sources.len() == 1 || targets.len() == 1 {
        emit(rails(&sources, '│'), sink)?;
        let all: BTreeSet<_> = sources.union(&targets).copied().collect();
        // Nonempty edges guarantee these two bounds.
        let left = edges.iter().flat_map(|(a, b)| [*a, *b]).min().unwrap_or(0);
        let right = edges.iter().flat_map(|(a, b)| [*a, *b]).max().unwrap_or(0);
        let mut line = vec![' '; right + 1];
        line[left..=right].fill('─');
        for column in all {
            line[column] = junction(
                sources.contains(&column),
                targets.contains(&column),
                column > left,
                column < right,
            );
        }
        emit(line, sink)?;
    } else {
        // Separate source and destination phases avoid conflating unrelated
        // wires that happen to use the same column in successive layers.
        let mut occupied: BTreeSet<_> = sources.union(&targets).copied().collect();
        let mut channels = vec![];
        for (source, _) in edges {
            let mut channel = source + 1;
            while occupied.contains(&channel) {
                channel += 1;
            }
            occupied.insert(channel);
            channels.push(channel);
        }
        let mut remaining = HashMap::<usize, usize>::new();
        for (source, _) in edges {
            *remaining.entry(*source).or_default() += 1;
        }
        let mut active = sources;
        for ((source, _), channel) in edges.iter().zip(&channels) {
            let count = remaining.get_mut(source).expect("every source was counted");
            *count -= 1;
            route(&active, *source, *channel, *count > 0, false, sink)?;
            if *count == 0 {
                active.remove(source);
            }
            active.insert(*channel);
        }
        for ((_, target), channel) in edges.iter().zip(channels) {
            route(
                &active,
                channel,
                *target,
                false,
                active.contains(target),
                sink,
            )?;
            active.remove(&channel);
            active.insert(*target);
        }
    }
    emit(rails(&targets, '▼'), sink)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(edges: &[(usize, usize)]) -> String {
        let mut output = vec![];
        write(edges, &mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    // Follow drawn wires downward, treating a crossing as two separate tracks.
    // This verifies connectivity from the output rather than the layout state.
    fn reachable(output: &str, source: usize) -> BTreeSet<usize> {
        let grid: Vec<Vec<char>> = output.lines().map(|line| line.chars().collect()).collect();
        let mut pending = vec![(0, source, 1isize, 0isize)];
        let mut seen = BTreeSet::new();
        let mut targets = BTreeSet::new();
        while let Some((row, column, dr, dc)) = pending.pop() {
            if !seen.insert((row, column, dr, dc)) {
                continue;
            }
            let Some(ch) = grid.get(row).and_then(|line| line.get(column)) else {
                continue;
            };
            if *ch == '▼' {
                targets.insert(column);
                continue;
            }
            let directions: &[(isize, isize)] = match ch {
                '│' => &[(1, 0)],
                '─' | '┴' => &[(0, -1), (0, 1)],
                '└' => &[(0, 1)],
                '┘' => &[(0, -1)],
                '┌' | '├' => &[(1, 0), (0, 1)],
                '┐' | '┤' => &[(1, 0), (0, -1)],
                '┬' | '┼' => &[(1, 0), (0, -1), (0, 1)],
                '╳' => {
                    if let (Some(r), Some(c)) =
                        (row.checked_add_signed(dr), column.checked_add_signed(dc))
                    {
                        pending.push((r, c, dr, dc));
                    }
                    continue;
                }
                _ => continue,
            };
            for (next_dr, next_dc) in directions {
                if (*next_dr, *next_dc) == (-dr, -dc) {
                    continue;
                }
                if let (Some(r), Some(c)) = (
                    row.checked_add_signed(*next_dr),
                    column.checked_add_signed(*next_dc),
                ) {
                    pending.push((r, c, *next_dr, *next_dc));
                }
            }
        }
        targets
    }

    #[test]
    fn independent_straight_wires_and_centered_blends_are_compact() {
        assert_eq!(text(&[(2, 2), (8, 8)]), "  │     │\n  ▼     ▼\n");
        assert_eq!(text(&[(2, 5), (8, 5)]), "  │     │\n  └──┬──┘\n     ▼\n");
    }

    #[test]
    fn rendered_connections_preserve_edges_without_false_joins() {
        let mut cases = vec![
            vec![(2, 8), (12, 8), (12, 18), (22, 18)],
            vec![(2, 12), (12, 2)],
            vec![(2, 12), (2, 22), (12, 12), (22, 2)],
            vec![(2, 12), (12, 12), (22, 12)],
            vec![(12, 2), (12, 12), (12, 22)],
        ];
        // Every nonempty bipartite graph with three ports on each side,
        // including unrelated wires that reuse a column in the next layer.
        for mask in 1..512 {
            let mut edges = vec![];
            for source in 0..3 {
                for target in 0..3 {
                    if mask & (1 << (source * 3 + target)) != 0 {
                        edges.push((source * 10 + 2, target * 10 + 2));
                    }
                }
            }
            cases.push(edges);
        }
        for edges in cases {
            let output = text(&edges);
            for source in edges.iter().map(|edge| edge.0).collect::<BTreeSet<_>>() {
                let expected = edges
                    .iter()
                    .filter(|edge| edge.0 == source)
                    .map(|edge| edge.1)
                    .collect();
                assert_eq!(
                    reachable(&output, source),
                    expected,
                    "source {source}:\n{output}"
                );
            }
        }
    }
}
