//! Official strict TOON parsing, with a lexical numeric-domain guard.

use serde_json::Value;

use super::CodecError;

/// Return delimiters outside quoted strings and header groups. Escaping and
/// structure are validated by the official parser, not by this numeric guard.
fn separators(input: &str, delimiter: char, header_groups: bool) -> Vec<usize> {
    let mut quoted = false;
    let mut escaped = false;
    let mut in_length = false;
    let mut length_closed = false;
    let mut field_depth = 0_u32;
    let mut indices = Vec::new();
    for (index, c) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if quoted && c == '\\' {
            escaped = true;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
            continue;
        }
        if quoted {
            continue;
        }
        if header_groups {
            match c {
                '[' if !length_closed => in_length = true,
                ']' if in_length => {
                    in_length = false;
                    length_closed = true;
                }
                '{' if length_closed => field_depth += 1,
                '}' if field_depth > 0 => field_depth -= 1,
                _ => {}
            }
        }
        if c == delimiter && !in_length && field_depth == 0 {
            indices.push(index);
        }
    }
    indices
}

fn numeric(token: &str) -> Result<(), CodecError> {
    let token = token.trim();
    if token.starts_with('"') {
        return Ok(());
    }
    if matches!(token, "NaN" | "Infinity" | "-Infinity" | "+Infinity") {
        return Err(CodecError::Number);
    }
    if !toon_format::utils::is_numeric_like(token) {
        return Ok(());
    }
    let number = token.parse::<f64>().map_err(|_| CodecError::Number)?;
    if !number.is_finite() {
        return Err(CodecError::Number);
    }
    if !token.contains(['.', 'e', 'E'])
        && token.parse::<i64>().is_err()
        && token.parse::<u64>().is_err()
    {
        // The official encoder emits large finite f64 values as canonical
        // decimals. Accept that exact spelling, never a rounded nearby integer.
        let n = toon_format::types::Number::from_f64(number).ok_or(CodecError::Number)?;
        if toon_format::utils::format_canonical_number(&n) != token {
            return Err(CodecError::Number);
        }
    }
    Ok(())
}

fn cells(input: &str, delimiter: char) -> Result<(), CodecError> {
    let mut start = 0;
    for end in separators(input, delimiter, false) {
        numeric(&input[start..end])?;
        start = end + delimiter.len_utf8();
    }
    numeric(&input[start..])
}

fn check_numbers(input: &str) -> Result<(), CodecError> {
    // Track only tabular row spans. Ordinary scalar text, including commas, is
    // one value. This guard never constructs values or replaces source text.
    let mut tables: Vec<(usize, char, bool)> = Vec::new();
    for raw in input.lines() {
        let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let indent = raw.len() - raw.trim_start_matches(' ').len();
        let content = raw.trim();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        while tables.last().is_some_and(|(level, _, _)| indent <= *level) {
            tables.pop();
        }
        if let Some((_, delimiter, keyed)) = tables.last() {
            let values = if *keyed {
                let colon = separators(content, ':', false)
                    .first()
                    .copied()
                    .ok_or(CodecError::Syntax)?;
                &content[colon + 1..]
            } else {
                content
            };
            cells(values, *delimiter)?;
            continue;
        }
        let content = content.strip_prefix("- ").unwrap_or(content);
        if let Some(colon) = separators(content, ':', true).first().copied() {
            let header = &content[..colon];
            let values = &content[colon + 1..];
            // A quoted key can contain brackets without declaring an array.
            let mut quoted = false;
            let mut escaped = false;
            let mut bracket = None;
            for (i, c) in header.char_indices() {
                if escaped {
                    escaped = false;
                    continue;
                }
                if quoted && c == '\\' {
                    escaped = true;
                    continue;
                }
                if c == '"' {
                    quoted = !quoted;
                }
                if !quoted && c == '[' {
                    bracket = Some(i);
                    break;
                }
            }
            if let Some(start) = bracket {
                let end = header[start..]
                    .find(']')
                    .map(|n| start + n)
                    .ok_or(CodecError::Syntax)?;
                let declaration = &header[start + 1..end];
                let delimiter = if declaration.contains('|') {
                    '|'
                } else if declaration.contains('\t') {
                    '\t'
                } else {
                    ','
                };
                if header[end + 1..].trim_start().starts_with('{') {
                    tables.push((indent, delimiter, declaration.contains(':')));
                } else {
                    cells(values, delimiter)?;
                }
            } else {
                numeric(values)?;
            }
        } else {
            numeric(content)?;
        }
    }
    Ok(())
}

pub(super) fn decode(input: &str) -> Result<Value, CodecError> {
    // Parse first so malformed indentation/counts/duplicate fields cannot be
    // hidden by lexical-domain checks. Never retain parser source-line errors.
    let value = toon_format::decode_strict(input).map_err(|error| match error {
        toon_format::ToonError::ParseError { message, .. } if message.starts_with("Duplicate") => {
            CodecError::DuplicateKey
        }
        _ => CodecError::Syntax,
    })?;
    check_numbers(input)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_guard_preserves_quoted_and_non_numeric_scalar_text() {
        for input in [
            "text: hello,18446744073709551616",
            "text: \"18446744073709551616\"",
            "text: 123 and more",
            "rows[1|]{a|b}:\n  hello,18446744073709551616|fine",
        ] {
            assert!(decode(input).is_ok(), "{input}");
        }
        for input in [
            "values[1]: 18446744073709551616",
            "rows[1]{a}:\n  18446744073709551616",
            "x: NaN",
        ] {
            assert_eq!(decode(input), Err(CodecError::Number));
        }
    }

    #[test]
    fn brackets_in_text_cells_cannot_hide_unsupported_following_numbers() {
        for bracket in ['[', '{'] {
            for number in ["18446744073709551617", "NaN", "1e999"] {
                for input in [
                    format!("values[2]: hello{bracket},{number}"),
                    format!("rows[1]{{a,b}}:\n  hello{bracket},{number}"),
                ] {
                    assert_eq!(decode(&input), Err(CodecError::Number), "{input}");
                }
            }
            assert!(decode(&format!("values[2]: hello{bracket},ordinary text")).is_ok());
        }
    }

    #[test]
    fn ordinary_braces_in_keys_do_not_hide_numeric_values() {
        for input in ["a{: NaN", "a{: 18446744073709551617"] {
            assert_eq!(decode(input), Err(CodecError::Number));
        }
        assert_eq!(
            decode("a{: ordinary text").unwrap(),
            serde_json::json!({"a{":"ordinary text"})
        );
    }
}
