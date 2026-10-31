//! Hide internal image references in human text while preserving the saved text.
use nouli_core::identity::Id;
use std::ops::Range;

pub(crate) fn image_tags(value: &str) -> Vec<Range<usize>> {
    tags(value, "[image:")
}

pub(crate) fn decision_tags(value: &str) -> Vec<Range<usize>> {
    tags(value, "[decision:")
}

fn tags(value: &str, prefix: &str) -> Vec<Range<usize>> {
    let mut tags = Vec::new();
    let mut offset = 0;
    while let Some(at) = value[offset..].find(prefix) {
        let start = offset + at;
        let tail = &value[start + prefix.len()..];
        let Some(end) = tail.find(']') else { break };
        offset = start + prefix.len() + end + 1;
        if Id::try_from(tail[..end].to_owned()).is_ok() {
            tags.push(start..offset);
        }
    }
    tags
}

pub(crate) fn image_label_projection(value: &str, cursor: usize) -> (String, usize) {
    let mut shown = String::new();
    let mut offset = 0;
    let mut mapped = None;
    let mut tags = image_tags(value);
    tags.extend(decision_tags(value));
    tags.sort_by_key(|tag| tag.start);
    for mut tag in tags {
        let decision = value[tag.clone()].starts_with("[decision:");
        if tag.start > offset && value.as_bytes()[tag.start - 1] == b' ' {
            tag.start -= 1;
        }
        if mapped.is_none() && cursor <= tag.start {
            mapped = Some(shown.len() + cursor.saturating_sub(offset));
        }
        shown.push_str(&value[offset..tag.start]);
        // Imported images have their filename before the tag. A standalone
        // saved reference still needs a visible attachment placeholder.
        if tag.start == 0 {
            shown.push_str(if decision { "[reference]" } else { "[image]" });
        }
        if mapped.is_none() && cursor < tag.end {
            mapped = Some(shown.len());
        }
        offset = tag.end;
    }
    if mapped.is_none() {
        mapped = Some(shown.len() + cursor.saturating_sub(offset));
    }
    shown.push_str(&value[offset..]);
    (shown, mapped.unwrap_or(0))
}

pub(crate) fn image_labels(value: &str) -> String {
    image_label_projection(value, value.len()).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames_stay_visible_and_cursors_follow_hidden_metadata() {
        let tag = format!("[image:{}]", Id::generate());
        let text = format!("look: 图 \\\"test\\\".png {tag} now");
        let expected = "look: 图 \\\"test\\\".png now";
        let (shown, cursor) = image_label_projection(&text, text.len());
        assert_eq!(shown, expected);
        assert_eq!(cursor, shown.len());
        let before = text.find(&tag).unwrap() - 1;
        assert_eq!(image_label_projection(&text, before).1, before);
        assert_eq!(
            image_label_projection(&text, before + tag.len() + 1).1,
            before
        );
        assert_eq!(image_labels(&tag), "[image]");
        assert_eq!(
            image_labels("literal [image:not-an-id]"),
            "literal [image:not-an-id]"
        );
        assert!(text.contains(&tag), "display never changes the stored text");
        let reference = format!("[decision:{}]", Id::generate());
        let combined = format!("@Where? · trip / 1 {reference} photo.png {tag}");
        assert_eq!(image_labels(&combined), "@Where? · trip / 1 photo.png");
        assert_eq!(image_labels(&reference), "[reference]");
    }
}
