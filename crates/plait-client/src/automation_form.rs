//! GPUI-free model, schedule builder, inline validation, and time zone date
//! helpers for creating host-owned automations (`runtime/src/automations/service.ts`
//! and `schedule.ts`).

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::host::{
    ApprovalMode, ModelChoice, ThinkingLevel,
    automations::{CommandPolicy, Definition, Schedule, Target},
    harness::HarnessId,
};

/// JavaScript `Number.MAX_SAFE_INTEGER` (matches `Number.isSafeInteger` in `service.ts`).
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
/// Keep instructions comfortably inside the host's 64 KiB request limit.
const MAX_INSTRUCTIONS_BYTES: usize = 60_000;
const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const MAX_TZIF_BYTES: u64 = 256 * 1024;
const ZONEINFO_ROOTS: [&str; 2] = ["/usr/share/zoneinfo", "/var/db/timezone/zoneinfo"];

/// Shown when the host's command policy for a target cannot be fetched.
pub const POLICY_UNAVAILABLE_ERROR: &str =
    "Couldn't read this project's command policy. Try again.";

/// Where an automation runs (`Definition::target` kind).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TargetKind {
    #[default]
    Project,
    Worktree,
    Directory,
}

impl TargetKind {
    pub const ALL: [Self; 3] = [Self::Project, Self::Worktree, Self::Directory];

    pub fn label(self) -> &'static str {
        match self {
            Self::Project => "This project (own worktree)",
            Self::Worktree => "An existing worktree",
            Self::Directory => "Folder",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Project | Self::Worktree => {
                "Gets its own worktree from the chosen checkout's current commit, and every later run continues there. Your checkout is never touched."
            }
            Self::Directory => {
                "Runs in place for a plain folder. Folders inside a Git project must use a project target instead."
            }
        }
    }
}

/// Schedule cadence supported by `runtime/src/automations/schedule.ts`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScheduleKind {
    Hourly,
    Daily,
    #[default]
    Weekdays,
    Weekly,
    Once,
}

impl ScheduleKind {
    pub const ALL: [Self; 5] = [
        Self::Hourly,
        Self::Daily,
        Self::Weekdays,
        Self::Weekly,
        Self::Once,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Hourly => "Hourly",
            Self::Daily => "Daily",
            Self::Weekdays => "Weekdays",
            Self::Weekly => "Weekly",
            Self::Once => "Once",
        }
    }

    /// Host default catch-up window (`schedule.ts::defaultCatchUpMs`).
    pub fn default_catch_up_ms(self) -> u64 {
        match self {
            Self::Hourly => 30 * MINUTE_MS,
            Self::Daily | Self::Weekdays | Self::Weekly | Self::Once => 12 * HOUR_MS,
        }
    }

    pub fn default_catch_up_label(self) -> &'static str {
        match self {
            Self::Hourly => "30 minutes",
            Self::Daily | Self::Weekdays | Self::Weekly | Self::Once => "12 hours",
        }
    }
}

/// Weekday choices for `ScheduleKind::Weekly` (`0` = Sunday .. `6` = Saturday).
pub const WEEKDAY_LABELS: [(u8, &str); 7] = [
    (1, "Mon"),
    (2, "Tue"),
    (3, "Wed"),
    (4, "Thu"),
    (5, "Fri"),
    (6, "Sat"),
    (0, "Sun"),
];

/// Plain form values validated and converted into a wire `Definition`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutomationDraftInput {
    pub name: String,
    pub instructions: String,
    pub target_kind: TargetKind,
    pub project_checkout: PathBuf,
    pub directory_path: String,
    pub schedule_kind: ScheduleKind,
    /// Minute `"0"`..`"59"` for `ScheduleKind::Hourly`.
    pub hourly_minute: String,
    /// Time `"HH:mm"` (or `"H:mm"`) for `Daily`, `Weekdays`, `Weekly`, and `Once`.
    pub time: String,
    /// Local calendar date `"YYYY-MM-DD"` for `ScheduleKind::Once`.
    pub once_date: String,
    /// Selected weekdays (`0` = Sunday .. `6` = Saturday) for `ScheduleKind::Weekly`.
    pub weekly_days: Vec<u8>,
    /// IANA time zone (e.g. `"America/New_York"`, `"UTC"`).
    pub time_zone: String,
    pub harness: HarnessId,
    pub model: Option<ModelChoice>,
    pub mode: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub approval: ApprovalMode,
    /// Host command policy for the target; `None` until fetched and never guessed.
    pub command_policy: Option<CommandPolicy>,
    /// Optional catch-up window (blank = host schedule default).
    pub catch_up: String,
}

/// Per-field inline validation messages matching `service.ts` and `schedule.ts`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValidationErrors {
    pub name: Option<String>,
    pub instructions: Option<String>,
    pub target: Option<String>,
    pub schedule_time: Option<String>,
    pub schedule_days: Option<String>,
    pub time_zone: Option<String>,
    pub agent: Option<String>,
    pub command_policy: Option<String>,
    pub catch_up: Option<String>,
}

impl ValidationErrors {
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.instructions.is_none()
            && self.target.is_none()
            && self.schedule_time.is_none()
            && self.schedule_days.is_none()
            && self.time_zone.is_none()
            && self.agent.is_none()
            && self.command_policy.is_none()
            && self.catch_up.is_none()
    }

    pub fn first_message(&self) -> Option<&str> {
        self.name
            .as_deref()
            .or(self.instructions.as_deref())
            .or(self.target.as_deref())
            .or(self.schedule_time.as_deref())
            .or(self.schedule_days.as_deref())
            .or(self.time_zone.as_deref())
            .or(self.agent.as_deref())
            .or(self.command_policy.as_deref())
            .or(self.catch_up.as_deref())
    }
}

