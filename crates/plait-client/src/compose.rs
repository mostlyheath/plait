//! Plain presentation decisions for the composer and per-thread controls.

use crate::host::{ApprovalMode, Model, ThinkingLevel, Usage};

pub fn thinking_levels(model: Option<&Model>) -> &[ThinkingLevel] {
    model
        .filter(|m| m.reasoning == Some(true))
        .and_then(|m| m.thinking_levels.as_deref())
        .unwrap_or_default()
}

/// What an effort / thinking control shows: just the level, since a bare "Off"
/// beside the model would not say what is off.
pub fn thinking_value(level: ThinkingLevel) -> &'static str {
    if level == ThinkingLevel::Off {
        "No thinking"
    } else {
        level.label()
    }
}

/// What the run-settings button and menu call an approval mode.
pub fn approval_label(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "Ask before actions",
        ApprovalMode::Auto => "Auto-approve actions",
    }
}

/// Compact value shown on the draft tab's Pi permissions control.
pub fn draft_approval_value(mode: ApprovalMode) -> &'static str {
    match mode {
        ApprovalMode::Ask => "Ask",
        ApprovalMode::Auto => "Auto",
    }
}

/// Hover text for a Pi thread's approval mode control.
pub fn approval_tip(is_thread_override: bool) -> &'static str {
    if is_thread_override {
        "Chosen for this thread. A change applies from your next message. Whether commands are sandboxed is a project setting."
    } else {
        "The default from Settings → New threads. A change applies from your next message. Whether commands are sandboxed is a project setting."
    }
}

/// Hover text for a draft tab's thinking control.
pub fn draft_thinking_tip(level: ThinkingLevel, is_override: bool) -> String {
    let source = if is_override {
        "Chosen for this thread"
    } else {
        "The default from Settings → New threads"
    };
    format!(
        "Thinking: {}. {source}, at the nearest level this model has.",
        level.label()
    )
}

/// Hover text for a draft tab's Pi permissions control.
pub fn draft_approval_tip(mode: ApprovalMode, is_override: bool) -> String {
    let source = if is_override {
        "Chosen for this thread."
    } else {
        "The default from Settings → New threads."
    };
    format!("{}. {source}", approval_label(mode))
}

/// Why a draft tab's Pi permissions control is blocked, if it is.
pub fn draft_approval_blocked(
    locked: Option<&'static str>,
    connected: bool,
    protocol: u32,
) -> Option<&'static str> {
    locked.or_else(|| {
        (connected && protocol < 2).then_some("This host asks before every edit and command.")
    })
}

/// Why a thread's model and thinking controls are locked right now.
pub fn configure_reason(
    protocol: u32,
    connected: bool,
    has_snapshot: bool,
    busy: bool,
    pending: bool,
) -> Option<&'static str> {
    if protocol < 3 {
        Some("Update the host to change a thread's model.")
    } else if !connected {
        Some("Reconnect to change this thread.")
    } else if !has_snapshot {
        Some("Loading thread…")
    } else if busy {
        Some("Available after the current run ends.")
    } else if pending {
        Some("Saving thread settings…")
    } else {
        None
    }
}

/// Why a harness thread's permissions control cannot change right now.
/// A run does not stop it.
pub fn mode_reason(
    protocol: u32,
    connected: bool,
    has_snapshot: bool,
    pending: bool,
) -> Option<&'static str> {
    if !connected {
        Some("Reconnect to change this thread.")
    } else if protocol < 4 {
        Some("Update the host to change this thread's permissions.")
    } else if !has_snapshot {
        Some("Loading thread…")
    } else if pending {
        Some("Saving permissions…")
    } else {
        None
    }
}

