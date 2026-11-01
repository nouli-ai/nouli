//! Inline `[image:UUID]` and `[decision:UUID]` markers that bind text to
//! saved image and saved-answer evidence.
//!
//! Stored text keeps every marker; human projections hide them. Only a marker
//! whose body is a valid ID counts, so literal brackets such as
//! `[image:not-an-id]` stay ordinary text.
use std::ops::Range;

use crate::identity::Id;

/// The opening of an inline image marker.
pub const IMAGE_MARKER_PREFIX: &str = "[image:";
/// The opening of an inline saved-answer marker.
pub const DECISION_MARKER_PREFIX: &str = "[decision:";
const MARKER_END: char = ']';

/// What an inline marker refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarkerKind {
    /// A saved image record.
    Image,
    /// A saved-answer reference record.
    Decision,
}

impl MarkerKind {
    /// The text that opens this kind of marker.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Image => IMAGE_MARKER_PREFIX,
            Self::Decision => DECISION_MARKER_PREFIX,
        }
    }

    /// The byte ranges of every valid marker of this kind, in text order.
    #[must_use]
    pub fn ranges(self, text: &str) -> Vec<Range<usize>> {
        let prefix = self.prefix();
        let mut ranges = Vec::new();
        let mut offset = 0;
        while let Some(at) = text[offset..].find(prefix) {
            let start = offset + at;
            let body = &text[start + prefix.len()..];
            let Some(end) = body.find(MARKER_END) else {
                break;
            };
            offset = start + prefix.len() + end + MARKER_END.len_utf8();
            if Id::try_from(body[..end].to_owned()).is_ok() {
                ranges.push(start..offset);
            }
        }
        ranges
    }

    /// The marker text for `id`.
    #[must_use]
    pub fn marker(self, id: impl std::fmt::Display) -> String {
        format!("{}{id}{MARKER_END}", self.prefix())
    }
}

/// One valid inline marker found in text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub kind: MarkerKind,
    /// The byte range of the whole marker, brackets included.
    pub range: Range<usize>,
}

/// Every valid image and saved-answer marker in `text`, ordered by position.
#[must_use]
pub fn marker_ranges(text: &str) -> Vec<Marker> {
    let mut markers = [MarkerKind::Image, MarkerKind::Decision]
        .into_iter()
        .flat_map(|kind| {
            kind.ranges(text)
                .into_iter()
                .map(move |range| Marker { kind, range })
        })
        .collect::<Vec<_>>();
    markers.sort_by_key(|marker| marker.range.start);
    markers
}

/// The marker text for an image identified by `id`.
#[must_use]
pub fn image_marker(id: impl std::fmt::Display) -> String {
    MarkerKind::Image.marker(id)
}

/// The marker text for a saved-answer reference identified by `id`.
#[must_use]
pub fn decision_marker(id: impl std::fmt::Display) -> String {
    MarkerKind::Decision.marker(id)
}

/// The kind and ID of `text` when it is exactly one valid marker.
#[must_use]
pub fn parse_marker(text: &str) -> Option<(MarkerKind, Id)> {
    let [marker] = marker_ranges(text).try_into().ok()?;
    if marker.range != (0..text.len()) {
        return None;
    }
    let body = &text[marker.kind.prefix().len()..text.len() - MARKER_END.len_utf8()];
    Id::try_from(body.to_owned())
        .ok()
        .map(|id| (marker.kind, id))
}

/// `text` without its markers, also removing the single space before each
/// one. Literal brackets and invalid markers are kept.
#[must_use]
pub fn strip_markers(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    let mut offset = 0;
    for Marker { mut range, .. } in marker_ranges(text) {
        if range.start > offset && text.as_bytes()[range.start - 1] == b' ' {
            range.start -= 1;
        }
        shown.push_str(&text[offset..range.start]);
        offset = range.end;
    }
    shown.push_str(&text[offset..]);
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_finds_only_valid_markers_in_text_order() {
        let image = image_marker(Id::generate());
        let decision = decision_marker(Id::generate());
        let text = format!("@Where? {decision} photo.png {image} [image:not-an-id] tail");
        let markers = marker_ranges(&text);
        assert_eq!(
            markers.iter().map(|m| m.kind).collect::<Vec<_>>(),
            [MarkerKind::Decision, MarkerKind::Image]
        );
        assert_eq!(&text[markers[0].range.clone()], decision);
        assert_eq!(&text[markers[1].range.clone()], image);
        assert_eq!(MarkerKind::Image.ranges(&text), [markers[1].range.clone()]);
        for text in ["literal [image:not-an-id]", "unterminated [image:", ""] {
            assert_eq!(marker_ranges(text), []);
        }
    }

    #[test]
    fn a_marker_inside_an_invalid_one_is_still_found() {
        let decision = decision_marker(Id::generate());
        let text = format!("[image:{decision}]");
        let markers = marker_ranges(&text);
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].kind, MarkerKind::Decision);
        assert_eq!(&text[markers[0].range.clone()], decision);
    }

    #[test]
    fn markers_format_and_parse_round_trip() {
        let id = Id::generate();
        assert_eq!(image_marker(id), format!("[image:{id}]"));
        assert_eq!(decision_marker(id), format!("[decision:{id}]"));
        assert_eq!(
            parse_marker(&decision_marker(id)),
            Some((MarkerKind::Decision, id))
        );
        assert_eq!(
            parse_marker(&image_marker(id)),
            Some((MarkerKind::Image, id))
        );
        assert_eq!(parse_marker(&format!(" {}", image_marker(id))), None);
        assert_eq!(parse_marker("[image:not-an-id]"), None);
    }

    #[test]
    fn stripping_hides_markers_and_their_leading_space_only() {
        let image = image_marker(Id::generate());
        let decision = decision_marker(Id::generate());
        assert_eq!(
            strip_markers(&format!("look: 图.png {image} now")),
            "look: 图.png now"
        );
        assert_eq!(
            strip_markers(&format!("{decision} photo.png {image}")),
            " photo.png"
        );
        assert_eq!(strip_markers(&format!("a {image}{decision}")), "a");
        assert_eq!(
            strip_markers("keep [image:not-an-id] text"),
            "keep [image:not-an-id] text"
        );
    }
}