/// Validates every user-editable field of `input` (excluding `command_policy`,
/// which is read from the host) and returns all per-field inline validation errors.
pub fn validate_fields(input: &AutomationDraftInput) -> ValidationErrors {
    match validate_and_build_inner(input, Some(CommandPolicy::Sandboxed)) {
        Ok(_) => ValidationErrors::default(),
        Err(errors) => errors,
    }
}

/// Validates every field of `input` against the host's rules and returns either
/// the wire `Definition` or all per-field inline validation errors. Refuses to
/// build if `input.command_policy` is `None` rather than guessing a policy.
// Every field's message is returned together; this runs once per form edit, not in a hot path.
#[allow(clippy::result_large_err)]
pub fn validate_and_build(input: &AutomationDraftInput) -> Result<Definition, ValidationErrors> {
    validate_and_build_inner(input, input.command_policy)
}

/// Validates every field of `input` and builds a wire `Definition` using the
/// explicitly fetched `command_policy`.
// Every field's message is returned together; this runs once per form edit, not in a hot path.
#[allow(clippy::result_large_err)]
pub fn validate_and_build_with_policy(
    input: &AutomationDraftInput,
    command_policy: CommandPolicy,
) -> Result<Definition, ValidationErrors> {
    validate_and_build_inner(input, Some(command_policy))
}

#[allow(clippy::result_large_err)]
fn validate_and_build_inner(
    input: &AutomationDraftInput,
    command_policy: Option<CommandPolicy>,
) -> Result<Definition, ValidationErrors> {
    let mut errors = ValidationErrors::default();

    let name = input.name.trim().to_owned();
    if name.is_empty() {
        errors.name = Some("Enter a name for this automation.".into());
    }

    let instructions = input.instructions.clone();
    if instructions.trim().is_empty() {
        errors.instructions = Some("Enter the instructions sent at each run.".into());
    } else if instructions.len() > MAX_INSTRUCTIONS_BYTES {
        errors.instructions = Some("Instructions are too long for a single host request.".into());
    }

    let target = validate_target(
        input.target_kind,
        &input.project_checkout,
        &input.directory_path,
    )
    .unwrap_or_else(|message| {
        errors.target = Some(message);
        Target::Directory {
            path: PathBuf::new(),
        }
    });

    let schedule = build_schedule(
        input.schedule_kind,
        &input.hourly_minute,
        &input.time,
        &input.once_date,
        &input.weekly_days,
        &input.time_zone,
        &mut errors,
    );

    let agent = validate_agent(input.harness, input.model.as_ref(), input.mode.as_deref());
    let (provider, model_id, mode) = match agent {
        Ok(parts) => parts,
        Err(message) => {
            errors.agent = Some(message);
            (String::new(), String::new(), None)
        }
    };

    let catch_up_ms = match parse_catch_up_ms(&input.catch_up) {
        Ok(value) => value,
        Err(message) => {
            errors.catch_up = Some(message);
            None
        }
    };

    if command_policy.is_none() {
        errors.command_policy = Some(POLICY_UNAVAILABLE_ERROR.into());
    }

    let (Some(schedule), Some(command_policy)) = (schedule, command_policy) else {
        return Err(errors);
    };
    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(Definition {
        name,
        instructions,
        target,
        harness: input.harness,
        provider,
        model_id,
        mode,
        thinking: input.thinking,
        approval: input.approval,
        command_policy,
        schedule,
        catch_up_ms,
    })
}

/// Builds a `Schedule` preview if the schedule fields alone are valid, for live
/// feedback in the form before other fields are filled.
// Every field's message is returned together; this runs once per form edit, not in a hot path.
#[allow(clippy::result_large_err)]
pub fn preview_schedule(input: &AutomationDraftInput) -> Result<Schedule, ValidationErrors> {
    let mut errors = ValidationErrors::default();
    match build_schedule(
        input.schedule_kind,
        &input.hourly_minute,
        &input.time,
        &input.once_date,
        &input.weekly_days,
        &input.time_zone,
        &mut errors,
    ) {
        Some(schedule) if errors.is_empty() => Ok(schedule),
        _ => Err(errors),
    }
}

fn validate_target(
    kind: TargetKind,
    project_checkout: &Path,
    directory_path: &str,
) -> Result<Target, String> {
    match kind {
        TargetKind::Project => {
            validate_host_path(project_checkout, "Choose a project for this automation.")?;
            Ok(Target::Project {
                checkout: project_checkout.to_path_buf(),
            })
        }
        TargetKind::Worktree => {
            validate_host_path(
                project_checkout,
                "Choose a project or worktree for this automation.",
            )?;
            Ok(Target::Worktree {
                checkout: project_checkout.to_path_buf(),
            })
        }
        TargetKind::Directory => {
            let trimmed = directory_path.trim();
            if trimmed.is_empty() {
                return Err("Choose a folder or enter an absolute path.".into());
            }
            let path = PathBuf::from(trimmed);
            validate_host_path(&path, "Enter an absolute folder path starting with /.")?;
            Ok(Target::Directory { path })
        }
    }
}

/// Matches `runtime/src/admission.ts::path` and `host::worktree::path`.
pub fn validate_host_path(path: &Path, empty_message: &str) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Err(empty_message.into());
    }
    let Some(text) = path.to_str() else {
        return Err("Path must be valid UTF-8.".into());
    };
    if !text.starts_with('/') {
        return Err("Path must be absolute (start with /).".into());
    }
    if text.len() > 4096 {
        return Err("Path is longer than the 4096-byte limit.".into());
    }
    if text.bytes().any(|b| b.is_ascii_control()) {
        return Err("Path cannot contain control characters.".into());
    }
    if text.contains("//") || text.split('/').any(|part| part == "." || part == "..") {
        return Err("Path cannot contain //, ., or .. segments.".into());
    }
    Ok(())
}

