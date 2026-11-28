//! Host wire data and serialization.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
pub mod agent;
pub mod automations;
pub mod build_cache;
pub mod checkout_queue;
pub mod folders;
pub mod git;
pub mod graph;
pub mod harness;
pub mod history;
pub mod linear;
pub mod providers;
pub mod release;
pub mod review;
pub mod sign_in;
pub mod threads;
pub mod workspace;

pub const REQUEST_LIMIT: usize = 64 * 1024;
/// Newest protocol this client speaks. Version 2 adds ordered tools and approval modes.
/// Version 3 adds full history, thread metadata, a live thread list, in-thread model and
/// thinking changes, queued inputs, usage, and reasoning. Version 4 adds agent harnesses.
pub const PROTOCOL_VERSION: u32 = 7;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelChoice {
    pub provider: String,
    pub model_id: String,
}

/// `Default` on the wire types below lets a call site name only the fields it has,
/// so a later optional field does not break it.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub provider: String,
    pub model_id: String,
    pub name: String,
    pub authenticated: bool,
    /// Capabilities are present only in a version 3 catalog.
    #[serde(default)]
    pub reasoning: Option<bool>,
    #[serde(default)]
    pub thinking_levels: Option<Vec<ThinkingLevel>>,
    #[serde(default)]
    pub context_window: Option<u64>,
    #[serde(default)]
    pub input: Option<Vec<String>>,
    #[serde(default)]
    pub image_input: Option<String>,
}

/// How much a reasoning model thinks before it answers. `Off` is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}
impl ThinkingLevel {
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Minimal => "Minimal",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Xhigh => "Extra high",
            Self::Max => "Maximum",
        }
    }
}

