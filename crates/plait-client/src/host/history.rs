//! A thread's complete conversation, paged backward. Needs a version 3 host.

use super::{Client, Error, Request};

pub use plait_protocol::host::history::*;

impl Client {
    /// The rest of a large field, after the part its item already carries.
    /// Blocking: one request for each 65,536 UTF-16 units.
    pub fn history_rest(&self, thread_id: &str, more: &More) -> Result<String, Error> {
        let mut rest = String::new();
        let mut offset = Some(more.offset);
        while let Some(current) = offset {
            let part: HistoryText = self.request(&Request::history_text(
                thread_id.to_owned(),
                more.reference.clone(),
                current,
            ))?;
            offset = next_offset(&part, current, more.length)?;
            rest.push_str(&part.text);
        }
        Ok(rest)
    }
}

// A host that repeats or overruns a part would otherwise loop or grow without bound.
fn next_offset(part: &HistoryText, asked: u64, length: u64) -> Result<Option<u64>, Error> {
    let end = asked + part.text.encode_utf16().count() as u64;
    let consistent = part.offset == asked
        && part.length == length
        && end <= length
        && match part.next_offset {
            Some(next) => next == end && next > asked,
            None => end == length,
        };
    if consistent {
        Ok(part.next_offset)
    } else {
        Err(Error::Protocol(
            "Host returned inconsistent history text.".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_pages_decode_items_and_reject_unknown_fields() {
        let page: HistoryPage = serde_json::from_value(serde_json::json!({
            "items": [
                {"type":"message","id":"request-a","role":"user","text":"Hello","reasoning":""},
                {"type":"message","id":"2-0","role":"assistant","text":"","reasoning":"Think",
                 "more":[{"field":"reasoning","ref":"e2.0.reasoning","offset":5,"length":70000}]},
                {"type":"tool","id":"call","name":"read","status":"done","input":"{}","output":"ok"},
            ],
            "before": "1:0:0",
        }))
        .unwrap();
        assert_eq!(page.before.as_deref(), Some("1:0:0"));
        assert_eq!(
            page.items.iter().map(HistoryItem::id).collect::<Vec<_>>(),
            ["request-a", "2-0", "call"]
        );
        assert!(page.items[0].more().is_empty());
        assert_eq!(page.items[1].more()[0].field, HistoryField::Reasoning);
        assert_eq!(page.items[1].more()[0].reference, "e2.0.reasoning");
        for invalid in [
            serde_json::json!({"items":[{"type":"note","id":"x"}],"before":null}),
            serde_json::json!({"items":[{"type":"tool","id":"x","name":"n","status":"done","input":"","output":"","extra":1}],"before":null}),
            serde_json::json!({"items":[],"before":null,"extra":true}),
        ] {
            assert!(serde_json::from_value::<HistoryPage>(invalid).is_err());
        }
    }

    #[test]
    fn history_text_parts_must_advance_and_end_at_the_declared_length() {
        let part = |text: &str, offset, next_offset, length| HistoryText {
            text: text.into(),
            offset,
            next_offset,
            length,
        };
        // Offsets count UTF-16 units: the emoji is two.
        assert_eq!(
            next_offset(&part("a🙂", 4, Some(7), 9), 4, 9).unwrap(),
            Some(7)
        );
        assert_eq!(next_offset(&part("bc", 7, None, 9), 7, 9).unwrap(), None);
        for bad in [
            part("", 4, Some(4), 9),
            part("ab", 4, None, 9),
            part("ab", 5, Some(7), 9),
            part("ab", 4, Some(6), 8),
            part("abcdefgh", 4, None, 9),
        ] {
            assert!(next_offset(&bad, 4, 9).is_err());
        }
    }
}