fn validate_agent(
    harness: HarnessId,
    model: Option<&ModelChoice>,
    mode: Option<&str>,
) -> Result<(String, String, Option<String>), String> {
    if harness == HarnessId::Other {
        return Err("Choose Pi, Codex, or Claude Code.".into());
    }
    let Some(model) = model else {
        return Err("Choose a model for this automation.".into());
    };
    let provider = model.provider.trim();
    let model_id = model.model_id.trim();
    if provider.is_empty()
        || model_id.is_empty()
        || provider.len() > 256
        || model_id.len() > 256
        || provider.bytes().any(|b| b.is_ascii_control())
        || model_id.bytes().any(|b| b.is_ascii_control())
    {
        return Err("Choose a valid model for this automation.".into());
    }
    let mode = if harness == HarnessId::Pi {
        None
    } else {
        let Some(mode) = mode.map(str::trim).filter(|m| !m.is_empty()) else {
            return Err("Select a harness mode.".into());
        };
        if !is_valid_mode(mode) {
            return Err("Invalid harness mode.".into());
        }
        Some(mode.to_owned())
    };
    Ok((provider.to_owned(), model_id.to_owned(), mode))
}

fn is_valid_mode(mode: &str) -> bool {
    !mode.is_empty()
        && mode.len() <= 32
        && mode
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

fn build_schedule(
    kind: ScheduleKind,
    hourly_minute: &str,
    time: &str,
    once_date: &str,
    weekly_days: &[u8],
    time_zone: &str,
    errors: &mut ValidationErrors,
) -> Option<Schedule> {
    let zone = match validate_time_zone(time_zone) {
        Ok(zone) => Some(zone),
        Err(message) => {
            errors.time_zone = Some(message);
            None
        }
    };

    match kind {
        ScheduleKind::Hourly => {
            let minute = match parse_minute(hourly_minute) {
                Ok(minute) => Some(minute),
                Err(message) => {
                    errors.schedule_time = Some(message);
                    None
                }
            };
            Some(Schedule::Hourly {
                minute: minute?,
                time_zone: zone?,
            })
        }
        ScheduleKind::Daily => {
            let time = match parse_hh_mm(time) {
                Ok(time) => Some(time),
                Err(message) => {
                    errors.schedule_time = Some(message);
                    None
                }
            };
            Some(Schedule::Daily {
                time: time?,
                time_zone: zone?,
            })
        }
        ScheduleKind::Weekdays => {
            let time = match parse_hh_mm(time) {
                Ok(time) => Some(time),
                Err(message) => {
                    errors.schedule_time = Some(message);
                    None
                }
            };
            Some(Schedule::Weekdays {
                time: time?,
                time_zone: zone?,
            })
        }
        ScheduleKind::Weekly => {
            let time = match parse_hh_mm(time) {
                Ok(time) => Some(time),
                Err(message) => {
                    errors.schedule_time = Some(message);
                    None
                }
            };
            let days = match normalize_weekdays(weekly_days) {
                Ok(days) => Some(days),
                Err(message) => {
                    errors.schedule_days = Some(message);
                    None
                }
            };
            Some(Schedule::Weekly {
                days: days?,
                time: time?,
                time_zone: zone?,
            })
        }
        ScheduleKind::Once => {
            let date = parse_date(once_date);
            let clock = parse_hh_mm(time);
            let at = match (date, clock) {
                (Ok(date), Ok(clock)) => Some(format!("{date}T{clock}")),
                (Err(message), _) | (_, Err(message)) => {
                    errors.schedule_time = Some(message);
                    None
                }
            };
            Some(Schedule::Once {
                at: at?,
                time_zone: zone?,
            })
        }
    }
}

/// Parses an hourly minute (`0`..=`59`).
pub fn parse_minute(input: &str) -> Result<u8, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Enter a minute from 0 to 59.".into());
    }
    let minute: u8 = trimmed
        .parse()
        .map_err(|_| "Enter a minute from 0 to 59.".to_owned())?;
    if minute > 59 {
        return Err("Minute must be from 0 to 59.".into());
    }
    Ok(minute)
}

/// Parses `"HH:mm"` (also accepting single-digit hour `"H:mm"`) and returns
/// zero-padded `"HH:mm"`.
pub fn parse_hh_mm(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    let Some((hour_str, min_str)) = trimmed.split_once(':') else {
        return Err("Enter a 24-hour time as HH:mm (for example 09:00).".into());
    };
    if hour_str.is_empty()
        || hour_str.len() > 2
        || min_str.len() != 2
        || !hour_str.bytes().all(|b| b.is_ascii_digit())
        || !min_str.bytes().all(|b| b.is_ascii_digit())
    {
        return Err("Enter a 24-hour time as HH:mm (for example 09:00).".into());
    }
    let hour: u8 = hour_str
        .parse()
        .map_err(|_| "Enter a 24-hour time as HH:mm (for example 09:00).".to_owned())?;
    let minute: u8 = min_str
        .parse()
        .map_err(|_| "Enter a 24-hour time as HH:mm (for example 09:00).".to_owned())?;
    if hour > 23 || minute > 59 {
        return Err("Time must be between 00:00 and 23:59.".into());
    }
    Ok(format!("{hour:02}:{minute:02}"))
}

