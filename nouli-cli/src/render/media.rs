//! Hide internal image references in human text while preserving the saved text.
use nouli_core::context::evidence::marker::{self, MarkerKind};

pub(crate) fn image_label_projection(value: &str, cursor: usize) -> (String, usize) {
    let mut shown = String::new();
    let mut offset = 0;
    let mut mapped = None;
    for marker::Marker {
        kind,
        range: mut tag,
    } in marker::marker_ranges(value)
    {
        let decision = kind == MarkerKind::Decision;
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
    use nouli_core::identity::Id;

    #[test]
    fn filenames_stay_visible_and_cursors_follow_hidden_metadata() {
        let tag = marker::image_marker(Id::generate());
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
        let reference = marker::decision_marker(Id::generate());
        let combined = format!("@Where? · trip / 1 {reference} photo.png {tag}");
        assert_eq!(image_labels(&combined), "@Where? · trip / 1 photo.png");
        assert_eq!(image_labels(&reference), "[reference]");
    }
}
