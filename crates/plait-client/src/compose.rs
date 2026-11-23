//! Plain presentation decisions for the composer and per-thread controls.

use crate::host::{Model, ThinkingLevel, Usage};

pub fn thinking_levels(model: Option<&Model>) -> &[ThinkingLevel] {
    model
        .filter(|m| m.reasoning == Some(true))
        .and_then(|m| m.thinking_levels.as_deref())
        .unwrap_or_default()
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
}