fn days_in_month(year: u32, month: u32) -> u32 {
    if month == 2 {
        let leap =
            (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
        return if leap { 29 } else { 28 };
    }
    match month {
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Parses `"YYYY-MM-DD"` with exact calendar date and leap-year checks.
pub fn parse_date(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    let parts: Vec<&str> = trimmed.split('-').collect();
    if parts.len() != 3
        || parts[0].len() != 4
        || parts[1].len() != 2
        || parts[2].len() != 2
        || parts.iter().any(|p| !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("Enter a date as YYYY-MM-DD (for example 2026-06-01).".into());
    }
    let year: u32 = parts[0]
        .parse()
        .map_err(|_| "Enter a valid calendar date as YYYY-MM-DD.".to_owned())?;
    let month: u32 = parts[1]
        .parse()
        .map_err(|_| "Enter a valid calendar date as YYYY-MM-DD.".to_owned())?;
    let day: u32 = parts[2]
        .parse()
        .map_err(|_| "Enter a valid calendar date as YYYY-MM-DD.".to_owned())?;
    if year < 1 || !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return Err("Date is out of range for that month.".into());
    }
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

fn normalize_weekdays(days: &[u8]) -> Result<Vec<u8>, String> {
    if days.is_empty() {
        return Err("Choose at least one day of the week.".into());
    }
    if days.iter().any(|day| *day > 6) {
        return Err("Weekdays must be between Sunday (0) and Saturday (6).".into());
    }
    let mut normalized = days.to_vec();
    normalized.sort_unstable();
    normalized.dedup();
    Ok(normalized)
}

/// Validates syntax of an IANA time zone name per `schedule.ts::validateTimeZone`.
pub fn validate_time_zone(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty()
        || trimmed.len() > 128
        || trimmed.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("Enter an IANA time zone (for example America/New_York or UTC).".into());
    }
    if trimmed.starts_with('+') || trimmed.starts_with('-') {
        return Err(
            "Use an IANA time zone name like America/New_York or UTC, not a numeric offset.".into(),
        );
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'))
        || trimmed.starts_with('/')
        || trimmed.ends_with('/')
        || trimmed.contains("//")
    {
        return Err("Enter a valid IANA time zone (for example America/New_York or UTC).".into());
    }
    Ok(trimmed.to_owned())
}

/// Parses an optional catch-up window into milliseconds.
/// Blank returns `Ok(None)` so the host uses the schedule's default.
/// Accepts `"0"`, plain minutes (`"30"`), minutes with suffix (`"30m"`, `"30min"`),
/// or hours (`"2h"`, `"12hr"`).
pub fn parse_catch_up_ms(input: &str) -> Result<Option<u64>, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let lower = trimmed.to_ascii_lowercase();
    let (digits, multiplier) = if let Some(rest) = lower.strip_suffix("min") {
        (rest.trim(), MINUTE_MS)
    } else if let Some(rest) = lower.strip_suffix('m') {
        (rest.trim(), MINUTE_MS)
    } else if let Some(rest) = lower.strip_suffix("hr") {
        (rest.trim(), HOUR_MS)
    } else if let Some(rest) = lower.strip_suffix('h') {
        (rest.trim(), HOUR_MS)
    } else {
        (lower.as_str(), MINUTE_MS)
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Enter whole minutes or hours (for example 30m, 2h, or 0).".into());
    }
    let count: u64 = digits
        .parse()
        .map_err(|_| "Catch-up window is too large.".to_owned())?;
    let ms = count
        .checked_mul(multiplier)
        .filter(|value| *value <= MAX_SAFE_INTEGER)
        .ok_or_else(|| "Catch-up window is too large.".to_owned())?;
    Ok(Some(ms))
}

/// Extracts an IANA time zone name from a symlink target like
/// `/var/db/timezone/zoneinfo/America/Los_Angeles` or
/// `/usr/share/zoneinfo/posix/Europe/London`.
pub fn parse_zoneinfo_path(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    let after = text.split_once("zoneinfo/")?.1;
    let cleaned = after
        .strip_prefix("posix/")
        .or_else(|| after.strip_prefix("right/"))
        .unwrap_or(after);
    validate_time_zone(cleaned).ok()
}

/// Best-effort local IANA time zone detection for macOS and Linux.
/// Performs filesystem reads (`/etc/localtime`, `/etc/timezone`); call on a
/// background executor, never during render.
pub fn detect_local_time_zone() -> String {
    if let Ok(tz) = std::env::var("TZ") {
        let candidate = tz.strip_prefix(':').unwrap_or(&tz);
        if let Ok(valid) = validate_time_zone(candidate) {
            return valid;
        }
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime")
        && let Some(zone) = parse_zoneinfo_path(&target)
    {
        return zone;
    }
    if let Ok(contents) = std::fs::read_to_string("/etc/timezone")
        && let Ok(valid) = validate_time_zone(&contents)
    {
        return valid;
    }
    "UTC".into()
}

/// Current UTC calendar date as `"YYYY-MM-DD"`.
pub fn today_utc_date() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    civil_date_from_unix_days((secs / 86_400) as i64)
}

/// Calendar date `"YYYY-MM-DD"` at `unix_secs` in the given IANA `time_zone`.
/// Reads the system TZif file under `/usr/share/zoneinfo` or
/// `/var/db/timezone/zoneinfo`; call on a background executor, never in render.
pub fn date_in_time_zone(unix_secs: i64, time_zone: &str) -> Option<String> {
    let offset_secs = utc_offset_in_time_zone(unix_secs, time_zone)?;
    let local_secs = unix_secs.checked_add(i64::from(offset_secs))?;
    Some(civil_date_from_unix_days(local_secs.div_euclid(86_400)))
}

/// Today's calendar date `"YYYY-MM-DD"` in the given IANA `time_zone`.
/// Performs filesystem reads into system zoneinfo; call on a background executor.
pub fn today_date_in_time_zone(time_zone: &str) -> Option<String> {
    let secs = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
    .ok()?;
    date_in_time_zone(secs, time_zone)
}

fn utc_offset_in_time_zone(unix_secs: i64, time_zone: &str) -> Option<i32> {
    let zone = validate_time_zone(time_zone).ok()?;
    if zone.split('/').any(|part| part == "." || part == "..") {
        return None;
    }
    if matches!(
        zone.as_str(),
        "UTC" | "Etc/UTC" | "GMT" | "Etc/GMT" | "Etc/GMT+0" | "Etc/GMT-0" | "Etc/GMT0"
    ) {
        return Some(0);
    }
    for root in ZONEINFO_ROOTS {
        let path = Path::new(root).join(&zone);
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let mut bytes = Vec::new();
        if file.take(MAX_TZIF_BYTES).read_to_end(&mut bytes).is_ok()
            && let Some(offset) = tzif_offset_at(&bytes, unix_secs)
        {
            return Some(offset);
        }
    }
    None
}

struct TzifHeader {
    version: u8,
    isutcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

fn read_be_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_be_bytes(slice))
}

fn read_be_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    let slice: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(i32::from_be_bytes(slice))
}

