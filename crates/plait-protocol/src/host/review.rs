use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    TurnReviewInfo {
        thread_id: String,
    },
    TurnDiff {
        thread_id: String,
        record_id: String,
        path: String,
    },
    TurnReviewPage {
        token: String,
        offset: usize,
    },
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Checkout {
    pub host_id: String,
    pub id: String,
    pub catalog_revision: u64,
    pub review_revision: u64,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Features {
    pub record_diff: bool,
    pub attribution: bool,
    pub stage_turn: bool,
    pub since_review: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Info {
    pub schema: u32,
    pub checkout: Checkout,
    pub features: Features,
    pub retention_days: u32,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Page<T> {
    pub token: String,
    pub offset: usize,
    pub total: usize,
    pub items: Vec<T>,
    pub next_offset: Option<usize>,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
pub enum Mode {
    #[serde(rename = "100644")]
    File,
    #[serde(rename = "100755")]
    Executable,
    #[serde(rename = "120000")]
    Symlink,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Entry {
    pub mode: Mode,
    pub fingerprint: String,
    pub bytes: u64,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ready,
    Empty,
    Binary,
    Mode,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    OutsideTurn,
    BeforeRecords,
    AfterRestart,
    NotCovered,
    Expired,
    MissingObject,
    ChangedAgain,
    Ambiguous,
    Composed,
    Undone,
    Conversion,
    Conflict,
    Unsupported,
    Limit,
    Changing,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Origin {
    Turn {
        record_id: String,
        thread_id: String,
        request_id: String,
        title: Option<String>,
        created_at: u64,
    },
    Unknown {
        reason: Reason,
    },
    Mixed {
        parts: Vec<Part>,
    },
}
#[derive(Clone, Debug, Deserialize)]
pub struct Part {
    pub first_atom: usize,
    pub last_atom: usize,
    pub origin: SingleOrigin,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SingleOrigin {
    Turn {
        record_id: String,
        thread_id: String,
        request_id: String,
        title: Option<String>,
        created_at: u64,
    },
    Unknown {
        reason: Reason,
    },
}
#[derive(Clone, Debug, Deserialize)]
pub struct Chunk {
    pub byte_offset: usize,
    pub bytes_base64: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Hunk {
    pub id: String,
    pub first_line: usize,
    pub last_line: usize,
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub origin: Origin,
    pub stageable: bool,
    pub refusal: Option<Reason>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Unreviewed,
    Reviewed,
    Changed,
    Expired,
    Unavailable,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Review {
    pub state: ReviewState,
    pub revision: u64,
    pub reviewed_at: Option<u64>,
    pub expires_at: Option<u64>,
    pub source_record_id: Option<String>,
    pub can_mark: bool,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Basis {
    Record,
    Full,
    SinceReview,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Unstaged,
    Staged,
}
#[derive(Clone, Debug, Deserialize)]
pub struct DiffReply {
    pub schema: u32,
    pub checkout: Checkout,
    pub view_id: String,
    pub path: String,
    pub side: Option<Side>,
    pub basis: Basis,
    pub source_record_id: Option<String>,
    pub before: Option<Entry>,
    pub after: Option<Entry>,
    pub status: Status,
    pub reason: Option<Reason>,
    pub patch_bytes: usize,
    pub patch: Page<Chunk>,
    pub hunks: Page<Hunk>,
    pub review: Review,
    pub truncated: bool,
    pub elapsed_ms: u64,
}
