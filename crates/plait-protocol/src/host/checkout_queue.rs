use serde::Deserialize;

fn bounded_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Waiting,
    Held,
    Started,
    Cancelled,
    Failed,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub request_id: String,
    pub state: State,
    pub position: u32,
    pub count: u32,
    detail: String,
}
impl Summary {
    pub fn valid(&self) -> bool {
        bounded_id(&self.request_id)
            && matches!(self.state, State::Waiting | State::Held)
            && (1..=32).contains(&self.position)
            && (1..=32).contains(&self.count)
    }
    pub fn label(&self) -> &'static str {
        if self.state == State::Held {
            "Held"
        } else {
            "Waiting"
        }
    }
    pub fn new(request_id: String, state: State, position: u32, count: u32) -> Self {
        let detail = format!(
            "{position} in line for this checkout. {}",
            if state == State::Held {
                "Resume releases this checkout's held Wait messages in order; interrupted runs are not replayed."
            } else {
                "Starts after earlier messages and the checkout's current writer finish."
            }
        );
        Self {
            request_id,
            state,
            position,
            count,
            detail,
        }
    }
    pub fn detail(&self) -> &str {
        &self.detail
    }
}
impl<'de> Deserialize<'de> for Summary {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            request_id: String,
            state: State,
            position: u32,
            count: u32,
        }
        let wire = Wire::deserialize(deserializer)?;
        let summary = Self::new(wire.request_id, wire.state, wire.position, wire.count);
        if !summary.valid() {
            return Err(serde::de::Error::custom("Invalid checkout queue summary"));
        }
        Ok(summary)
    }
}