fn read_be_i64(bytes: &[u8], offset: usize) -> Option<i64> {
    let slice: [u8; 8] = bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?;
    Some(i64::from_be_bytes(slice))
}

fn parse_tzif_header(bytes: &[u8], offset: usize) -> Option<TzifHeader> {
    if bytes.get(offset..offset.checked_add(4)?)? != b"TZif" {
        return None;
    }
    let version = *bytes.get(offset.checked_add(4)?)?;
    let isutcnt = usize::try_from(read_be_u32(bytes, offset.checked_add(20)?)?).ok()?;
    let isstdcnt = usize::try_from(read_be_u32(bytes, offset.checked_add(24)?)?).ok()?;
    let leapcnt = usize::try_from(read_be_u32(bytes, offset.checked_add(28)?)?).ok()?;
    let timecnt = usize::try_from(read_be_u32(bytes, offset.checked_add(32)?)?).ok()?;
    let typecnt = usize::try_from(read_be_u32(bytes, offset.checked_add(36)?)?).ok()?;
    let charcnt = usize::try_from(read_be_u32(bytes, offset.checked_add(40)?)?).ok()?;
    if typecnt == 0 {
        return None;
    }
    Some(TzifHeader {
        version,
        isutcnt,
        isstdcnt,
        leapcnt,
        timecnt,
        typecnt,
        charcnt,
    })
}

fn tzif_body_len(header: &TzifHeader, time_size: usize, leap_size: usize) -> Option<usize> {
    header
        .timecnt
        .checked_mul(time_size)?
        .checked_add(header.timecnt)?
        .checked_add(header.typecnt.checked_mul(6)?)?
        .checked_add(header.charcnt)?
        .checked_add(header.leapcnt.checked_mul(leap_size)?)?
        .checked_add(header.isstdcnt)?
        .checked_add(header.isutcnt)
}

/// Parses an RFC 8536 TZif v1/v2/v3 file and returns the UTC offset (seconds
/// east of UTC) in effect at `unix_secs`.
fn tzif_offset_at(bytes: &[u8], unix_secs: i64) -> Option<i32> {
    let v1 = parse_tzif_header(bytes, 0)?;
    if v1.version == 0 {
        return lookup_tzif_block(bytes, 44, &v1, false, None, unix_secs);
    }
    let v2_start = 44usize.checked_add(tzif_body_len(&v1, 4, 8)?)?;
    let v2 = parse_tzif_header(bytes, v2_start)?;
    let data_start = v2_start.checked_add(44)?;
    let footer_start = data_start.checked_add(tzif_body_len(&v2, 8, 12)?)?;
    let footer = bytes
        .get(footer_start..)?
        .strip_prefix(b"\n")
        .and_then(|rest| rest.strip_suffix(b"\n"))
        .and_then(|raw| std::str::from_utf8(raw).ok())
        .filter(|s| !s.is_empty());
    lookup_tzif_block(bytes, data_start, &v2, true, footer, unix_secs)
}

fn lookup_tzif_block(
    bytes: &[u8],
    data_start: usize,
    header: &TzifHeader,
    is_64bit: bool,
    footer: Option<&str>,
    unix_secs: i64,
) -> Option<i32> {
    let time_size = if is_64bit { 8 } else { 4 };
    let times_len = header.timecnt.checked_mul(time_size)?;
    let types_start = data_start.checked_add(times_len)?;
    let ttinfo_start = types_start.checked_add(header.timecnt)?;
    let ttinfo_end = ttinfo_start.checked_add(header.typecnt.checked_mul(6)?)?;
    if ttinfo_end > bytes.len() {
        return None;
    }

    let mut transitions = Vec::with_capacity(header.timecnt);
    for idx in 0..header.timecnt {
        let offset = data_start.checked_add(idx.checked_mul(time_size)?)?;
        let ts = if is_64bit {
            read_be_i64(bytes, offset)?
        } else {
            i64::from(read_be_i32(bytes, offset)?)
        };
        transitions.push(ts);
    }
    let type_indices = bytes.get(types_start..ttinfo_start)?;
    let mut ttinfos = Vec::with_capacity(header.typecnt);
    for idx in 0..header.typecnt {
        let offset = ttinfo_start.checked_add(idx.checked_mul(6)?)?;
        let utoff = read_be_i32(bytes, offset)?;
        let is_dst = *bytes.get(offset + 4)? != 0;
        ttinfos.push((utoff, is_dst));
    }

    if transitions.is_empty() {
        if let Some(offset) = footer.and_then(|tz| posix_tz_offset_at(tz, unix_secs)) {
            return Some(offset);
        }
        return ttinfos.first().map(|(utoff, _)| *utoff);
    }
    if unix_secs < transitions[0] {
        return ttinfos
            .iter()
            .find(|(_, is_dst)| !*is_dst)
            .or_else(|| ttinfos.first())
            .map(|(utoff, _)| *utoff);
    }
    let pos = transitions.partition_point(|&ts| ts <= unix_secs);
    if pos == transitions.len()
        && let Some(offset) = footer.and_then(|tz| posix_tz_offset_at(tz, unix_secs))
    {
        return Some(offset);
    }
    let type_idx = usize::from(*type_indices.get(pos.checked_sub(1)?)?);
    ttinfos.get(type_idx).map(|(utoff, _)| *utoff)
}

