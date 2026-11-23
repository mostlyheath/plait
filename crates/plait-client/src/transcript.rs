use std::{
    collections::{HashMap, HashSet},
    ops::Range,
};

use crate::host::{Message, Snapshot, Tool};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row<'a> {
    Message(&'a Message),
    Streaming(&'a str),
    Reasoning(&'a Message),
    LiveReasoning(&'a str),
    Tool(&'a Tool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Key<'a> {
    Message(&'a str),
    Streaming,
    Reasoning(&'a str),
    LiveReasoning,
    Tool(&'a str),
}

impl<'a> Row<'a> {
    fn key(self) -> Key<'a> {
        match self {
            Self::Message(message) => Key::Message(&message.id),
            Self::Streaming(_) => Key::Streaming,
            Self::Reasoning(message) => Key::Reasoning(&message.id),
            Self::LiveReasoning(_) => Key::LiveReasoning,
            Self::Tool(tool) => Key::Tool(&tool.id),
        }
    }
}

pub fn row_count(snapshot: &Snapshot) -> usize {
    snapshot
        .messages
        .iter()
        .map(|message| {
            usize::from(!message.text.is_empty() || !message.images.is_empty())
                + usize::from(!message.reasoning.is_empty())
        })
        .sum::<usize>()
        + snapshot.tools.len()
        + usize::from(!snapshot.live_text.is_empty())
        + usize::from(!snapshot.live_reasoning.is_empty())
}

/// A row by position in the snapshot, cheap to cache between snapshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Message(usize),
    Streaming,
    Reasoning(usize),
    LiveReasoning,
    Tool(usize),
}

/// Conversation order: each tool follows the message it came after, and live
/// text comes last. Version 1 hosts send no order, so their tools follow the
/// stream as before; a tool whose message was clipped leads the preview.
pub fn layout(snapshot: &Snapshot) -> Vec<Slot> {
    let positions: HashMap<&str, usize> = snapshot
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| (message.id.as_str(), index))
        .collect();
    let mut leading = Vec::new();
    let mut following: Vec<Vec<usize>> = vec![Vec::new(); snapshot.messages.len()];
    let mut unordered = Vec::new();
    for (index, tool) in snapshot.tools.iter().enumerate() {
        match &tool.after {
            None => unordered.push(index),
            Some(after) => match after.as_deref().and_then(|id| positions.get(id)) {
                Some(&message) => following[message].push(index),
                None => leading.push(index),
            },
        }
    }
    let mut slots = Vec::with_capacity(snapshot.messages.len() * 2 + snapshot.tools.len() + 2);
    slots.extend(leading.into_iter().map(Slot::Tool));
    for (message, tools) in following.into_iter().enumerate() {
        if !snapshot.messages[message].reasoning.is_empty() {
            slots.push(Slot::Reasoning(message));
        }
        if !snapshot.messages[message].text.is_empty()
            || !snapshot.messages[message].images.is_empty()
        {
            slots.push(Slot::Message(message));
        }
        slots.extend(tools.into_iter().map(Slot::Tool));
    }
    if !snapshot.live_reasoning.is_empty() {
        slots.push(Slot::LiveReasoning);
    }
    if !snapshot.live_text.is_empty() {
        slots.push(Slot::Streaming);
    }
    slots.extend(unordered.into_iter().map(Slot::Tool));
    slots
}

pub fn resolve(snapshot: &Snapshot, slot: Slot) -> Option<Row<'_>> {
    match slot {
        Slot::Message(index) => snapshot.messages.get(index).map(Row::Message),
        Slot::Streaming => {
            (!snapshot.live_text.is_empty()).then_some(Row::Streaming(&snapshot.live_text))
        }
        Slot::Tool(index) => snapshot.tools.get(index).map(Row::Tool),
        Slot::Reasoning(index) => snapshot.messages.get(index).map(Row::Reasoning),
        Slot::LiveReasoning => (!snapshot.live_reasoning.is_empty())
            .then_some(Row::LiveReasoning(&snapshot.live_reasoning)),
    }
}

pub fn rows(snapshot: &Snapshot) -> Vec<Row<'_>> {
    layout(snapshot)
        .into_iter()
        .filter_map(|slot| resolve(snapshot, slot))
        .collect()
}

/// What a tool acted on, for its one-line label: the command or the path.
/// Inputs are clipped previews, so unparseable JSON has no subject.
pub fn tool_subject(tool: &Tool) -> Option<String> {
    let input: serde_json::Value = serde_json::from_str(&tool.input).ok()?;
    let key = if tool.name == "command" {
        "command"
    } else {
        "path"
    };
    let value = input.get(key)?.as_str()?.trim();
    let line = value.lines().next().unwrap_or_default();
    (!line.is_empty()).then(|| {
        if line.len() < value.len() {
            format!("{line} …")
        } else {
            line.to_owned()
        }
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct Splice {
    // Ranges apply in order, after all earlier splices, not to the original list.
    pub range: Range<usize>,
    pub count: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Update {
    Reset(usize),
    Patch {
        splices: Vec<Splice>,
        remeasure: Vec<Range<usize>>,
    },
}

pub fn changes(previous: Option<&Snapshot>, next: Option<&Snapshot>) -> Update {
    let count = next.map_or(0, row_count);
    let (Some(previous), Some(next)) = (previous, next) else {
        return Update::Reset(count);
    };
    if previous.id != next.id || previous.checkout != next.checkout {
        return Update::Reset(count);
    }
    let old = rows(previous);
    let new = rows(next);
    let positions: HashMap<_, _> = old
        .iter()
        .enumerate()
        .map(|(index, row)| (row.key(), index))
        .collect();
    let keys: HashSet<_> = new.iter().map(|row| row.key()).collect();
    // Ambiguous identity must not preserve measurements or disclosure on the wrong row.
    if positions.len() != old.len() || keys.len() != new.len() {
        return Update::Reset(count);
    }

    let mut splices = Vec::new();
    let mut remeasure: Vec<Range<usize>> = Vec::new();
    let (mut old_start, mut new_start) = (0, 0);
    // Retain ordered stable IDs in linear time, including a clipped history prefix.
    // Reordered rows are removed/reinserted; they never inherit another row's height.
    for (new_index, row) in new.iter().enumerate() {
        let Some(&old_index) = positions
            .get(&row.key())
            .filter(|&&index| index >= old_start)
        else {
            continue;
        };
        splice(
            &mut splices,
            new_start,
            old_index - old_start,
            new_index - new_start,
        );
        if old[old_index] != *row {
            if let Some(range) = remeasure.last_mut().filter(|range| range.end == new_index) {
                range.end += 1;
            } else {
                remeasure.push(new_index..new_index + 1);
            }
        }
        old_start = old_index + 1;
        new_start = new_index + 1;
    }
    splice(
        &mut splices,
        new_start,
        old.len() - old_start,
        new.len() - new_start,
    );
    Update::Patch { splices, remeasure }
}

fn splice(splices: &mut Vec<Splice>, start: usize, removed: usize, inserted: usize) {
    if removed != 0 || inserted != 0 {
        splices.push(Splice {
            range: start..start + removed,
            count: inserted,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{MessageRole, ThreadStatus};

    fn snapshot(ids: &[usize]) -> Snapshot {
        let mut snapshot: Snapshot = serde_json::from_value(serde_json::json!({
            "id":"thread", "checkout":"/repo", "model":{"provider":"p", "model_id":"m"},
            "status":"running", "messages":[], "live_text":"", "tools":[],
            "pending_approval":null, "error":null, "truncated":false
        }))
        .unwrap();
        snapshot.messages = ids
            .iter()
            .map(|id| Message {
                images: Vec::new(),
                id: id.to_string(),
                role: MessageRole::Assistant,
                text: format!("Message {id}"),
                reasoning: String::new(),
            })
            .collect();
        snapshot
    }

    fn assert_applies(old: &Snapshot, new: &Snapshot) {
        let mut result = rows(old);
        let expected = rows(new);
        match changes(Some(old), Some(new)) {
            Update::Reset(count) => assert_eq!(count, expected.len()),
            Update::Patch { splices, remeasure } => {
                for splice in splices {
                    let replacement = expected
                        [splice.range.start..splice.range.start + splice.count]
                        .iter()
                        .copied();
                    result.splice(splice.range, replacement);
                }
                for range in remeasure {
                    result[range.clone()].copy_from_slice(&expected[range]);
                }
                assert_eq!(result, expected);
            }
        }
    }

    #[test]
    fn streaming_changes_only_one_row_and_status_changes_none() {
        let mut old = snapshot(&(0..2000).collect::<Vec<_>>());
        old.live_text = "First".into();
        let mut new = old.clone();
        new.live_text.push_str(" word");
        assert_eq!(
            changes(Some(&old), Some(&new)),
            Update::Patch {
                splices: vec![],
                remeasure: std::iter::once(2000..2001).collect()
            }
        );
        new = old.clone();
        new.status = ThreadStatus::Stopped;
        assert_eq!(
            changes(Some(&old), Some(&new)),
            Update::Patch {
                splices: vec![],
                remeasure: vec![]
            }
        );
    }

    #[test]
    fn clipping_and_appending_keep_the_overlapping_history() {
        let old = snapshot(&[0, 1, 2]);
        let new = snapshot(&[1, 2, 3]);
        assert_eq!(
            changes(Some(&old), Some(&new)),
            Update::Patch {
                splices: vec![
                    Splice {
                        range: 0..1,
                        count: 0
                    },
                    Splice {
                        range: 2..2,
                        count: 1
                    }
                ],
                remeasure: vec![],
            }
        );
        assert_applies(&old, &new);
    }

    #[test]
    fn messages_stream_and_tools_have_distinct_stable_identities() {
        let mut old = snapshot(&[1]);
        old.live_text = "partial".into();
        old.tools.push(serde_json::from_value(serde_json::json!({"id":"1","name":"read","status":"running","input":"file","output":""})).unwrap());
        let mut new = old.clone();
        new.live_text.clear();
        new.messages.push(snapshot(&[2]).messages.remove(0));
        new.tools[0].output = "done".into();
        assert_applies(&old, &new);
        let legacy = rows(&old);
        assert!(matches!(legacy[1], Row::Streaming(_)));
        assert!(matches!(legacy[2], Row::Tool(_)));
        assert_eq!(legacy.len(), 3);
        assert_eq!(resolve(&old, Slot::Tool(usize::MAX)), None);
    }

    fn tool(id: &str, after: serde_json::Value, input: serde_json::Value) -> Tool {
        let mut value = serde_json::json!({
            "id": id, "name": "command", "status": "done", "input": input.to_string(), "output": ""
        });
        if !after.is_null() || id.starts_with("null") {
            value["after"] = after;
        }
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn ordered_tools_follow_their_message_and_stream_comes_last() {
        let mut snapshot = snapshot(&[1, 2]);
        snapshot.live_text = "partial".into();
        snapshot.tools = vec![
            tool("a", "1".into(), serde_json::json!({})),
            tool("null-b", serde_json::Value::Null, serde_json::json!({})),
            tool("c", "2".into(), serde_json::json!({})),
            tool("d", "clipped".into(), serde_json::json!({})),
            tool("e", "1".into(), serde_json::json!({})),
        ];
        let order: Vec<_> = rows(&snapshot)
            .into_iter()
            .map(|row| match row {
                Row::Message(message) => format!("m{}", message.id),
                Row::Streaming(_) => "live".into(),
                Row::Reasoning(_) | Row::LiveReasoning(_) => "reasoning".into(),
                Row::Tool(tool) => tool.id.clone(),
            })
            .collect();
        assert_eq!(order, ["null-b", "d", "m1", "a", "e", "m2", "c", "live"]);
        assert_eq!(layout(&snapshot).len(), row_count(&snapshot));
        let mut next = snapshot.clone();
        next.messages.push(super::super::host::Message {
            images: Vec::new(),
            id: "3".into(),
            role: MessageRole::Assistant,
            text: "Done.".into(),
            reasoning: String::new(),
        });
        next.tools
            .push(tool("f", "3".into(), serde_json::json!({})));
        next.live_text.clear();
        assert_applies(&snapshot, &next);
    }

    #[test]
    fn tool_subject_names_the_command_or_path() {
        let command = tool(
            "1",
            "1".into(),
            serde_json::json!({"command": "cargo test\n--locked"}),
        );
        assert_eq!(tool_subject(&command).as_deref(), Some("cargo test …"));
        let mut edit = tool(
            "2",
            "1".into(),
            serde_json::json!({"path": " src/main.rs "}),
        );
        edit.name = "edit".into();
        assert_eq!(tool_subject(&edit).as_deref(), Some("src/main.rs"));
        edit.input = "{\"path\": \"clipped".into();
        assert_eq!(tool_subject(&edit), None);
        edit.input = serde_json::json!({"command": "x"}).to_string();
        assert_eq!(tool_subject(&edit), None);
    }

    #[test]
    fn identity_changes_and_duplicate_ids_reset_measurements() {
        let old = snapshot(&[1, 2]);
        let mut new = old.clone();
        new.id = "other".into();
        assert_eq!(changes(Some(&old), Some(&new)), Update::Reset(2));
        assert_eq!(changes(None, Some(&old)), Update::Reset(2));
        assert_eq!(changes(Some(&old), None), Update::Reset(0));
        assert_eq!(
            changes(Some(&old), Some(&snapshot(&[1, 1]))),
            Update::Reset(2)
        );
        assert_eq!(
            changes(Some(&snapshot(&[1, 1])), Some(&old)),
            Update::Reset(2)
        );
    }

    #[test]
    fn arbitrary_insert_delete_reorder_and_content_changes_apply_correctly() {
        let mut seed = 7u64;
        let mut random = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (seed >> 32) as usize
        };
        for _ in 0..2000 {
            let mut ids: Vec<_> = (0..20).collect();
            for index in (1..ids.len()).rev() {
                ids.swap(index, random() % (index + 1));
            }
            let old = snapshot(&ids[..random() % 21]);
            for index in (1..ids.len()).rev() {
                ids.swap(index, random() % (index + 1));
            }
            let mut new = snapshot(&ids[..random() % 21]);
            for message in &mut new.messages {
                if random() % 3 == 0 {
                    message.text.push_str(" changed");
                }
            }
            assert_applies(&old, &new);
        }
    }
}