/// What to say when a Pi run is under one approval mode and the control
/// names another. `None` when they agree or the host does not report the
/// run's mode.
pub fn run_mode_note(running: Option<ApprovalMode>, chosen: ApprovalMode) -> Option<String> {
    let running = running.filter(|running| *running != chosen)?;
    let now = match running {
        ApprovalMode::Ask => "asks before actions",
        ApprovalMode::Auto => "approves actions automatically",
    };
    Some(format!(
        "This run {now}. {} applies from your next message.",
        approval_label(chosen)
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextPressure {
    Normal,
    High,
    NearlyFull,
}

pub struct ContextUsage {
    pub percent: u64,
    pub pressure: ContextPressure,
    pub label: String,
    pub detail: String,
}

impl ContextUsage {
    pub fn new(usage: &Usage, model: Option<&Model>) -> Option<Self> {
        let used = usage.context_used?;
        let window = usage
            .context_window
            .or_else(|| model.and_then(|m| m.context_window))?;
        if window == 0 {
            return None;
        }
        let percent = ((u128::from(used) * 100) / u128::from(window)).min(100) as u64;
        let pressure = if percent >= 90 {
            ContextPressure::NearlyFull
        } else if percent >= 75 {
            ContextPressure::High
        } else {
            ContextPressure::Normal
        };
        let state = match pressure {
            ContextPressure::Normal => "context",
            ContextPressure::High => "context high",
            ContextPressure::NearlyFull => "context nearly full",
        };
        let reasoning = usage
            .reasoning
            .map_or_else(|| "not reported".into(), |n| n.to_string());
        Some(Self {
            percent,
            pressure,
            label: format!("{percent}% {state}"),
            detail: format!(
                "Context {used} / {window} tokens. Latest response: input {}, output {}, cache read {}, cache write {}, reasoning {reasoning}. Usage is not cumulative.",
                usage.input, usage.output, usage.cache_read, usage.cache_write
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn usage(used: u64, window: u64) -> Usage {
        Usage {
            input: 100,
            output: 20,
            cache_read: 30,
            cache_write: 0,
            reasoning: None,
            total: 150,
            context_used: Some(used),
            context_window: Some(window),
        }
    }
    #[test]
    fn context_thresholds_are_worded_and_overflow_safe() {
        for (used, pressure) in [
            (74, ContextPressure::Normal),
            (75, ContextPressure::High),
            (89, ContextPressure::High),
            (90, ContextPressure::NearlyFull),
            (200, ContextPressure::NearlyFull),
        ] {
            let value = ContextUsage::new(&usage(used, 100), None).unwrap();
            assert_eq!(value.pressure, pressure);
            assert!(value.detail.contains("not cumulative"));
        }
        assert_eq!(
            ContextUsage::new(&usage(u64::MAX, u64::MAX), None)
                .unwrap()
                .percent,
            100
        );
        assert!(ContextUsage::new(&usage(1, 0), None).is_none());
        let mut value = usage(50, 100);
        value.context_used = None;
        assert!(ContextUsage::new(&value, None).is_none());
    }
    #[test]
    fn thinking_levels_come_only_from_the_reasoning_model() {
        let mut model = Model {
            reasoning: Some(true),
            thinking_levels: Some(vec![
                ThinkingLevel::Off,
                ThinkingLevel::Low,
                ThinkingLevel::Max,
            ]),
            ..Default::default()
        };
        assert_eq!(
            thinking_levels(Some(&model)),
            &[ThinkingLevel::Off, ThinkingLevel::Low, ThinkingLevel::Max]
        );
        model.reasoning = Some(false);
        assert!(thinking_levels(Some(&model)).is_empty());
        assert!(thinking_levels(None).is_empty());
    }

    #[test]
    fn thinking_and_approval_labels_match_design_wording() {
        assert_eq!(thinking_value(ThinkingLevel::Off), "No thinking");
        assert_eq!(thinking_value(ThinkingLevel::Low), "Low");
        assert_eq!(thinking_value(ThinkingLevel::High), "High");
        assert_eq!(approval_label(ApprovalMode::Ask), "Ask before actions");
        assert_eq!(approval_label(ApprovalMode::Auto), "Auto-approve actions");
        assert_eq!(draft_approval_value(ApprovalMode::Ask), "Ask");
        assert_eq!(draft_approval_value(ApprovalMode::Auto), "Auto");
        assert!(approval_tip(true).starts_with("Chosen for this thread."));
        assert!(approval_tip(false).starts_with("The default from Settings → New threads."));
        assert!(draft_thinking_tip(ThinkingLevel::Medium, true).contains("Chosen for this thread"));
        assert!(
            draft_thinking_tip(ThinkingLevel::Medium, false)
                .contains("The default from Settings → New threads")
        );
        assert_eq!(
            draft_approval_tip(ApprovalMode::Auto, true),
            "Auto-approve actions. Chosen for this thread."
        );
        assert_eq!(
            draft_approval_tip(ApprovalMode::Ask, false),
            "Ask before actions. The default from Settings → New threads."
        );
    }

    #[test]
    fn lock_reasons_follow_priority_order() {
        assert_eq!(
            configure_reason(2, false, false, true, true),
            Some("Update the host to change a thread's model.")
        );
        assert_eq!(
            configure_reason(3, false, false, true, true),
            Some("Reconnect to change this thread.")
        );
        assert_eq!(
            configure_reason(3, true, false, true, true),
            Some("Loading thread…")
        );
        assert_eq!(
            configure_reason(3, true, true, true, true),
            Some("Available after the current run ends.")
        );
        assert_eq!(
            configure_reason(3, true, true, false, true),
            Some("Saving thread settings…")
        );
        assert_eq!(configure_reason(3, true, true, false, false), None);

        assert_eq!(
            mode_reason(3, false, false, true),
            Some("Reconnect to change this thread.")
        );
        assert_eq!(
            mode_reason(3, true, false, true),
            Some("Update the host to change this thread's permissions.")
        );
        assert_eq!(mode_reason(4, true, false, true), Some("Loading thread…"));
        assert_eq!(
            mode_reason(4, true, true, true),
            Some("Saving permissions…")
        );
        assert_eq!(mode_reason(4, true, true, false), None);

        assert_eq!(
            draft_approval_blocked(Some("Creating thread…"), true, 1),
            Some("Creating thread…")
        );
        assert_eq!(
            draft_approval_blocked(None, true, 1),
            Some("This host asks before every edit and command.")
        );
        assert_eq!(draft_approval_blocked(None, false, 1), None);
        assert_eq!(draft_approval_blocked(None, true, 2), None);
    }

    #[test]
    fn a_run_under_another_approval_mode_is_said_in_the_open() {
        assert_eq!(
            run_mode_note(Some(ApprovalMode::Ask), ApprovalMode::Ask),
            None
        );
        assert_eq!(run_mode_note(None, ApprovalMode::Auto), None);
        assert_eq!(
            run_mode_note(Some(ApprovalMode::Auto), ApprovalMode::Ask).as_deref(),
            Some(
                "This run approves actions automatically. Ask before actions applies from your next message."
            )
        );
        assert_eq!(
            run_mode_note(Some(ApprovalMode::Ask), ApprovalMode::Auto).as_deref(),
            Some(
                "This run asks before actions. Auto-approve actions applies from your next message."
            )
        );
    }
}