/// Evaluates an RFC 8536 §3.3.1 POSIX TZ footer string (e.g. `EST5EDT,M3.2.0,M11.1.0`
/// or `<+14>-14`) and returns seconds east of UTC at `unix_secs`.
fn posix_tz_offset_at(tz: &str, unix_secs: i64) -> Option<i32> {
    let rest = take_posix_name(tz)?;
    let (std_west, rest) = take_posix_hms(rest)?;
    let std_utoff = std_west.checked_neg()?;
    if rest.is_empty() {
        return Some(std_utoff);
    }
    let rest = take_posix_name(rest)?;
    let (dst_utoff, rest) = if rest.starts_with(',') {
        (std_utoff.checked_add(3600)?, rest)
    } else if rest.is_empty() {
        return Some(std_utoff);
    } else {
        let (dst_west, rest) = take_posix_hms(rest)?;
        (dst_west.checked_neg()?, rest)
    };
    let rules = rest.strip_prefix(',')?;
    let (start_rule, end_rule) = rules.split_once(',')?;
    let local_days = unix_secs
        .checked_add(i64::from(std_utoff))?
        .div_euclid(86_400);
    let (year, _, _) = civil_from_unix_days(local_days);
    let start_utc = posix_rule_local_secs(year, start_rule)?.checked_sub(i64::from(std_utoff))?;
    let end_utc = posix_rule_local_secs(year, end_rule)?.checked_sub(i64::from(dst_utoff))?;
    let is_dst = if start_utc < end_utc {
        unix_secs >= start_utc && unix_secs < end_utc
    } else {
        unix_secs >= start_utc || unix_secs < end_utc
    };
    Some(if is_dst { dst_utoff } else { std_utoff })
}

fn take_posix_name(input: &str) -> Option<&str> {
    if let Some(after_open) = input.strip_prefix('<') {
        let close = after_open.find('>')?;
        return after_open.get(close + 1..);
    }
    let end = input
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(input.len());
    (end > 0).then(|| &input[end..])
}

/// Parses a signed POSIX offset or time `[+-]?h[:mm[:ss]]` into seconds.
fn take_posix_hms(input: &str) -> Option<(i32, &str)> {
    let (sign, mut rest) = if let Some(after) = input.strip_prefix('-') {
        (-1i32, after)
    } else if let Some(after) = input.strip_prefix('+') {
        (1i32, after)
    } else {
        (1i32, input)
    };
    let (hours, after) = take_uint(rest)?;
    rest = after;
    let (minutes, after) = if let Some(after_colon) = rest.strip_prefix(':') {
        take_uint(after_colon)?
    } else {
        (0, rest)
    };
    rest = after;
    let (seconds, after) = if let Some(after_colon) = rest.strip_prefix(':') {
        take_uint(after_colon)?
    } else {
        (0, rest)
    };
    let total = hours
        .checked_mul(3600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)?;
    Some((total.checked_mul(sign)?, after))
}

fn take_uint(input: &str) -> Option<(i32, &str)> {
    let end = input
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(input.len());
    if end == 0 {
        return None;
    }
    let value: i32 = input[..end].parse().ok()?;
    Some((value, &input[end..]))
}

fn posix_rule_local_secs(year: i64, rule: &str) -> Option<i64> {
    let (date_part, time_secs) = match rule.split_once('/') {
        Some((date, time)) => {
            let (secs, rest) = take_posix_hms(time)?;
            if !rest.is_empty() {
                return None;
            }
            (date, i64::from(secs))
        }
        None => (rule, 2 * 3600),
    };
    let unix_day = if let Some(spec) = date_part.strip_prefix('M') {
        let mut parts = spec.split('.');
        let month: u32 = parts.next()?.parse().ok()?;
        let week: u32 = parts.next()?.parse().ok()?;
        let weekday: i64 = parts.next()?.parse().ok()?;
        if parts.next().is_some()
            || !(1..=12).contains(&month)
            || !(1..=5).contains(&week)
            || !(0..=6).contains(&weekday)
        {
            return None;
        }
        let first_day = unix_days_from_civil(year, month, 1);
        let first_wd = (first_day + 4).rem_euclid(7);
        let offset = (weekday - first_wd).rem_euclid(7);
        let mut day_of_month = 1 + offset as u32 + (week - 1) * 7;
        let max_day = days_in_month(u32::try_from(year).ok()?, month);
        if week == 5 && day_of_month > max_day {
            day_of_month -= 7;
        }
        if day_of_month > max_day {
            return None;
        }
        first_day + i64::from(day_of_month - 1)
    } else if let Some(julian) = date_part.strip_prefix('J') {
        let day: u32 = julian.parse().ok()?;
        if !(1..=365).contains(&day) {
            return None;
        }
        let leap = is_leap_year(year);
        let zero_based = if leap && day >= 60 { day } else { day - 1 };
        unix_days_from_civil(year, 1, 1) + i64::from(zero_based)
    } else {
        let day: u32 = date_part.parse().ok()?;
        if day > 365 {
            return None;
        }
        unix_days_from_civil(year, 1, 1) + i64::from(day)
    };
    unix_day.checked_mul(86_400)?.checked_add(time_secs)
}

fn is_leap_year(year: i64) -> bool {
    (year.rem_euclid(4) == 0 && year.rem_euclid(100) != 0) || year.rem_euclid(400) == 0
}

fn unix_days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = u64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + u64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Converts days since 1970-01-01 into `(year, month, day)` (Hinnant civil-from-days).
fn civil_from_unix_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = y + i64::from(m <= 2);
    (year, m, d)
}