impl Model {
    pub fn choice(&self) -> ModelChoice {
        ModelChoice {
            provider: self.provider.clone(),
            model_id: self.model_id.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub protocol_version: u32,
    pub default_model: Option<ModelChoice>,
    pub models: Vec<Model>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    #[default]
    Idle,
    Running,
    AwaitingApproval,
    Stopped,
    Interrupted,
    Error,
}

impl ThreadStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Ready",
            Self::Running => "Running",
            Self::AwaitingApproval => "Needs approval",
            Self::Stopped => "Stopped",
            Self::Interrupted => "Interrupted",
            Self::Error => "Error",
        }
    }
    pub fn busy(self) -> bool {
        matches!(self, Self::Running | Self::AwaitingApproval)
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ThreadSummary {
    #[serde(default)]
    pub checkout_queue: Option<checkout_queue::Summary>,
    pub id: String,
    pub checkout: PathBuf,
    pub model: ModelChoice,
    pub status: ThreadStatus,
    /// Version 3 only. Empty until the thread has a first message or a chosen name.
    #[serde(default)]
    pub title: Option<String>,
    /// Version 3 only, as are the fields below. Unix milliseconds.
    #[serde(default)]
    pub created_at: Option<u64>,
    #[serde(default)]
    pub updated_at: Option<u64>,
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub pending_approval: Option<bool>,
    /// Version 4 only.
    #[serde(default)]
    pub harness: Option<harness::HarnessId>,
}

impl From<&Snapshot> for ThreadSummary {
    fn from(snapshot: &Snapshot) -> Self {
        Self {
            checkout_queue: None,
            id: snapshot.id.clone(),
            checkout: snapshot.checkout.clone(),
            model: snapshot.model.clone(),
            status: snapshot.status,
            title: snapshot.title.clone(),
            created_at: snapshot.created_at,
            updated_at: snapshot.updated_at,
            archived: snapshot.archived,
            // A version 1 or 2 snapshot says nothing about version 3 summaries.
            pending_approval: snapshot
                .title
                .is_some()
                .then_some(snapshot.pending_approval.is_some()),
            harness: snapshot.harness.as_ref().map(|harness| harness.id),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub id: String,
    pub checkout: PathBuf,
    pub model: ModelChoice,
    pub status: ThreadStatus,
    pub messages: Vec<Message>,
    pub live_text: String,
    pub tools: Vec<Tool>,
    pub pending_approval: Option<Approval>,
    pub error: Option<HostError>,
    pub truncated: bool,
    /// Absent from version 1 hosts, which always ask.
    #[serde(default)]
    pub approval_mode: Option<ApprovalMode>,
    /// The fields below exist only in version 3 snapshots.
    #[serde(default)]
    pub title: Option<String>,
    /// Unix milliseconds.
    #[serde(default)]
    pub created_at: Option<u64>,
    #[serde(default)]
    pub updated_at: Option<u64>,
    #[serde(default)]
    pub archived: Option<bool>,
    #[serde(default)]
    pub thinking_level: Option<ThinkingLevel>,
    /// Inputs waiting behind the active run, oldest first.
    #[serde(default)]
    pub queued: Vec<Queued>,
    /// Tokens of the latest model response; `None` before the first one.
    #[serde(default)]
    pub usage: Option<Usage>,
    #[serde(default)]
    pub live_reasoning: String,
    /// Set while the host waits to repeat a failed model request.
    #[serde(default)]
    pub retry: Option<Retry>,
    /// Version 4 only: which harness runs the thread.
    #[serde(default)]
    pub harness: Option<harness::ThreadHarness>,
    /// Version 5 only; refetch turn records when it changes.
    #[serde(default)]
    pub records_revision: Option<u64>,
    #[serde(default)]
    pub image_input: Option<String>,
}

impl Snapshot {
    /// Capacity of retained text plus a conservative allowance for the bounded wire envelope.
    pub fn retained_bytes(&self) -> usize {
        use std::mem::size_of;
        2 * 1024 * 1024
            + self.messages.capacity() * size_of::<Message>()
            + self.tools.capacity() * size_of::<Tool>()
            + self
                .messages
                .iter()
                .map(|m| {
                    m.id.capacity()
                        + m.text.capacity()
                        + m.reasoning.capacity()
                        + m.images.capacity() * size_of::<crate::images::ImageRef>()
                        + m.images
                            .iter()
                            .map(|i| i.id.capacity() + i.media_type.capacity())
                            .sum::<usize>()
                })
                .sum::<usize>()
            + self
                .tools
                .iter()
                .map(|t| {
                    t.id.capacity() + t.name.capacity() + t.input.capacity() + t.output.capacity()
                })
                .sum::<usize>()
            + self.live_text.capacity()
            + self.live_reasoning.capacity()
    }
}

/// What a submission does when its thread already has an active run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueMode {
    /// Start a new run after the active one ends.
    FollowUp,
    /// Join the active run at its next step.
    Steer,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Queued {
    /// The ID its message takes once it joins the conversation.
    pub id: String,
    pub request_id: String,
    pub text: String,
    pub mode: QueueMode,
    pub approval: ApprovalMode,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub reasoning: Option<u64>,
    pub total: u64,
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Retry {
    pub attempt: u32,
    /// Unix milliseconds of the next attempt.
    pub at: u64,
}

/// How a run's edits and commands are authorized. Sandbox and file checks apply to both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    Ask,
    Auto,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Message {
    #[serde(default)]
    pub images: Vec<crate::images::ImageRef>,
    pub id: String,
    pub role: MessageRole,
    /// Empty for a version 3 assistant message that only reasoned before its tool calls.
    pub text: String,
    /// Version 3 only: the bounded tail of the model's reasoning for this message.
    #[serde(default)]
    pub reasoning: String,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    #[default]
    User,
    Assistant,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub status: ToolStatus,
    pub input: String,
    pub output: String,
    /// `None`: a version 1 host sent no order. `Some(None)`: before any message.
    #[serde(default, deserialize_with = "present")]
    pub after: Option<Option<String>>,
}

// Distinguishes a missing field from an explicit null.
fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Done,
    Error,
    Interrupted,
}
impl ToolStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "Running",
            Self::Done => "Done",
            // The word its opened output is headed with.
            Self::Error => "Failed",
            Self::Interrupted => "Interrupted",
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub id: String,
    pub tool_id: String,
    pub kind: ApprovalKind,
    pub summary: String,
    #[serde(deserialize_with = "crate::display_text::deserialize")]
    pub details: String,
    pub dirty_paths: Vec<String>,
    pub risk: String,
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    Edit,
    Command,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostError {
    #[serde(default)]
    pub retry_after_seconds: Option<u32>,
    pub code: String,
    pub message: String,
    /// A Linear write's recorded outcome: `not_sent`, `rejected` or `unknown`,
    /// or for a conflict, the earlier request's outcome.
    #[serde(default)]
    pub outcome: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    ImagePut {
        protocol_version: u32,
        thread_id: String,
        image_id: String,
        offset: usize,
        total: usize,
        data: String,
    },
    ImageRead {
        protocol_version: u32,
        thread_id: String,
        image_id: String,
        offset: usize,
    },
    Status {
        #[serde(skip_serializing_if = "Option::is_none")]
        protocol_version: Option<u32>,
    },
    SetDefaultModel {
        provider: String,
        model_id: String,
    },
    ListThreads {
        checkout: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        identity: Option<git::WireIdentity>,
    },
    CreateThread {
        checkout: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        identity: Option<git::WireIdentity>,
        provider: String,
        model_id: String,
        request_id: String,
        /// The starting level; the host uses the nearest one the model has.
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<ThinkingLevel>,
        /// Version 4 only; the host runs Pi without it.
        #[serde(skip_serializing_if = "Option::is_none")]
        harness: Option<harness::HarnessId>,
        /// The harness's own mode; the host's default for the harness without it.
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        protocol_version: Option<u32>,
    },
    SubmitCheckout {
        #[serde(skip_serializing_if = "Option::is_none")]
        identity: Option<git::WireIdentity>,
        thread_id: String,
        request_id: String,
        text: String,
        approval: ApprovalMode,
        images: Vec<String>,
    },
    CheckoutQueueInfo {},
    WatchThreadsQueue {
        archived: bool,
    },
    ListThreadsQueue {
        archived: bool,
        limit: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
    },
    CancelCheckoutWait {
        thread_id: String,
        request_id: String,
    },
    ResumeCheckoutQueue {
        thread_id: String,
    },
    CheckoutQueue {
        thread_id: String,
    },
    Submit {
        #[serde(skip_serializing_if = "Option::is_none")]
        identity: Option<git::WireIdentity>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        images: Vec<String>,
        thread_id: String,
        request_id: String,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        protocol_version: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        approval: Option<ApprovalMode>,
        #[serde(skip_serializing_if = "Option::is_none")]
        when_busy: Option<QueueMode>,
    },
    Stop {
        thread_id: String,
    },
    Approve {
        thread_id: String,
        approval_id: String,
        allow: bool,
    },
    Watch {
        thread_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        protocol_version: Option<u32>,
    },
    // Every operation below needs a host that negotiated version 3. Build them with the
    // constructors on `Request`, which set the version.
    /// Threads across checkouts, most recently active first.
    #[serde(rename = "list_threads")]
    ListThreadsPage {
        #[serde(skip_serializing_if = "Option::is_none")]
        checkout: Option<PathBuf>,
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cursor: Option<String>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        archived: bool,
        protocol_version: u32,
    },
    WatchThreads {
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        archived: bool,
        protocol_version: u32,
    },
    History {
        thread_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
        protocol_version: u32,
    },
    HistoryText {
        thread_id: String,
        #[serde(rename = "ref")]
        reference: String,
        offset: u64,
        protocol_version: u32,
    },
    RenameThread {
        thread_id: String,
        title: String,
        protocol_version: u32,
    },
    ArchiveThread {
        thread_id: String,
        archived: bool,
        protocol_version: u32,
    },
    ConfigureThread {
        thread_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        thinking: Option<ThinkingLevel>,
        /// Version 4, harness threads only. A mode alone may change during a run.
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        protocol_version: u32,
    },
    CancelQueued {
        thread_id: String,
        request_id: String,
        protocol_version: u32,
    },
    TurnRecords {
        thread_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        protocol_version: u32,
    },
    UndoTurn {
        thread_id: String,
        record_id: String,
        request_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        leave_changed: Option<bool>,
        protocol_version: u32,
    },
    ChangedFiles {
        thread_id: String,
        protocol_version: u32,
    },
}

impl Request {
    pub fn turn_records(
        thread_id: String,
        before: Option<String>,
        limit: Option<usize>,
        protocol_version: u32,
    ) -> Self {
        Self::TurnRecords {
            thread_id,
            before,
            limit,
            protocol_version,
        }
    }
    pub fn undo_turn(
        thread_id: String,
        record_id: String,
        request_id: String,
        leave_changed: bool,
    ) -> Self {
        Self::UndoTurn {
            thread_id,
            record_id,
            request_id,
            leave_changed: leave_changed.then_some(true),
            protocol_version: 5,
        }
    }

    /// Version 1 hosts reject unknown fields, so only a negotiated version 2 sends any.
    pub fn submit(
        thread_id: String,
        request_id: String,
        text: String,
        approval: Option<ApprovalMode>,
    ) -> Self {
        Self::Submit {
            identity: None,
            images: Vec::new(),
            thread_id,
            request_id,
            text,
            protocol_version: approval.map(|_| 2),
            approval,
            when_busy: None,
        }
    }

    /// A version 3 submission that queues or steers when its thread is busy, and
    /// starts a run as usual when it is not. `approval` is the mode of a run it starts.
    pub fn submit_when_busy(
        thread_id: String,
        request_id: String,
        text: String,
        approval: ApprovalMode,
        when_busy: QueueMode,
    ) -> Self {
        Self::Submit {
            identity: None,
            images: Vec::new(),
            thread_id,
            request_id,
            text,
            protocol_version: Some(3),
            approval: Some(approval),
            when_busy: Some(when_busy),
        }
    }

    /// The largest form of a submission, for size checks before negotiation.
    pub fn submit_bound(thread_id: &str, request_id: &str, text: &str) -> Self {
        Self::submit_when_busy(
            thread_id.into(),
            request_id.into(),
            text.into(),
            ApprovalMode::Auto,
            QueueMode::FollowUp,
        )
    }

    /// Threads of one checkout, or of all with `None`; archived ones only when asked.
    pub fn list_threads_page(
        checkout: Option<PathBuf>,
        limit: Option<u32>,
        cursor: Option<String>,
        archived: bool,
    ) -> Self {
        Self::ListThreadsPage {
            checkout,
            limit,
            cursor,
            archived,
            protocol_version: 3,
        }
    }

    /// The page of a thread's complete conversation older than `before`; the newest with `None`.
    pub fn history(thread_id: String, before: Option<String>, limit: Option<u32>) -> Self {
        Self::History {
            thread_id,
            before,
            limit,
            protocol_version: 3,
        }
    }

    pub fn history_text(thread_id: String, reference: String, offset: u64) -> Self {
        Self::HistoryText {
            thread_id,
            reference,
            offset,
            protocol_version: 3,
        }
    }

    pub fn rename_thread(thread_id: String, title: String) -> Self {
        Self::RenameThread {
            thread_id,
            title,
            protocol_version: 3,
        }
    }

    pub fn archive_thread(thread_id: String, archived: bool) -> Self {
        Self::ArchiveThread {
            thread_id,
            archived,
            protocol_version: 3,
        }
    }

    /// A new thread. Only a version 3 host knows a starting thinking level, and only
    /// a version 4 host another harness, so an older one gets the request it knows.
    /// Pi is never named, so a Pi thread's request is unchanged from version 3.
    pub fn create_thread(
        checkout: PathBuf,
        choice: &ModelChoice,
        request_id: String,
        thinking: Option<ThinkingLevel>,
        harness: harness::HarnessId,
        protocol: u32,
    ) -> Self {
        let thinking = thinking.filter(|_| protocol >= 3);
        let harness = (harness != harness::HarnessId::Pi && protocol >= 4).then_some(harness);
        let protocol_version = if harness.is_some() {
            Some(4)
        } else {
            thinking.map(|_| 3)
        };
        Self::CreateThread {
            checkout,
            identity: None,
            provider: choice.provider.clone(),
            model_id: choice.model_id.clone(),
            request_id,
            thinking,
            harness,
            mode: None,
            protocol_version,
        }
    }

    /// Starts a harness thread in `mode` rather than the harness's default.
    /// Pi threads and older hosts ignore it.
    pub fn with_mode(mut self, chosen: Option<String>) -> Self {
        if let Self::CreateThread { harness, mode, .. } = &mut self {
            *mode = chosen.filter(|_| harness.is_some());
        }
        self
    }

    /// Changes a harness thread's own mode, from its next turn. Allowed during a run.
    pub fn set_thread_mode(thread_id: String, mode: String) -> Self {
        Self::ConfigureThread {
            thread_id,
            provider: None,
            model_id: None,
            thinking: None,
            mode: Some(mode),
            protocol_version: 4,
        }
    }

    /// Changes the model, the thinking level, or both, from the thread's next run.
    pub fn configure_thread(
        thread_id: String,
        model: Option<ModelChoice>,
        thinking: Option<ThinkingLevel>,
    ) -> Self {
        let (provider, model_id) = model.map_or((None, None), |choice| {
            (Some(choice.provider), Some(choice.model_id))
        });
        Self::ConfigureThread {
            thread_id,
            provider,
            model_id,
            thinking,
            mode: None,
            protocol_version: 3,
        }
    }

    pub fn cancel_queued(thread_id: String, request_id: String) -> Self {
        Self::CancelQueued {
            thread_id,
            request_id,
            protocol_version: 3,
        }
    }

    /// Files the thread's tools wrote. Hosts from before this operation reject it.
    pub fn changed_files(thread_id: String) -> Self {
        Self::ChangedFiles {
            thread_id,
            protocol_version: 3,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadsReply {
    pub threads: Vec<ThreadSummary>,
}
/// A version 3 listing page. `next_cursor` continues it; `None` ends it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadsPage {
    pub threads: Vec<ThreadSummary>,
    pub next_cursor: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadReply {
    pub thread: Snapshot,
}
/// Version 5 pages can continue within a record. Append each list at its offset.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnRecordsReply {
    pub records: Vec<TurnRecord>,
    pub before: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnRecord {
    pub id: String,
    pub request_ids: Vec<String>,
    pub request_ids_offset: usize,
    pub request_ids_total: usize,
    pub kind: RecordKind,
    pub undoes: Option<String>,
    #[serde(default, deserialize_with = "decode_outcome")]
    pub outcome: Option<TurnOutcome>,
    pub state: RecordState,
    pub head_moved: bool,
    pub after_restart: bool,
    pub paths: Vec<RecordPath>,
    pub paths_offset: usize,
    pub paths_total: usize,
    pub not_covered: Vec<crate::git::record::Uncovered>,
    pub not_covered_offset: usize,
    pub not_covered_total: usize,
    pub created_at: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnOutcome {
    Finished,
    Failed,
    Stopped,
    Interrupted,
}

fn decode_outcome<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<TurnOutcome>, D::Error> {
    Ok(match Option::<String>::deserialize(d)?.as_deref() {
        Some("finished") => Some(TurnOutcome::Finished),
        Some("failed") => Some(TurnOutcome::Failed),
        Some("stopped") => Some(TurnOutcome::Stopped),
        Some("interrupted") => Some(TurnOutcome::Interrupted),
        _ => None,
    })
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    Turn,
    Undo,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    /// A failed undo with a terminal journal proving no file effects.
    NoEffect,
    Recording,
    Ready,
    Failed,
    Undone,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordPath {
    pub path: PathBuf,
    pub change: RecordChange,
    pub added: Option<u64>,
    pub removed: Option<u64>,
}
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordChange {
    Added,
    Modified,
    Deleted,
    Mode,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UndoTurnReply {
    pub state: UndoState,
    pub changed_since: Vec<PathBuf>,
    pub changed_since_total: usize,
    pub not_covered: Vec<crate::git::record::Uncovered>,
    pub not_covered_total: usize,
    pub record_id: String,
    pub after_restart: bool,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UndoState {
    Undone,
    Blocked,
}

/// Paths relative to the thread's checkout, sorted, at most 500.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangedFilesReply {
    pub files: Vec<PathBuf>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultReply {
    pub default_model: ModelChoice,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitReply {
    pub thread_id: String,
    pub request_id: String,
    pub accepted: bool,
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn existing_request_and_reply_fixtures_keep_their_wire_shape() {
        assert_eq!(
            serde_json::to_value(Request::turn_records(
                "t".into(),
                Some("4:500:0:0".into()),
                Some(2),
                5
            ))
            .unwrap(),
            json!({"op":"turn_records","protocol_version":5,"thread_id":"t","before":"4:500:0:0","limit":2})
        );
        assert_eq!(
            serde_json::to_value(Request::undo_turn("t".into(), "r".into(), "u".into(), true))
                .unwrap(),
            json!({"op":"undo_turn","protocol_version":5,"thread_id":"t","record_id":"r","request_id":"u","leave_changed":true})
        );
        let value = json!({"state":"blocked","changed_since":["a"],"changed_since_total":1,"not_covered":[],"not_covered_total":0,"record_id":"r","after_restart":true});
        let reply: UndoTurnReply = serde_json::from_value(value).unwrap();
        assert_eq!(reply.state, UndoState::Blocked);
        assert_eq!(reply.changed_since, [PathBuf::from("a")]);
        let page: ThreadsPage = serde_json::from_value(json!({"threads":[{"id":"t","checkout":"/repo","model":{"provider":"p","model_id":"m"},"status":"awaiting_approval","title":"Fix it","created_at":5,"updated_at":9,"archived":false,"pending_approval":true}],"next_cursor":"9:1"})).unwrap();
        assert_eq!(page.threads[0].status, ThreadStatus::AwaitingApproval);
        assert_eq!(page.next_cursor.as_deref(), Some("9:1"));
    }
}

#[cfg(test)]
mod identity_compatibility_tests {
    use super::*;
    #[test]
    fn requests_without_identity_keep_legacy_wire_shape() {
        assert_eq!(
            serde_json::to_value(Request::ListThreads {
                checkout: "/repo".into(),
                identity: None
            })
            .unwrap(),
            serde_json::json!({"op":"list_threads","checkout":"/repo"})
        );
        assert_eq!(
            serde_json::to_value(Request::submit(
                "thread".into(),
                "request".into(),
                "hello".into(),
                None
            ))
            .unwrap(),
            serde_json::json!({"op":"submit","thread_id":"thread","request_id":"request","text":"hello"})
        );
        assert_eq!(
            serde_json::to_value(agent::Request::agent_resources("/repo".into())).unwrap(),
            serde_json::json!({"op":"agent_resources","checkout":"/repo","protocol_version":3})
        );
    }
}