/// Converts days since 1970-01-01 into `"YYYY-MM-DD"`.
fn civil_date_from_unix_days(days: i64) -> String {
    let (year, m, d) = civil_from_unix_days(days);
    format!("{year:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_input() -> AutomationDraftInput {
        AutomationDraftInput {
            name: "Daily dependency check".into(),
            instructions: "Check outdated dependencies and report findings.".into(),
            target_kind: TargetKind::Project,
            project_checkout: PathBuf::from("/code/forecast"),
            directory_path: String::new(),
            schedule_kind: ScheduleKind::Weekdays,
            hourly_minute: "15".into(),
            time: "09:00".into(),
            once_date: "2026-06-01".into(),
            weekly_days: vec![1, 3, 5],
            time_zone: "America/New_York".into(),
            harness: HarnessId::Pi,
            model: Some(ModelChoice {
                provider: "openai-codex".into(),
                model_id: "gpt-5.6-luna".into(),
            }),
            mode: None,
            thinking: None,
            approval: ApprovalMode::Auto,
            command_policy: Some(CommandPolicy::Sandboxed),
            catch_up: String::new(),
        }
    }

    #[test]
    fn builds_every_schedule_kind_and_normalizes_times_and_days() {
        let mut input = valid_input();

        input.schedule_kind = ScheduleKind::Hourly;
        input.hourly_minute = "5".into();
        let def = validate_and_build(&input).unwrap();
        assert_eq!(
            def.schedule,
            Schedule::Hourly {
                minute: 5,
                time_zone: "America/New_York".into(),
            }
        );

        input.schedule_kind = ScheduleKind::Daily;
        input.time = "8:30".into();
        let def = validate_and_build(&input).unwrap();
        assert_eq!(
            def.schedule,
            Schedule::Daily {
                time: "08:30".into(),
                time_zone: "America/New_York".into(),
            }
        );

        input.schedule_kind = ScheduleKind::Weekdays;
        input.time = "09:00".into();
        let def = validate_and_build(&input).unwrap();
        assert_eq!(
            def.schedule,
            Schedule::Weekdays {
                time: "09:00".into(),
                time_zone: "America/New_York".into(),
            }
        );

        input.schedule_kind = ScheduleKind::Weekly;
        input.weekly_days = vec![5, 1, 3, 1];
        let def = validate_and_build(&input).unwrap();
        assert_eq!(
            def.schedule,
            Schedule::Weekly {
                days: vec![1, 3, 5],
                time: "09:00".into(),
                time_zone: "America/New_York".into(),
            }
        );

        input.schedule_kind = ScheduleKind::Once;
        input.once_date = "2028-02-29".into(); // leap year
        input.time = "14:45".into();
        let def = validate_and_build(&input).unwrap();
        assert_eq!(
            def.schedule,
            Schedule::Once {
                at: "2028-02-29T14:45".into(),
                time_zone: "America/New_York".into(),
            }
        );
    }

    #[test]
    fn rejects_invalid_schedules_matching_runtime_rules() {
        let mut input = valid_input();
        input.schedule_kind = ScheduleKind::Hourly;
        input.hourly_minute = "60".into();
        assert!(
            validate_and_build(&input)
                .unwrap_err()
                .schedule_time
                .is_some()
        );

        input.schedule_kind = ScheduleKind::Daily;
        input.time = "24:00".into();
        assert!(
            validate_and_build(&input)
                .unwrap_err()
                .schedule_time
                .is_some()
        );

        input.schedule_kind = ScheduleKind::Weekly;
        input.time = "09:00".into();
        input.weekly_days = vec![];
        assert!(
            validate_and_build(&input)
                .unwrap_err()
                .schedule_days
                .is_some()
        );

        input.schedule_kind = ScheduleKind::Once;
        input.once_date = "2026-02-29".into(); // not a leap year
        assert!(
            validate_and_build(&input)
                .unwrap_err()
                .schedule_time
                .is_some()
        );

        input.schedule_kind = ScheduleKind::Weekdays;
        input.time_zone = "+02:00".into();
        assert!(validate_and_build(&input).unwrap_err().time_zone.is_some());

        input.time_zone = "America/New York".into();
        assert!(validate_and_build(&input).unwrap_err().time_zone.is_some());
    }

    #[test]
    fn validates_targets_agent_modes_and_catch_up_windows() {
        let mut input = valid_input();
        input.name = "   ".into();
        input.instructions = "\n\t".into();
        let err = validate_and_build(&input).unwrap_err();
        assert!(err.name.is_some());
        assert!(err.instructions.is_some());

        let mut input = valid_input();
        input.target_kind = TargetKind::Directory;
        input.directory_path = "relative/folder".into();
        assert!(validate_and_build(&input).unwrap_err().target.is_some());

        input.directory_path = "/tmp/../etc".into();
        assert!(validate_and_build(&input).unwrap_err().target.is_some());

        input.directory_path = "/tmp//dir".into();
        assert!(validate_and_build(&input).unwrap_err().target.is_some());

        input.directory_path = "/Users/me/notes".into();
        let def = validate_and_build(&input).unwrap();
        assert_eq!(
            def.target,
            Target::Directory {
                path: PathBuf::from("/Users/me/notes"),
            }
        );

        let mut input = valid_input();
        input.target_kind = TargetKind::Worktree;
        let def = validate_and_build(&input).unwrap();
        assert_eq!(
            def.target,
            Target::Worktree {
                checkout: PathBuf::from("/code/forecast"),
            }
        );

        // Non-Pi harnesses require a mode matching ^[a-z_]{1,32}$.
        let mut input = valid_input();
        input.harness = HarnessId::Codex;
        input.mode = None;
        assert!(validate_and_build(&input).unwrap_err().agent.is_some());

        input.mode = Some("Bad-Mode".into());
        assert!(validate_and_build(&input).unwrap_err().agent.is_some());

        input.mode = Some("workspace_write".into());
        let def = validate_and_build(&input).unwrap();
        assert_eq!(def.mode.as_deref(), Some("workspace_write"));

        // Pi omits mode even if stale mode text remained.
        input.harness = HarnessId::Pi;
        let def = validate_and_build(&input).unwrap();
        assert_eq!(def.mode, None);

        // Catch-up window parsing.
        assert_eq!(parse_catch_up_ms("").unwrap(), None);
        assert_eq!(parse_catch_up_ms("0").unwrap(), Some(0));
        assert_eq!(parse_catch_up_ms("30").unwrap(), Some(30 * MINUTE_MS));
        assert_eq!(parse_catch_up_ms("45m").unwrap(), Some(45 * MINUTE_MS));
        assert_eq!(parse_catch_up_ms("2h").unwrap(), Some(2 * HOUR_MS));
        assert!(parse_catch_up_ms("-5m").is_err());
        assert!(parse_catch_up_ms("1.5h").is_err());
        assert!(parse_catch_up_ms("soon").is_err());
    }

    #[test]
    fn refuses_to_guess_command_policy_when_missing() {
        let mut input = valid_input();
        input.command_policy = None;
        let err = validate_and_build(&input).unwrap_err();
        assert_eq!(
            err.command_policy.as_deref(),
            Some(POLICY_UNAVAILABLE_ERROR)
        );
        // User-editable fields alone remain valid while policy is still being fetched.
        assert!(validate_fields(&input).is_empty());
        // Supplying the fetched policy explicitly builds the definition.
        let def = validate_and_build_with_policy(&input, CommandPolicy::Sandboxed).unwrap();
        assert_eq!(def.command_policy, CommandPolicy::Sandboxed);
    }

    #[test]
    fn target_wording_does_not_suggest_placement_difference_for_project_and_worktree() {
        assert_eq!(TargetKind::Project.label(), "This project (own worktree)");
        assert_eq!(TargetKind::Worktree.label(), "An existing worktree");
        assert_eq!(TargetKind::Project.hint(), TargetKind::Worktree.hint());
        assert!(TargetKind::Project.hint().contains("own worktree"));
        assert!(TargetKind::Project.hint().contains("never touched"));
    }

    #[test]
    fn once_date_uses_chosen_time_zone_across_day_boundaries() {
        // 2026-06-01T02:00:00Z: still 2026-05-31 in America/New_York (EDT, UTC-4),
        // already 2026-06-01 in UTC and Asia/Tokyo (JST, UTC+9).
        let summer_early_utc = unix_days_from_civil(2026, 6, 1) * 86_400 + 2 * 3600;
        assert_eq!(
            date_in_time_zone(summer_early_utc, "UTC").as_deref(),
            Some("2026-06-01")
        );
        assert_eq!(
            date_in_time_zone(summer_early_utc, "America/New_York").as_deref(),
            Some("2026-05-31")
        );
        assert_eq!(
            date_in_time_zone(summer_early_utc, "Asia/Tokyo").as_deref(),
            Some("2026-06-01")
        );

        // 2026-01-15T20:00:00Z: 2026-01-15 in UTC and New York (EST, UTC-5),
        // already 2026-01-16 in Asia/Tokyo (UTC+9) and Pacific/Kiritimati (UTC+14).
        let winter_late_utc = unix_days_from_civil(2026, 1, 15) * 86_400 + 20 * 3600;
        assert_eq!(
            date_in_time_zone(winter_late_utc, "UTC").as_deref(),
            Some("2026-01-15")
        );
        assert_eq!(
            date_in_time_zone(winter_late_utc, "America/New_York").as_deref(),
            Some("2026-01-15")
        );
        assert_eq!(
            date_in_time_zone(winter_late_utc, "Asia/Tokyo").as_deref(),
            Some("2026-01-16")
        );
        assert_eq!(
            date_in_time_zone(winter_late_utc, "Pacific/Kiritimati").as_deref(),
            Some("2026-01-16")
        );
        assert_eq!(
            date_in_time_zone(winter_late_utc, "Pacific/Honolulu").as_deref(),
            Some("2026-01-15")
        );

        // Rejects path traversal and nonexistent zones.
        assert_eq!(date_in_time_zone(winter_late_utc, "../etc/passwd"), None);
        assert_eq!(
            date_in_time_zone(winter_late_utc, "Mars/Olympus_Mons"),
            None
        );

        // POSIX TZ footer evaluation for fixed and DST rules (north and south).
        assert_eq!(
            posix_tz_offset_at("<+14>-14", winter_late_utc),
            Some(14 * 3600)
        );
        assert_eq!(
            posix_tz_offset_at("HST10", winter_late_utc),
            Some(-10 * 3600)
        );
        assert_eq!(
            posix_tz_offset_at("EST5EDT,M3.2.0,M11.1.0", summer_early_utc),
            Some(-4 * 3600)
        );
        assert_eq!(
            posix_tz_offset_at("EST5EDT,M3.2.0,M11.1.0", winter_late_utc),
            Some(-5 * 3600)
        );
        assert_eq!(
            posix_tz_offset_at("AEST-10AEDT,M10.1.0,M4.1.0/3", winter_late_utc),
            Some(11 * 3600)
        );
        assert_eq!(
            posix_tz_offset_at("AEST-10AEDT,M10.1.0,M4.1.0/3", summer_early_utc),
            Some(10 * 3600)
        );
    }

    #[test]
    fn parses_macos_and_linux_zoneinfo_links_and_unix_dates() {
        assert_eq!(
            parse_zoneinfo_path(Path::new("/var/db/timezone/zoneinfo/America/Los_Angeles"))
                .as_deref(),
            Some("America/Los_Angeles")
        );
        assert_eq!(
            parse_zoneinfo_path(Path::new("../usr/share/zoneinfo/posix/Europe/London")).as_deref(),
            Some("Europe/London")
        );
        assert_eq!(parse_zoneinfo_path(Path::new("/etc/localtime")), None);
        assert_eq!(civil_date_from_unix_days(0), "1970-01-01");
        assert_eq!(civil_date_from_unix_days(20_605), "2026-06-01");
        assert_eq!(unix_days_from_civil(1970, 1, 1), 0);
        assert_eq!(unix_days_from_civil(2026, 6, 1), 20_605);
    }
}
