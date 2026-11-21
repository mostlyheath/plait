//! Scene snapshots of the root view, rendered by GPUI's headless renderers.
//!
//! No window opens: GPUI's test platform lays out and paints the real root
//! view, and a headless renderer draws the scene into an offscreen texture.
//! On macOS that is Metal with Core Text, the renderer and text system the app
//! uses. On Linux it is wgpu on a software Vulkan driver with Linux fonts, so
//! those images check layout, density, and color only.
//!
//! Neither platform exercises a native window, real input events, or the
//! accessibility tree an assistive client reads.
//!
//! ```sh
//! bash scripts/snapshots.sh /tmp/plait-snapshots
//! ```
//!
//! To add a scene, put its sample state in a small function beside the others,
//! then call `harness.capture("name-{suffix}")` from `walk` or a `*_walk`
//! helper. Every scene runs at both window sizes.

use super::{
    Plait,
    composer::Composer,
    model_picker::ModelPurpose,
    settings::{SettingsPage, SettingsTab},
    theme::Assets,
};
use gpui::{prelude::*, *};
use plait_core::host::harness::HarnessId;
use plait_core::{
    host::{
        Approval, ApprovalKind, ApprovalMode, HostError, Message, MessageRole, Model, ModelChoice,
        Snapshot, Status, ThreadStatus, ThreadSummary, Tool, ToolStatus,
    },
    model::DiffMode,
    state::Load,
};
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(debug_assertions)]
mod a11y;
mod admission;
mod branches;
mod find;
mod isolation;
mod many_drafts;
mod mcp;
mod messages;
mod mixed_tabs;
mod settings_modal;
mod showcase;
mod storage;
mod tabs;
mod turn_review;
mod undo;
mod ways_out;
mod worktrees;
mod worktrees_view;

const THREAD: &str = "thr_5791c2e4";

/// Set in the child process that draws scenes. Names its scratch home.
const SCENE_HOME: &str = "PLAIT_SCENE_HOME";

/// Whether this process may build the root view.
///
/// `Plait::new` probes the local host socket and reads desktop preferences,
/// both found through `HOME`. On a developer's machine those belong to a
/// running Plait, so the test process itself never draws. It re-runs the named
/// test in a child whose home and host store are empty scratch directories and
/// reports the child's result. Only that child returns `true`.
fn isolated(test: &str) -> bool {
    if let Some(home) = std::env::var_os(SCENE_HOME) {
        assert_eq!(std::env::var_os("HOME").as_ref(), Some(&home), "scene home");
        assert!(std::env::var_os("PLAIT_HOST_STORE").is_some(), "host store");
        // A preferences file named in the environment is outside the scratch
        // home, and a test that saves a preference would write it.
        let preferences = plait_core::preferences::default_path();
        assert!(
            preferences
                .as_ref()
                .is_some_and(|path| path.starts_with(&home)),
            "preferences are outside the scratch home: {preferences:?}"
        );
        return true;
    }
    // Unix sockets have a short pathname limit, including the temporary home.
    let home = PathBuf::from("/tmp").join(format!("plait-scene-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).expect("scratch home");
    // libtest names omit the crate: `ui::snapshot::<test>`.
    let module = module_path!().split_once("::").expect("module path").1;
    let output = Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", &format!("{module}::{test}")])
        .args(["--include-ignored", "--test-threads=1", "--nocapture"])
        .env(SCENE_HOME, &home)
        .env("HOME", &home)
        .env("PLAIT_HOST_STORE", home.join("host"))
        .env("PLAIT_DESKTOP_STATE_DIR", home.join("desktop"))
        // Both would point the preferences outside the scratch home.
        .env_remove("PLAIT_DESKTOP_PREFERENCES_FILE")
        .env_remove("XDG_CONFIG_HOME")
        .output()
        .expect("re-run the test binary");
    let _ = std::fs::remove_dir_all(&home);
    let stdout = String::from_utf8_lossy(&output.stdout);
    // A filter that matched nothing also exits successfully.
    assert!(
        output.status.success() && stdout.contains("1 passed"),
        "isolated run of {test} failed:\n{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

/// Reads the one request on a connection a scripted host just accepted, or
/// `None` when its client left without sending one.
///
/// A client can be gone before it is served: the build-output watch closes as
/// soon as its creation ends. macOS then refuses the read timeout with EINVAL.
/// That is not the host's failure, and a read from a departed client cannot
/// block, so the request it sent is still read and the host keeps serving.
fn scripted_request(
    stream: &mut std::os::unix::net::UnixStream,
    patience: Duration,
) -> Option<serde_json::Value> {
    use std::io::BufRead;
    // An accepted socket can inherit the listener's non-blocking mode.
    stream.set_nonblocking(false).ok()?;
    let _ = stream.set_read_timeout(Some(patience));
    let mut line = String::new();
    std::io::BufReader::new(stream).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

#[core::prelude::v1::test]
fn a_scripted_host_outlives_a_client_that_left() {
    use std::io::Write;
    use std::os::unix::net::{UnixListener, UnixStream};
    // Unix sockets have a short pathname limit.
    let path = PathBuf::from("/tmp").join(format!("plait-scripted-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    // Gone without a request, gone after one, and still waiting for a reply.
    drop(UnixStream::connect(&path).unwrap());
    let mut sent = UnixStream::connect(&path).unwrap();
    writeln!(sent, "{{\"op\":\"watch_build_output\"}}").unwrap();
    drop(sent);
    let mut waiting = UnixStream::connect(&path).unwrap();
    writeln!(waiting, "{{\"op\":\"submit\"}}").unwrap();
    let next = || {
        let (mut stream, _) = listener.accept().unwrap();
        scripted_request(&mut stream, Duration::from_secs(3))
    };
    assert_eq!(next(), None);
    assert_eq!(next().unwrap()["op"], "watch_build_output");
    assert_eq!(next().unwrap()["op"], "submit");
    std::fs::remove_file(path).unwrap();
}

/// Real text shaping for the headless window, with the app's own fonts.
#[cfg(target_os = "macos")]
fn text_system() -> Arc<dyn PlatformTextSystem> {
    // Core Text, as in the app. The Mac platform needs the main thread; this does not.
    Arc::new(gpui_apple::AppleTextSystem::new())
}

/// Real text shaping for the headless window, with the fonts installed here.
#[cfg(target_os = "linux")]
fn text_system() -> Arc<dyn PlatformTextSystem> {
    // Without a display server the Linux platform has no text system.
    // Metrics differ from macOS.
    Arc::new(gpui_wgpu::CosmicTextSystem::new("DejaVu Sans"))
}

fn git(repository: &Path, arguments: &[&str]) {
    let status = Command::new("git")
        // Keep fixture commits from starting detached maintenance during later checks.
        .args([
            "-c",
            "maintenance.auto=false",
            "-c",
            "gc.auto=0",
            "-c",
            "receive.autoGC=false",
        ])
        .arg("-C")
        .arg(repository)
        .args(arguments)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_DATE", "2030-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2030-01-01T00:00:00Z")
        .status()
        .expect("git is required for snapshots");
    assert!(status.success(), "git {arguments:?} failed");
}

/// A disposable repository with staged, unstaged, untracked, and deleted files.
fn fixture_repository(root: &Path) -> PathBuf {
    let repository = root.join("plait");
    std::fs::create_dir_all(repository.join("src")).unwrap();
    git(&repository, &["init", "-q", "-b", "main"]);
    git(&repository, &["config", "user.name", "Plait Snapshot"]);
    git(
        &repository,
        &["config", "user.email", "snapshot@example.invalid"],
    );
    git(&repository, &["config", "commit.gpgsign", "false"]);
    let review = repository.join("src/review.rs");
    std::fs::write(&review, "pub fn refresh() {\n    read_status();\n}\n").unwrap();
    std::fs::write(repository.join("README.md"), "# Fixture\n").unwrap();
    git(&repository, &["add", "."]);
    git(&repository, &["commit", "-qm", "fixture"]);
    // A local bare remote, so the branch has an upstream and one commit to push.
    git(root, &["init", "-q", "--bare", "-b", "main", "remote.git"]);
    let remote = root.join("remote.git");
    git(
        &repository,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&repository, &["push", "-q", "-u", "origin", "main"]);
    std::fs::write(
        repository.join("README.md"),
        "# Fixture\n\nRead in the background.\n",
    )
    .unwrap();
    git(&repository, &["commit", "-qam", "Describe the fixture"]);
    std::fs::write(&review, "pub fn refresh() {\n    queue_status();\n}\n").unwrap();
    git(&repository, &["add", "src/review.rs"]);
    std::fs::write(
        &review,
        "pub fn refresh() {\n    queue_status();\n    notify_when_ready();\n}\n",
    )
    .unwrap();
    std::fs::write(repository.join("notes.md"), "# Read-only review\n").unwrap();
    std::fs::remove_file(repository.join("README.md")).unwrap();
    repository.canonicalize().unwrap()
}

fn choice() -> ModelChoice {
    ModelChoice {
        provider: "openai-codex".into(),
        model_id: "gpt-5.6-luna".into(),
    }
}

fn status() -> Status {
    let model = |provider: &str, model_id: &str, name: &str, authenticated| Model {
        provider: provider.into(),
        model_id: model_id.into(),
        name: name.into(),
        authenticated,
        ..Default::default()
    };
    Status {
        protocol_version: 2,
        default_model: Some(choice()),
        models: vec![
            model("openai-codex", "gpt-5.6-luna", "GPT-5.6 Luna", true),
            model("openai-codex", "gpt-5.6-sol", "GPT-5.6 Sol", true),
            model("openai-codex", "gpt-6-astra", "GPT-6 Astra", true),
            model(
                "openrouter",
                "anthropic/claude-fable-5",
                "Anthropic: Claude Fable 5",
                true,
            ),
            model(
                "openrouter",
                "anthropic/claude-opus-latest",
                "Anthropic: Claude Opus Latest",
                true,
            ),
            model("google", "gemini-3-pro", "Gemini 3 Pro", false),
        ]
        .into_iter()
        // Enough catalog for the picker to scroll: providers that are not signed in.
        .chain((0..36).map(|index| {
            model(
                &format!("sample-provider-{}", index / 6 + 1),
                &format!("sample-model-{}", index + 1),
                &format!("Sample Model {}", index + 1),
                false,
            )
        }))
        .collect(),
    }
}

fn other_choice() -> ModelChoice {
    ModelChoice {
        provider: "openrouter".into(),
        model_id: "anthropic/claude-opus-latest".into(),
    }
}

fn thread_snapshot(checkout: &Path, approval: bool) -> Snapshot {
    let message = |id: &str, role, text: &str| Message {
        id: id.into(),
        role,
        text: text.into(),
        ..Default::default()
    };
    Snapshot {
        id: THREAD.into(),
        checkout: checkout.to_owned(),
        model: choice(),
        status: if approval {
            ThreadStatus::AwaitingApproval
        } else {
            ThreadStatus::Running
        },
        messages: vec![
            message(
                "m1",
                MessageRole::User,
                "Queue the status refresh instead of reading it inline, then run the tests.",
            ),
            message(
                "m2",
                MessageRole::Assistant,
                "## Planned change\n\nI’ll change `refresh()` to **queue the read**, then notify when it is ready.\n\n```rust\npub fn refresh() {\n    queue_status();\n    notify_when_ready();\n}\n```\n\nThe edit is limited to `src/review.rs`.",
            ),
            message("m3", MessageRole::User, "Go ahead."),
        ],
        live_text: if approval {
            String::new()
        } else {
            "The edit is applied. Running the tests in the sandbox now".into()
        },
        tools: vec![
            Tool {
                id: "t1".into(),
                name: "edit".into(),
                status: ToolStatus::Done,
                input: "{\"path\":\"src/review.rs\",\"old_text\":\"    read_status();\",\"new_text\":\"    queue_status();\\n    notify_when_ready();\"}".into(),
                output: "Replaced 1 line.".into(),
                after: Some(Some("m3".into())),
            },
            Tool {
                id: "t2".into(),
                name: "command".into(),
                status: ToolStatus::Running,
                input: "{\"command\":\"cargo test\"}".into(),
                output: String::new(),
                after: Some(Some("m3".into())),
            },
        ],
        pending_approval: approval.then(|| Approval {
            id: "a1".into(),
            tool_id: "t2".into(),
            kind: ApprovalKind::Command,
            summary: "Run cargo test".into(),
            details: "{\"command\":\"cargo\",\"args\":[\"test\"],\"cwd\":\".\"}".into(),
            dirty_paths: vec!["notes.md".into()],
            risk: "Runs in the command sandbox with a read-only checkout and no network.".into(),
        }),
        error: None,
        truncated: false,
        approval_mode: Some(if approval {
            ApprovalMode::Ask
        } else {
            ApprovalMode::Auto
        }),
        ..Default::default()
    }
}

/// One sample tool call, ordered after the last sample message.
fn tool(id: &str, name: &str, status: ToolStatus, input: &str, output: &str) -> Tool {
    Tool {
        id: id.into(),
        name: name.into(),
        status,
        input: input.into(),
        output: output.into(),
        after: Some(Some("m3".into())),
    }
}

/// Finished tool calls in each end state, on a thread the host stopped with an error.
fn tool_states(snapshot: &mut Snapshot) {
    snapshot.status = ThreadStatus::Error;
    snapshot.live_text.clear();
    snapshot.tools = vec![
        tool(
            "t1",
            "edit",
            ToolStatus::Done,
            "{\"path\":\"src/review.rs\",\"old_text\":\"    read_status();\",\"new_text\":\"    queue_status();\\n    notify_when_ready();\"}",
            "Replaced 1 line.",
        ),
        tool(
            "t2",
            "command",
            ToolStatus::Done,
            "{\"command\":\"cargo test\"}",
            "Sample test output: 24 tests passed.",
        ),
        tool(
            "t3",
            "command",
            ToolStatus::Error,
            "{\"command\":\"cargo clippy -- -D warnings\"}",
            "Sample error output: exit status 101.",
        ),
        tool(
            "t4",
            "edit",
            ToolStatus::Interrupted,
            "{\"path\":\"notes.md\",\"old_text\":\"# Read-only review\",\"new_text\":\"# Review\"}",
            "",
        ),
    ];
    snapshot.error = Some(HostError {
        code: "sample_error".into(),
        message: "Sample host error. This is a rendering fixture, not an agent run.".into(),
    });
    snapshot.truncated = true;
}

/// The sample turn completed: nothing is streaming and no tool is running.
fn finished_turn(snapshot: &mut Snapshot) {
    snapshot.status = ThreadStatus::Idle;
    snapshot.live_text.clear();
    snapshot.tools[1].status = ToolStatus::Done;
    snapshot.tools[1].output = "Sample test output: 24 tests passed.".into();
    snapshot.messages.push(Message {
        id: "m4".into(),
        role: MessageRole::Assistant,
        text: "Sample result. The refresh is queued and the tests pass.\n\n- `src/review.rs` queues the read\n- 24 tests passed".into(),
        ..Default::default()
    });
}

/// Sample replies that exercise every kind of conversation text.
const PROSE: &str = "## Sample reading fixture\n\nThis is a rendering fixture, not an agent run. See [the GPUI site](https://gpui.rs/) or https://example.com/docs for more, with ~~struck~~ text, *emphasis*, **strong**, and `inline code`.\n\n1. First step\n   - nested point that is long enough to wrap onto a second line at the narrow window size so the hanging indent is visible\n     - deeper point\n2. Second step\n\n   A paragraph that belongs to the second step.\n\n- [x] finished task\n- [ ] open task\n\n> A quoted remark.\n\n| Crate | Role | Lines |\n| :-- | :-- | --: |\n| `plait` | GPUI presentation and input | 6200 |\n| `plait-core` | Models, **projection**, and Git | 5400 |\n| `plait-host` | Host lifetime | 1300 |\n\n---\n\nDone.";

const CODE: &str = "```rust\n// Sample code\npub fn refresh<'a>(queue: &'a mut Queue) -> Result<usize, Error> {\n    let count = queue.len() + 0x1f;\n    println!(\"queued {count}\");\n    Ok(count)\n}\n```\n\n```typescript\nexport async function load(path: string): Promise<Thread[]> {\n  const reply = await fetch(`/threads/${path}`); // sample\n  return reply.ok ? reply.json() : [];\n}\n```\n\n```python\ndef total(rows: list[int]) -> int:\n    \"\"\"Sample docstring.\"\"\"\n    return sum(row for row in rows if row > 0)  # positive only\n```\n\n```sh\nCARGO_TERM_COLOR=never cargo test --locked | tee \"$TMPDIR/test.log\" # sample\n```";

const DATA: &str = "```json\n{\"name\": \"plait\", \"private\": true, \"version\": 3}\n```\n\n```toml\n[workspace.package]\nedition = \"2024\" # sample\n```\n\n```yaml\nsteps:\n  - run: \"cargo test\"\n    retries: 2\n```\n\n```diff\n@@ -1,3 +1,3 @@\n-    read_status();\n+    queue_status();\n```\n\n```go\nfunc Sum(xs []int) int { return len(xs) } // sample\n```\n\n```cpp\n#include <vector>\nint main() { std::vector<int> v{1, 2}; return 0; }\n```\n\n```\nplain text with no language\n```\n\n| Sample | One | Two | Three | Four | Five | Six | Seven |\n| - | - | - | - | - | - | - | - |\n| A wide table scrolls sideways | 1 | 2 | 3 | 4 | 5 | 6 | A last cell that is long enough to wrap inside its column when it reaches the width limit |";

/// A finished thread whose replies cover prose, tables, links, and code.
fn reading(snapshot: &mut Snapshot) {
    snapshot.status = ThreadStatus::Idle;
    snapshot.live_text.clear();
    snapshot.tools.clear();
    let message = |id: &str, role, text: &str| Message {
        id: id.into(),
        role,
        text: text.into(),
        ..Default::default()
    };
    snapshot.messages = vec![
        message(
            "r1",
            MessageRole::User,
            "Show the sample fixture.\nIt links to https://example.com/start too.",
        ),
        message("r2", MessageRole::Assistant, PROSE),
        message("r3", MessageRole::User, "And the code samples."),
        message("r4", MessageRole::Assistant, CODE),
        message("r5", MessageRole::Assistant, DATA),
    ];
}

/// One finished call of every tool, a failed command, and arguments the host clipped.
fn tool_cards(snapshot: &mut Snapshot) {
    snapshot.status = ThreadStatus::Idle;
    snapshot.live_text.clear();
    let listing: String = (1..=30)
        .map(|line| format!("pub fn sample_{line}() {{}}\n"))
        .collect();
    snapshot.tools = vec![
        tool(
            "c1",
            "read",
            ToolStatus::Done,
            "{\"path\":\"src/review.rs\"}",
            &listing,
        ),
        tool(
            "c2",
            "edit",
            ToolStatus::Done,
            "{\"path\":\"src/review.rs\",\"old_text\":\"pub fn refresh() {\\n    read_status();\\n}\",\"new_text\":\"pub fn refresh() {\\n    queue_status();\\n    notify_when_ready();\\n}\"}",
            "Replaced 1 occurrence.",
        ),
        tool(
            "c3",
            "write",
            ToolStatus::Done,
            "{\"path\":\"notes/plan.toml\",\"content\":\"# Sample file\\n[plan]\\nsteps = 3\\n\"}",
            "Created notes/plan.toml.",
        ),
    ];
    snapshot.messages.push(Message {
        id: "m4".into(),
        role: MessageRole::Assistant,
        text: "Sample note between two runs of tool calls.".into(),
        ..Default::default()
    });
    let after = |mut call: Tool| {
        call.after = Some(Some("m4".into()));
        call
    };
    snapshot.tools.extend([
        after(tool(
            "c4",
            "command",
            ToolStatus::Done,
            "{\"command\":\"cargo test --locked\"}",
            "Sample output\nrunning 24 tests\ntest result: ok. 24 passed; 0 failed",
        )),
        after(tool(
            "c5",
            "command",
            ToolStatus::Error,
            "{\"command\":\"cargo clippy -- -D warnings\"}",
            "Sample error output\nerror: unused variable: `count`\nexit status 101",
        )),
        after(tool(
            "c6",
            "edit",
            ToolStatus::Done,
            "ample clipped arguments\",\"new_text\":\"the start was cut off\"}",
            "Replaced 1 occurrence.",
        )),
    ]);
}

/// A long run of calls: finished and folded, or still running with its latest steps.
fn long_run(snapshot: &mut Snapshot, running: bool) {
    snapshot.status = if running {
        ThreadStatus::Running
    } else {
        ThreadStatus::Idle
    };
    snapshot.live_text.clear();
    let names = ["read", "read", "edit", "command", "read", "edit", "command"];
    snapshot.tools = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let (input, output) = match *name {
                // Long enough to wrap in the row at both window sizes.
                "command" => (
                    "{\"command\":\"cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && git status --short --branch\"}",
                    "Sample output.",
                ),
                "edit" => (
                    "{\"path\":\"src/review.rs\",\"old_text\":\"a\",\"new_text\":\"b\"}",
                    "Replaced 1 occurrence.",
                ),
                _ => ("{\"path\":\"src/review.rs\"}", "pub fn sample() {}\n"),
            };
            let last = index + 1 == names.len();
            let status = match (index, running && last) {
                (_, true) => ToolStatus::Running,
                (3, _) => ToolStatus::Error,
                _ => ToolStatus::Done,
            };
            tool(&format!("g{index}"), name, status, input, output)
        })
        .collect();
    if !running {
        snapshot.messages.push(Message {
            id: "m4".into(),
            role: MessageRole::Assistant,
            text: "Sample result after a long run of tool calls.".into(),
            ..Default::default()
        });
    }
}

/// An unsent message of several lines, as typed into the composer.
const DRAFT: &str =
    "Also notify when the queue is empty.\nKeep the public signature of refresh() as it is.";

// Real Git reads supply the fixture data; elapsed time is not scene content.
fn pin_read_times(repository: &mut plait_core::state::RepositoryState, wide: bool) {
    // Preserve the reference render's measured values, which differed by size.
    const STATUS_WIDE_MS: u64 = 36;
    const STATUS_NARROW_MS: u64 = 39;
    const STAGED_DIFF_MS: u64 = 18;
    const UNSTAGED_DIFF_MS: u64 = 19;
    use plait_core::model::{Diff, DiffLine, Snapshot as GitSnapshot};
    if let Load::Ready(snapshot) = &mut repository.data.snapshot
        && !snapshot.elapsed.is_zero()
    {
        *snapshot = Arc::new(GitSnapshot {
            root: snapshot.root.clone(),
            branch: snapshot.branch.clone(),
            named: snapshot.named,
            tracking: snapshot.tracking.clone(),
            changes: snapshot.changes.clone(),
            elapsed: Duration::from_millis(if wide {
                STATUS_WIDE_MS
            } else {
                STATUS_NARROW_MS
            }),
        });
    }
    let selected = &repository.view.changes.selected;
    let diff_ms = if selected
        .as_ref()
        .is_some_and(|key| key.mode == DiffMode::Unstaged)
    {
        UNSTAGED_DIFF_MS
    } else {
        STAGED_DIFF_MS
    };
    for (load, elapsed_ms) in [
        (&mut repository.view.changes.diff, diff_ms),
        (&mut repository.view.history.patch, STAGED_DIFF_MS),
    ] {
        if let Load::Ready(diff) = load
            && !diff.elapsed.is_zero()
        {
            *diff = Arc::new(Diff {
                lines: diff
                    .lines
                    .iter()
                    .map(|line| DiffLine {
                        text: line.text.clone(),
                        kind: line.kind,
                        old_line: line.old_line,
                        new_line: line.new_line,
                    })
                    .collect(),
                rows: diff.rows.clone(),
                widest_row: diff.widest_row,
                hunks: diff.hunks.clone(),
                truncated: diff.truncated,
                elapsed: Duration::from_millis(elapsed_ms),
            });
        }
    }
}

fn pin_scene_values(plait: &mut Plait, wide: bool, name: &str) {
    pin_read_times(&mut plait.state.repository, wide);
    let total = |message: Option<&Arc<plait_core::markdown::Message>>| {
        message.map_or(0, |message| plait_core::reveal::total(&message.blocks))
    };
    let text = match name {
        "message-roles-streaming-wide" | "message-roles-streaming-narrow" => "I’ll".len(),
        "palette-no-matches-narrow" => "The edit is applied. Running the tests in the sandb".len(),
        _ => total(plait.workspace.formatted.live.as_ref()),
    };
    let reasoning = total(plait.workspace.formatted.live_reasoning.as_ref());
    plait
        .workspace
        .reveal
        .set_scene_prefixes(THREAD, text, reasoning);
    plait.last_render_us = 320;
    plait.max_render_us = 640;
    plait.switch_us = 480;
    plait.first_render_ms = Some(40);
    plait.interaction = None;
}

struct Harness {
    cx: HeadlessAppContext,
    window: WindowHandle<Plait>,
    /// Where to write PNG files. `None` draws every scene without a renderer.
    output: Option<PathBuf>,
}

impl Harness {
    fn new(repository: &Path, output: Option<&Path>, width: f32, height: f32) -> Self {
        let text_system = text_system();
        let render = output.is_some();
        let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(Assets), move || {
            if render {
                gpui_platform::current_headless_renderer()
            } else {
                Ok(None)
            }
        });
        cx.allow_parking();
        // As main does, so scenes show the shortcuts the palette reads from the keymap.
        cx.update(super::menus);
        let path = repository.to_owned();
        let window = cx
            .open_window(size(px(width), px(height)), |window, cx| {
                cx.new(|cx| Plait::new(path, Instant::now(), window, cx))
            })
            .expect("headless window");
        let mut harness = Self {
            cx,
            window,
            output: output.map(Path::to_owned),
        };
        harness.settle();
        harness
    }

    /// Run queued work until the Git reads and the host probe have landed.
    fn settle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.cx.run_until_parked();
            let busy = self
                .window
                .update(&mut self.cx, |plait, _, _| {
                    let repository = &plait.state.repository;
                    matches!(repository.snapshot(), Load::Loading)
                        || matches!(repository.diff(), Load::Loading)
                        || plait.host.connection.connecting
                        || plait.workspace.file_viewer.loading
                })
                .expect("window update");
            if !busy {
                return;
            }
            assert!(Instant::now() < deadline, "background work did not finish");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn update(&mut self, change: impl FnOnce(&mut Plait, &mut Window, &mut Context<Plait>)) {
        self.window
            .update(&mut self.cx, |plait, window, cx| {
                change(plait, window, cx);
                plait.refresh_tab_rows();
                cx.notify();
            })
            .expect("window update");
        self.settle();
    }

    fn change_snapshot(&mut self, change: impl FnOnce(&mut Snapshot)) {
        self.update(|plait, _, cx| {
            let previous = plait.host.thread.snapshot.clone().expect("sample snapshot");
            change(Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap()));
            plait.workspace.formatted = Arc::new(plait_core::markdown::Transcript::update(
                &plait.workspace.formatted,
                plait.host.thread.snapshot.as_ref().unwrap(),
            ));
            plait.refresh_live_list(Some(&previous));
            plait.conversation_rows_changed(cx);
            let snapshot = plait.host.thread.snapshot.as_ref().unwrap();
            if let Some(thread) = plait
                .host
                .connection
                .threads
                .iter_mut()
                .find(|thread| thread.id == snapshot.id)
            {
                thread.status = snapshot.status;
            }
            plait.sync_composer(cx);
        });
    }

    /// Deliver one pointer event to the window, as the platform would. Handlers
    /// belong to the last drawn frame, so draw with `capture` first.
    fn pointer(&mut self, event: PlatformInput) {
        let window = self.window.into();
        self.cx
            .update_window(window, |_, window, cx| {
                window.dispatch_event(event, cx);
            })
            .expect("pointer event");
        self.settle();
    }

    /// Press one key combination, dispatched from the focused element.
    fn keystroke(&mut self, keys: &'static str) {
        let keystroke = Keystroke::parse(keys).expect("keystroke");
        let window = self.window.into();
        self.cx
            .update_window(window, |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            })
            .expect("keystroke");
        self.settle();
    }

    /// Deliver the platform's repeat event, rather than another fresh press.
    fn held_keystroke(&mut self, keys: &'static str) {
        self.pointer(PlatformInput::KeyDown(KeyDownEvent {
            keystroke: Keystroke::parse(keys).expect("keystroke"),
            is_held: true,
            prefer_character_input: false,
        }));
    }

    /// Run layout and paint without saving an image.
    fn draw(&mut self) {
        let window = self.window.into();
        self.cx
            .update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .expect("draw");
    }

    /// Run layout and paint with the debug accessibility collector active.
    #[cfg(debug_assertions)]
    fn draw_a11y(&mut self) -> super::a11y::A11yFrame {
        let (_, frame) = super::a11y::collect(|| self.draw());
        frame
    }

    /// Draw the scene, which runs layout and paint, and save it when rendering.
    fn capture(&mut self, name: &str) {
        let Some(output) = &self.output else {
            #[cfg(debug_assertions)]
            self.draw_a11y().assert_no_duplicates(name);
            #[cfg(not(debug_assertions))]
            self.draw();
            return;
        };
        let handle = self.window.into();
        self.cx
            .update_window(handle, |view, window, cx| {
                view.downcast::<Plait>()
                    .expect("scene root")
                    .update(cx, |plait, _| {
                        pin_scene_values(
                            plait,
                            f32::from(window.viewport_size().width) > 1000.,
                            name,
                        );
                    });
                // Notifying here would draw once before this explicit frame and
                // replace the pinned diagnostics with that draw's measurements.
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .expect("draw scene");
        let image = self.cx.capture_screenshot(handle).expect("screenshot");
        image
            .save(output.join(format!("{name}.png")))
            .expect("write snapshot");
    }

    /// Present a connected host without a socket. Labeled sample data only.
    fn connect(&mut self, repository: &Path, approval: bool) {
        let checkout = repository.to_owned();
        self.update(move |plait, window, cx| {
            plait.host.connection.connecting = false;
            plait.host.connection.connected = true;
            plait.host.connection.error = None;
            plait.host.connection.status = Some(status());
            plait.host.connection.settings_model = Some(choice());
            plait.host.creation.new_model = Some(choice());
            plait.host.connection.threads = vec![
                ThreadSummary {
                    checkout_queue: None,
                    id: THREAD.into(),
                    checkout: checkout.clone(),
                    model: choice(),
                    status: ThreadStatus::Running,
                    ..Default::default()
                },
                ThreadSummary {
                    checkout_queue: None,
                    id: "thr_0c44a9d1".into(),
                    checkout: checkout.clone(),
                    model: choice(),
                    status: ThreadStatus::Idle,
                    ..Default::default()
                },
            ];
            plait.tabs = Default::default();
            assert!(plait.open_thread_tab(THREAD.into(), checkout.clone(), cx));
            plait.refresh_tab_rows();
            // Mirror select_host_thread without opening a socket.
            plait.host.thread.selected = Some(THREAD.into());
            plait.workspace.history = Default::default();
            plait.workspace.restore = None;
            plait.workspace.restore_pending = [false; 4];
            plait.host.thread.snapshot = Some(Arc::new(thread_snapshot(&checkout, approval)));
            plait.workspace.formatted = Arc::new(plait_core::markdown::Transcript::update(
                &plait.workspace.formatted,
                plait.host.thread.snapshot.as_ref().unwrap(),
            ));
            plait.settings_open = false;
            plait.settings_page = SettingsPage::default();
            plait.new_thread_open = false;
            plait.draft = Default::default();
            plait.workspace.tab = super::Workspace::Conversation;
            let composer = cx.new(|cx| Composer::new(window, cx));
            plait.workspace.composers.insert(THREAD.into(), composer);
            plait.refresh_live_list(None);
            plait
                .session
                .data
                .get_or_insert_with(|| plait_core::session::Session::empty("/fixture-host".into()));
            plait.capture_session(cx);
            plait.sync_composer(cx);
        });
    }
}

impl Harness {
    /// Let go of every key, as the platform reports it.
    fn release(&mut self) {
        self.pointer(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
            modifiers: Modifiers::default(),
            capslock: Capslock::default(),
        }));
    }

    /// Give the pending approval a new ID, so an earlier answer does not cover it.
    fn new_approval(&mut self, id: &'static str) {
        self.change_snapshot(|snapshot| snapshot.pending_approval.as_mut().unwrap().id = id.into());
        self.update(|plait, _, _| plait.host.connection.error = None);
    }

    /// Press `keys` from a released keyboard and report whether an answer to
    /// `id` was sent. No host runs here, so a sent answer fails and says so.
    fn answers(&mut self, keys: &'static str, id: &str) -> bool {
        self.release();
        self.press(keys, id)
    }

    /// Press `keys` without injecting release or modifier events.
    fn press(&mut self, keys: &'static str, id: &str) -> bool {
        self.approval_press(keys, id, false)
    }

    fn approval_press(&mut self, keys: &'static str, id: &str, is_held: bool) -> bool {
        self.draw();
        if is_held {
            self.held_keystroke(keys);
        } else {
            self.keystroke(keys);
        }
        let id = id.to_owned();
        self.window
            .update(&mut self.cx, |plait, _, _| {
                let sent = plait.host.thread.decided_approvals.contains(&id);
                assert_eq!(
                    sent,
                    plait.host.connection.error.is_some(),
                    "a sent answer fails here"
                );
                sent
            })
            .expect("window update")
    }
}

/// Deny Once and Allow Once answer from the keyboard exactly when the
/// approval card's buttons would, once per press.
#[core::prelude::v1::test]
fn approval_shortcuts_act_only_when_the_buttons_would() {
    if !isolated("approval_shortcuts_act_only_when_the_buttons_would") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-approval-keys-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, true);
    harness.update(|plait, _, cx| {
        for key in ["cmd-alt-y", "cmd-alt-."] {
            let key = Keystroke::parse(key).unwrap();
            assert!(
                !super::actions::editing_keys(cx).contains(&key),
                "the composer does not take {key:?}"
            );
        }
        // The buttons read the keymap.
        assert_eq!(
            plait.shortcut_label("approval.allow_once"),
            keys("cmd-alt-y")
        );
        assert_eq!(
            plait.shortcut_label("approval.deny_once"),
            keys("cmd-alt-.")
        );
        // One gate decides for the buttons and the keys.
        let approval = plait
            .host
            .thread
            .snapshot
            .as_ref()
            .unwrap()
            .pending_approval
            .clone()
            .unwrap();
        assert!(plait.can_answer(&approval));
        assert_eq!(plait.shortcut_answer(true), Some(("a1".into(), true)));
        assert_eq!(plait.shortcut_answer(false), Some(("a1".into(), false)));
    });
    assert!(harness.answers("cmd-alt-y", "a1"), "Allow Once answers");
    harness.new_approval("a2");
    assert!(harness.answers("cmd-alt-.", "a2"), "Deny Once answers");

    // A held key answers one approval, not the next.
    harness.new_approval("a3");
    assert!(
        !harness.approval_press("cmd-alt-.", "a3", true),
        "a repeat is ignored"
    );
    assert!(harness.answers("cmd-alt-.", "a3"), "a new press answers");
    harness.new_approval("a4");
    // A delivered key-up is harmless; the next fresh press still answers.
    let key_up = Keystroke::parse("cmd-alt-y").unwrap();
    harness.pointer(PlatformInput::KeyUp(KeyUpEvent { keystroke: key_up }));
    assert!(harness.press("cmd-alt-y", "a4"));

    harness.new_approval("a4-next");
    assert!(
        harness.press("cmd-alt-y", "a4-next"),
        "a fresh press needs no release event"
    );

    // A key held from before the approval arrived does not answer it.
    harness.change_snapshot(|snapshot| snapshot.pending_approval = None);
    harness.update(|plait, _, _| plait.host.connection.error = None);
    assert!(!harness.answers("cmd-alt-y", "a5"), "no approval waits");
    harness.change_snapshot(|snapshot| {
        let mut approval = thread_snapshot(Path::new("/"), true)
            .pending_approval
            .unwrap();
        approval.id = "a5".into();
        snapshot.pending_approval = Some(approval);
    });
    assert!(
        !harness.approval_press("cmd-alt-y", "a5", true),
        "the held key stays spent"
    );

    // Everything that disables the buttons stops the keys.
    harness.update(|plait, _, _| plait.host.connection.connected = false);
    assert!(!harness.answers("cmd-alt-y", "a5"), "disconnected");
    harness.update(|plait, _, _| {
        plait.host.connection.connected = true;
        plait.host.thread.controlling = true;
    });
    assert!(
        !harness.answers("cmd-alt-y", "a5"),
        "a control request is in flight"
    );
    harness.update(|plait, _, _| {
        plait.host.thread.controlling = false;
        plait.host.thread.decided_approvals.insert("a5".into());
    });
    harness.release();
    harness.draw();
    harness.keystroke("cmd-alt-y");
    harness.update(|plait, _, _| {
        assert!(
            plait.host.connection.error.is_none(),
            "already decided: nothing is sent"
        );
        plait.host.thread.decided_approvals.remove("a5");
    });
    // Pi's approval with no arguments shows nothing to approve.
    harness.change_snapshot(|snapshot| snapshot.pending_approval.as_mut().unwrap().details.clear());
    assert!(!harness.answers("cmd-alt-y", "a5"), "not approvable");
    harness.new_approval("a6");
    harness.change_snapshot(|snapshot| {
        snapshot.pending_approval.as_mut().unwrap().details = "{\"command\":\"cargo\"}".into()
    });

    // Only the open thread's card, while it is on screen, can be answered.
    harness.update(|plait, _, _| plait.settings_open = true);
    assert!(
        !harness.answers("cmd-alt-y", "a6"),
        "Settings covers the card"
    );
    harness.update(|plait, _, _| {
        plait.settings_open = false;
        plait.workspace.tab = super::Workspace::Changes;
    });
    assert!(
        !harness.answers("cmd-alt-y", "a6"),
        "Changes covers the card"
    );
    harness.update(|plait, _, _| {
        plait.workspace.tab = super::Workspace::Conversation;
        plait.host.thread.selected = Some("thr_0c44a9d1".into());
    });
    assert!(
        !harness.answers("cmd-alt-y", "a6"),
        "another thread is open"
    );
    harness.update(|plait, _, _| plait.host.thread.selected = Some(THREAD.into()));
    // The palette opens over the card, so the keys wait until it closes.
    harness.update(|plait, window, cx| plait.toggle_command_palette(window, cx));
    harness.update(|plait, _, _| {
        assert!(plait.overlay.is_some(), "the palette is open");
        assert_eq!(plait.shortcut_answer(true), None);
    });
    assert!(
        !harness.answers("cmd-alt-y", "a6"),
        "the palette covers the card"
    );
    harness.update(|plait, window, cx| plait.toggle_command_palette(window, cx));
    harness.update(|plait, _, _| assert!(plait.overlay.is_none(), "the palette is closed"));

    // A rebound key answers, and the button shows it.
    harness.update(|plait, _, cx| {
        let keymap = &mut plait.preferences.value.keymap;
        keymap.insert("approval.allow_once".into(), "cmd-alt-u".into());
        super::actions::install(cx, keymap);
        assert_eq!(
            plait.shortcut_label("approval.allow_once"),
            keys("cmd-alt-u")
        );
    });
    assert!(!harness.answers("cmd-alt-y", "a6"), "the old key is free");
    assert!(harness.answers("cmd-alt-u", "a6"), "the new key answers");
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// AppKit sends a key without Command through text input, and every repeat
/// comes back with `is_held` false. Its key-up does arrive, so only a release
/// lets the key answer again.
#[core::prelude::v1::test]
fn a_rebound_key_without_command_answers_once_while_held() {
    if !isolated("a_rebound_key_without_command_answers_once_while_held") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-held-plain-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, true);
    harness.update(|plait, _, cx| {
        let keymap = &mut plait.preferences.value.keymap;
        keymap.insert("approval.allow_once".into(), "alt-pagedown".into());
        super::actions::install(cx, keymap);
        let shortcuts = super::actions::shortcuts();
        let allow = shortcuts
            .iter()
            .find(|s| s.actions.iter().any(|a| a.id == "approval.allow_once"))
            .unwrap();
        let key = Keystroke::parse("alt-pagedown").unwrap();
        let editing = super::actions::editing_keys(cx);
        assert_eq!(super::actions::refusal(allow, &[key], editing), None);
    });
    let key_up = || {
        PlatformInput::KeyUp(KeyUpEvent {
            keystroke: Keystroke::parse("alt-pagedown").unwrap(),
        })
    };
    assert!(harness.answers("alt-pagedown", "a1"), "a press answers");
    harness.new_approval("a2");
    assert!(
        !harness.press("alt-pagedown", "a2"),
        "a repeat arrives fresh and must not answer"
    );
    harness.pointer(key_up());
    assert!(
        harness.press("alt-pagedown", "a2"),
        "after a release it answers"
    );

    // Held from before the approval arrived.
    harness.pointer(key_up());
    harness.change_snapshot(|snapshot| snapshot.pending_approval = None);
    harness.update(|plait, _, _| plait.host.connection.error = None);
    assert!(!harness.press("alt-pagedown", "a3"), "no approval waits");
    harness.change_snapshot(|snapshot| {
        let mut approval = thread_snapshot(Path::new("/"), true)
            .pending_approval
            .unwrap();
        approval.id = "a3".into();
        snapshot.pending_approval = Some(approval);
    });
    assert!(
        !harness.press("alt-pagedown", "a3"),
        "the held key stays spent"
    );
    // Command shortcuts still need no release between presses.
    harness.new_approval("a4");
    assert!(harness.answers("cmd-alt-.", "a4"));
    harness.new_approval("a5");
    assert!(harness.press("cmd-alt-.", "a5"));
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A saved keymap can give two actions one key: a hand edit, or a new
/// default matching an old override. Only the binding GPUI ranks first acts.
#[core::prelude::v1::test]
fn a_shared_key_runs_only_the_winning_action() {
    if !isolated("a_shared_key_runs_only_the_winning_action") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-shared-key-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, true);
    // Next Changed File keeps its default, Option-Down.
    harness.update(|plait, _, cx| {
        let keymap = &mut plait.preferences.value.keymap;
        keymap.insert("approval.allow_once".into(), "alt-down".into());
        super::actions::install(cx, keymap);
    });
    assert!(
        harness.answers("alt-down", "a1"),
        "the later binding answers, as GPUI ranks it"
    );
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn desktop_recovery_does_not_rebind_saved_work_or_clear_a_newer_revision() {
    if !isolated("desktop_recovery_does_not_rebind_saved_work_or_clear_a_newer_revision") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-recovery-ui-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        let pending = plait
            .host
            .submissions
            .begin(THREAD, "first".into())
            .unwrap();
        plait.workspace.composers[THREAD].update(cx, |composer, cx| {
            composer.restore_draft("first".into(), 7, Some(("first".into(), 6)), cx);
        });
        plait.capture_session(cx);
        let saved = plait.session.data.as_ref().unwrap().threads[THREAD].clone();
        plait.host.connection.threads[0].checkout = "/a-different-checkout".into();
        let snapshot = Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap());
        snapshot.checkout = plait.host.connection.threads[0].checkout.clone();
        snapshot.messages.push(Message {
            id: format!("request-{}", pending.request_id),
            role: MessageRole::User,
            text: "first".into(),
            ..Default::default()
        });
        let composer = cx.new(|cx| Composer::new(window, cx));
        plait.restore_composer(THREAD, &composer, cx);
        assert_eq!(composer.read(cx).saved_draft().0, "");
        plait
            .workspace
            .composers
            .insert(THREAD.into(), composer.clone());
        plait.capture_session(cx);
        plait.record_snapshot(cx);
        assert_eq!(plait.session.data.as_ref().unwrap().threads[THREAD], saved);
        assert!(!plait.session_accepts_thread(THREAD));
        assert!(plait.host.submissions.pending(THREAD).is_some());
        // Only the matching identity can reconcile the accepted request, not a reused thread ID.
        plait.host.connection.threads[0].checkout = saved.checkout.clone();
        Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap()).checkout =
            saved.checkout.clone();
        plait.restore_composer(THREAD, &composer, cx);
        plait.record_snapshot(cx);
        assert!(plait.host.submissions.pending(THREAD).is_none());
        assert_eq!(composer.read(cx).saved_draft().0, "first");
        assert_eq!(composer.read(cx).saved_draft().1, 7);
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Opening a real file, finding in it, noticing changes on disk, and quick open.
#[core::prelude::v1::test]
fn files_tab_reads_finds_and_reloads_a_real_file() {
    use plait_core::file_view::Contents;
    let scratch = std::env::temp_dir().join(format!("plait-files-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    let text = |plait: &Plait| match plait
        .workspace
        .file_viewer
        .opened
        .as_deref()
        .map(|o| &o.contents)
    {
        Some(Contents::Text(text)) => text.text.clone(),
        other => panic!("not text: {other:?}"),
    };
    harness.update(|plait, _, cx| plait.open_file("src/review.rs".into(), Some(2), cx));
    harness.update(|plait, _, _| {
        assert_eq!(plait.workspace.tab, super::Workspace::Files);
        assert!(text(plait).contains("notify_when_ready"));
    });
    harness.draw();
    // Find counts matches off the UI thread and steps through them, wrapping.
    harness.update(|plait, window, cx| {
        plait.open_find(window, cx);
        let field = plait.workspace.file_viewer.find.field.clone().unwrap();
        field.update(cx, |field, cx| field.set_text("STATUS", cx));
    });
    harness.update(|plait, _, cx| {
        assert_eq!(
            plait.workspace.file_viewer.find.matches.len(),
            1,
            "ASCII case is ignored"
        );
        plait.step_find(true, cx);
        assert_eq!(
            plait.workspace.file_viewer.find.current, 0,
            "one match wraps to itself"
        );
    });
    harness.draw();
    // Closing clears the matches; reopening searches the kept query again.
    harness.update(|plait, window, cx| {
        plait.close_find(window, cx);
        assert!(plait.workspace.file_viewer.find.matches.is_empty());
        plait.open_find(window, cx);
    });
    harness.update(|plait, _, cx| {
        let find = &plait.workspace.file_viewer.find;
        assert_eq!(find.field.as_ref().unwrap().read(cx).text(), "STATUS");
        assert_eq!(find.matches.len(), 1, "reopening finds the kept query");
    });
    // A change on disk is noticed by size and time, then reread.
    std::fs::write(repository.join("src/review.rs"), "pub fn refresh() {}\n").unwrap();
    harness.update(|plait, _, cx| plait.check_file(cx));
    harness.update(|plait, _, _| {
        assert_eq!(text(plait), "pub fn refresh() {}\n");
        assert_eq!(
            plait.workspace.file_viewer.note.as_deref(),
            Some("Updated from disk.")
        );
        assert!(
            plait.workspace.file_viewer.find.matches.is_empty(),
            "find reruns on new text"
        );
    });
    // Deleted: the last version stays, with a note.
    std::fs::remove_file(repository.join("src/review.rs")).unwrap();
    harness.update(|plait, _, cx| plait.check_file(cx));
    harness.update(|plait, _, _| {
        assert_eq!(text(plait), "pub fn refresh() {}\n");
        assert!(
            plait
                .workspace
                .file_viewer
                .note
                .as_ref()
                .unwrap()
                .contains("version read before")
        );
    });
    // Letter case is ignored outside ASCII too, as in the conversation.
    std::fs::write(repository.join("notes.txt"), "Café\ncafé CAFÉ\n").unwrap();
    harness.update(|plait, _, cx| plait.open_file("notes.txt".into(), None, cx));
    harness.update(|plait, _, cx| {
        let field = plait.workspace.file_viewer.find.field.clone().unwrap();
        field.update(cx, |field, cx| field.set_text("CAFÉ", cx));
    });
    harness.update(|plait, _, _| {
        assert_eq!(
            *plait.workspace.file_viewer.find.matches,
            vec![0..5, 6..11, 12..17]
        );
    });
    // Binary files are described, not drawn.
    std::fs::write(repository.join("logo.png"), [0x89, b'P', 0, 0]).unwrap();
    harness.update(|plait, window, cx| {
        plait.close_find(window, cx);
        plait.open_file("logo.png".into(), None, cx)
    });
    harness.update(|plait, _, _| {
        assert!(matches!(
            plait
                .workspace
                .file_viewer
                .opened
                .as_deref()
                .unwrap()
                .contents,
            Contents::Binary { len: 4 }
        ));
    });
    harness.draw();
    // Quick open lists the checkout's files once Git has read them.
    harness.update(|plait, window, cx| plait.quick_open(window, cx));
    harness.update(|plait, window, cx| {
        let overlay = plait.overlay.as_ref().expect("quick open");
        assert!(overlay.files && !overlay.palette.read(cx).is_empty());
        plait.close_overlay(true, window, cx);
    });
    let _ = std::fs::remove_dir_all(&scratch);
}

/// The Files tab with a file, with find open, and quick open.
fn files_walk(harness: &mut Harness, suffix: &str) {
    harness.update(|plait, _, cx| plait.open_file("src/review.rs".into(), None, cx));
    harness.capture(&format!("files-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.open_find(window, cx);
        let field = plait.workspace.file_viewer.find.field.clone().unwrap();
        field.update(cx, |field, cx| field.set_text("re", cx));
    });
    // Two matches in "refresh", and the current one in "ready", underlined.
    harness.update(|plait, _, cx| plait.step_find(false, cx));
    harness.update(|plait, _, _| assert_eq!(plait.workspace.file_viewer.find.matches.len(), 3));
    harness.capture(&format!("files-find-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_find(window, cx);
        plait.quick_open(window, cx);
    });
    harness.capture(&format!("quick-open-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.show_tab(super::Workspace::Conversation, window, cx);
    });
}

impl Harness {
    /// Type into the new-thread draft's composer, as the keyboard would.
    fn type_draft(&mut self, text: &'static str) {
        self.window
            .update(&mut self.cx, |plait, window, cx| {
                let composer = plait.draft.composer.clone().expect("an open draft");
                composer.update(cx, |composer, cx| composer.type_text(text, window, cx));
            })
            .expect("window update");
        self.settle();
    }

    /// Wait until no thread creation is in flight; the result stays as the host left it.
    fn creation_settles(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self
            .window
            .update(&mut self.cx, |plait, _, _| plait.draft_creating())
            .expect("window update")
        {
            assert!(Instant::now() < deadline, "creation did not finish");
            std::thread::sleep(Duration::from_millis(5));
            self.settle();
        }
    }
}

fn draft_text(plait: &Plait, cx: &App) -> String {
    plait
        .draft
        .composer
        .as_ref()
        .map(|composer| composer.read(cx).text().to_owned())
        .unwrap_or_default()
}

/// New Thread opens a draft and creates nothing; sending creates the thread,
/// and an unconfirmed creation is retried with the same request ID.
#[core::prelude::v1::test]
fn a_draft_creates_its_thread_only_when_sent_and_keeps_its_request_id() {
    if !isolated("a_draft_creates_its_thread_only_when_sent_and_keeps_its_request_id") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-new-thread-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        let before = window.focused(cx);
        plait.toggle_command_palette(window, cx);
        assert!(plait.overlay.is_some());
        assert!(
            window.focused(cx) != before,
            "the palette takes the keyboard"
        );
        plait.toggle_command_palette(window, cx);
        assert!(plait.overlay.is_none());
        assert!(window.focused(cx) == before, "closing returns focus");
        // Opening the draft creates nothing and leaves the open thread.
        plait.open_draft(window, cx);
        assert!(plait.draft.open && plait.draft_visible());
        assert!(plait.host.thread.selected.is_none());
        assert!(plait.draft.pending.is_none() && !plait.draft_creating());
        let composer = plait.draft.composer.clone().unwrap();
        assert!(
            composer.focus_handle(cx).is_focused(window),
            "ready to type"
        );
        // Without a usable default, sending says why and creates nothing.
        plait.host.connection.status.as_mut().unwrap().default_model = None;
        assert!(plait.draft_blocked().is_some());
    });
    harness.type_draft("Queue the status refresh");
    harness.keystroke("enter");
    harness.update(|plait, _, cx| {
        assert!(plait.draft.pending.is_none() && !plait.draft_creating());
        assert_eq!(draft_text(plait, cx), "Queue the status refresh");
        plait.host.connection.status = Some(status());
        plait.sync_composer(cx);
        assert!(plait.draft_blocked().is_none());
    });
    harness.keystroke("enter");
    let first = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            assert!(plait.draft.pending.is_some(), "sending creates the thread");
            let sending = plait.draft.sending.clone().expect("the message waits");
            assert_eq!(sending.text, "Queue the status refresh");
            assert_eq!(sending.approval, Some(ApprovalMode::Auto));
            // The header cannot change what is being created.
            assert!(plait.draft_locked().is_some());
            many_drafts::pending_choice(plait)
        })
        .expect("window update");
    assert_eq!(first.1, choice());
    // No host runs in this scratch home, so the request fails and stays unconfirmed.
    harness.creation_settles();
    harness.update(|plait, window, cx| {
        assert_eq!(many_drafts::pending_choice(plait), first);
        assert!(plait.draft_pending());
        assert_eq!(draft_text(plait, cx), "Queue the status refresh");
        // The draft shows its own state; the bar above stays for elsewhere.
        assert!(plait.creation_bar(cx).is_none());
        // Another model cannot be chosen while it is unconfirmed.
        plait.choose_draft_model(HarnessId::Pi, other_choice(), cx);
        assert_eq!(plait.draft_model(), Some(choice()));
        // Sending again retries the same request.
        plait.retry_draft(window, cx);
        assert!(plait.draft_creating());
    });
    harness.creation_settles();
    harness.update(|plait, window, cx| {
        assert_eq!(many_drafts::pending_choice(plait), first);
        // Leaving keeps the draft in its own tab. New Thread always starts fresh.
        let draft_tab = plait.tabs.model.active().unwrap();
        plait.select_host_thread(THREAD.into(), window, cx);
        assert!(!plait.draft.open);
        plait.open_draft(window, cx);
        assert_eq!(draft_text(plait, cx), "");
        plait.request_tab(
            super::tab_lifecycle::Intent::Activate(draft_tab),
            window,
            cx,
        );
        assert_eq!(draft_text(plait, cx), "Queue the status refresh");
    });
    // Escape leaves a draft that holds anything as it is.
    harness.keystroke("escape");
    harness.update(|plait, _, cx| {
        assert_eq!(draft_text(plait, cx), "Queue the status refresh");
        assert!(plait.draft.pending.is_some());
        assert_eq!(plait.tabs.model.tabs().len(), 3);
    });
    // Discard removes this tab and returns to its empty neighbour.
    harness.update(|plait, window, cx| plait.discard_draft(window, cx));
    harness.update(|plait, _, cx| {
        assert!(plait.draft.open && plait.draft.sending.is_none());
        assert_eq!(draft_text(plait, cx), "");
        assert!(plait.draft.pending.is_none());
        assert_eq!(plait.tabs.model.tabs().len(), 2);
        assert_eq!(plait.tabs.model.closed_len(), 0);
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// The draft's thinking chip lists only its model's levels, starts at
/// Settings' default moved to the nearest of them, carries a choice across
/// model changes, and sends the chosen level, the same one on a retry.
#[core::prelude::v1::test]
fn a_draft_sends_its_thinking_level_and_repeats_it_on_retry() {
    use plait_core::host::ThinkingLevel::*;
    if !isolated("a_draft_sends_its_thinking_level_and_repeats_it_on_retry") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-draft-thinking-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        let status = plait.host.connection.status.as_mut().unwrap();
        status.protocol_version = 4;
        let model = status
            .models
            .iter_mut()
            .find(|m| m.choice() == choice())
            .unwrap();
        model.reasoning = Some(true);
        model.thinking_levels = Some(vec![Off, Low, High]);
        plait.inject_harnesses(Some(claude_harnesses("ready", None)));
        plait.open_draft(window, cx);
        // Medium, the unset default, is as near Low as High: the lower one.
        assert_eq!(plait.preferences.value.default_thinking(), Medium);
        assert_eq!(plait.draft_thinking_levels(), &[Off, Low, High]);
        assert_eq!(plait.draft_thinking(), Some(Low));
        // A model without levels has no thinking chip.
        plait.choose_draft_model(HarnessId::Pi, other_choice(), cx);
        assert_eq!(plait.draft_thinking(), None);
        // Unchosen, it follows the default on each model.
        plait.choose_draft_harness(HarnessId::Claude, cx);
        assert_eq!(plait.draft_thinking(), Some(Medium));
        // Only the model's own levels can be chosen.
        plait.choose_draft_thinking(Off, cx);
        assert_eq!(plait.draft_thinking(), Some(Medium));
        plait.choose_draft_thinking(Max, cx);
        assert_eq!(plait.draft_thinking(), Some(Max));
        // Codex stops at High, so Maximum moves there and stays there.
        plait.choose_draft_harness(HarnessId::Codex, cx);
        assert_eq!(plait.draft_thinking(), Some(High));
        plait.choose_draft_harness(HarnessId::Claude, cx);
        assert_eq!(plait.draft_thinking(), Some(High));
        let mini = ModelChoice {
            provider: "claude".into(),
            model_id: "sample-claude-mini".into(),
        };
        plait.choose_draft_model(HarnessId::Claude, mini, cx);
        assert_eq!(
            plait.draft_model().map(|m| m.model_id),
            Some("sample-claude-mini".into())
        );
        assert_eq!(plait.draft_thinking(), Some(High));
        plait.choose_draft_thinking(Xhigh, cx);
    });
    harness.type_draft("Queue the status refresh");
    harness.keystroke("enter");
    harness.update(|plait, _, cx| {
        assert!(plait.draft_pending(), "sending creates the thread");
        assert_eq!(many_drafts::pending_thinking(plait), Some(Xhigh));
        // The chip cannot change what is being created.
        plait.choose_draft_thinking(Low, cx);
        assert_eq!(plait.draft_thinking(), Some(Xhigh));
        // A later default does not reach the unconfirmed creation.
        plait.preferences.value.default_thinking = Some(Off);
    });
    // No host runs in this scratch home, so the request stays unconfirmed.
    harness.creation_settles();
    harness.update(|plait, window, cx| {
        assert!(plait.draft_pending());
        // A retry repeats the same request at the same level.
        let request = many_drafts::pending_choice(plait);
        plait.retry_draft(window, cx);
        assert!(plait.draft_creating());
        assert_eq!(many_drafts::pending_choice(plait), request);
        assert_eq!(many_drafts::pending_thinking(plait), Some(Xhigh));
    });
    harness.creation_settles();
    harness.update(|plait, _, _| assert_eq!(many_drafts::pending_thinking(plait), Some(Xhigh)));
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A creation reply reaches the draft that sent it, and no other view: a
/// discarded draft's thread is only listed; one left for another thread
/// gets its message without taking the screen.
#[core::prelude::v1::test]
fn a_creation_reply_lands_only_on_the_draft_that_sent_it() {
    if !isolated("a_creation_reply_lands_only_on_the_draft_that_sent_it") {
        return;
    }
    use super::new_thread::Sending;
    use plait_core::host::{Error, HostError, ThreadReply};
    let scratch = std::env::temp_dir().join(format!("plait-draft-reply-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    let reply = |id: &str| -> Result<ThreadReply, Error> {
        Ok(ThreadReply {
            thread: Snapshot {
                id: id.into(),
                checkout: repository.clone(),
                model: choice(),
                ..Default::default()
            },
        })
    };
    // As `send_draft` leaves things, without a socket.
    let sent = |plait: &mut Plait, request: &str, text: &str| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        many_drafts::set_pending(plait, request);
        plait.draft.sending = Some(Sending {
            images: Vec::new(),
            generation: plait.draft.generation,
            request_id: request.into(),
            text: text.into(),
            revision: 1,
            approval: Some(ApprovalMode::Ask),
            choice: Some(ApprovalMode::Ask),
        });
    };
    harness.update(|plait, window, cx| {
        // Discarded: return to the neighbouring thread; the late reply sends nothing.
        plait.open_draft(window, cx);
        sent(plait, "discarded", "Gone");
        plait.discard_draft(window, cx);
        many_drafts::finish(plait, "discarded", reply("thr_discarded"), window, cx);
        assert!(plait.draft.pending.is_none());
        assert!(
            plait
                .host
                .connection
                .threads
                .iter()
                .all(|t| t.id != "thr_discarded")
        );
        assert_eq!(plait.host.thread.selected.as_deref(), Some(THREAD));
        assert!(!plait.workspace.composers.contains_key("thr_discarded"));
        assert!(plait.host.submissions.all("thr_discarded").is_empty());

        // A reply for another request than the pending one changes nothing.
        plait.open_draft(window, cx);
        sent(plait, "current", "Kept");
        many_drafts::finish(plait, "earlier", reply("thr_earlier"), window, cx);
        assert!(plait.draft_creating() && plait.draft_pending());
        assert!(
            !plait
                .host
                .connection
                .threads
                .iter()
                .any(|t| t.id == "thr_earlier")
        );
        // Nor does a reply for a draft from before a discard.
        plait.draft.generation += 1;
        many_drafts::finish(plait, "current", reply("thr_stale"), window, cx);
        assert!(
            plait.host.thread.selected.is_none()
                && !plait.workspace.composers.contains_key("thr_stale")
        );
        plait.discard_draft(window, cx);

        // Left for another thread: the message goes to the new thread, which
        // does not take the screen.
        plait.open_draft(window, cx);
        sent(plait, "left", "Sent while away");
        plait.select_host_thread(THREAD.into(), window, cx);
        many_drafts::finish(plait, "left", reply("thr_left"), window, cx);
        assert_eq!(plait.host.thread.selected.as_deref(), Some(THREAD));
        assert!(
            !plait.workspace.composers.contains_key("thr_left")
                && plait
                    .session
                    .data
                    .as_ref()
                    .unwrap()
                    .threads
                    .contains_key("thr_left"),
            "a hidden creation retains recovery data, not a composer"
        );
        assert_eq!(
            plait.host.submissions.all("thr_left")[0].text,
            "Sent while away"
        );
        assert_eq!(plait.thread_approval("thr_left"), Some(ApprovalMode::Ask));
        assert!(plait.draft.composer.is_none() && plait.draft.sending.is_none());

        // On screen: the thread opens with its message on the way.
        plait.open_draft(window, cx);
        sent(plait, "visible", "Open it");
        many_drafts::finish(plait, "visible", reply("thr_visible"), window, cx);
        assert_eq!(plait.host.thread.selected.as_deref(), Some("thr_visible"));
        assert_eq!(plait.host.submissions.all("thr_visible")[0].text, "Open it");
        assert!(!plait.draft.open);

        // Refused: the text stays in the draft with the reason.
        plait.open_draft(window, cx);
        sent(plait, "refused", "Try again");
        let refused = Err(Error::Rejected(HostError {
            code: "invalid".into(),
            message: "Sample refusal.".into(),
        }));
        many_drafts::finish(plait, "refused", refused, window, cx);
        assert!(plait.draft.pending.is_none() && plait.draft.sending.is_none());
        assert!(
            plait
                .draft
                .error
                .as_deref()
                .unwrap()
                .contains("Sample refusal.")
        );
        assert!(plait.draft.open && plait.draft.composer.is_some());
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Menlo ships with every Mac, so it stands in for a font the person chose.
#[cfg(target_os = "macos")]
#[core::prelude::v1::test]
fn a_typed_code_font_is_drawn_only_when_it_is_installed() {
    use super::appearance::Fit;
    use super::theme::{SYSTEM_MONO, mono};
    // Choosing a font saves a preference; only a scratch home may receive it.
    if !isolated("a_typed_code_font_is_drawn_only_when_it_is_installed") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-font-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 1380., 860.);
    harness.settle();
    harness.update(|plait, _, cx| plait.prepare_code_font(cx));
    fonts_settled(&mut harness);
    let type_name = |harness: &mut Harness, name: &'static str| {
        harness.update(move |plait, _, cx| {
            let field = plait.code_font.field.clone().expect("the field");
            field.update(cx, |field, cx| field.set_text(name, cx));
        });
        fonts_settled(harness);
    };
    assert_eq!(mono().as_ref(), SYSTEM_MONO);
    // A family that is not installed is never handed to the text system.
    type_name(&mut harness, "No Such Font Family");
    harness.update(|plait, _, _| {
        assert_eq!(mono().as_ref(), SYSTEM_MONO);
        assert_eq!(plait.preferences.value.code_font(), None);
    });
    // An installed one is found whatever its case, drawn, and saved by its own name.
    type_name(&mut harness, "menlo");
    harness.update(|plait, _, _| {
        assert_eq!(mono().as_ref(), "Menlo");
        assert_eq!(plait.preferences.value.code_font(), Some("Menlo"));
    });
    // A half-typed name keeps the last choice.
    type_name(&mut harness, "men");
    harness.update(|plait, _, _| {
        assert_eq!(mono().as_ref(), "Menlo");
        assert_eq!(plait.preferences.value.code_font(), Some("Menlo"));
    });
    // Helvetica ships with every Mac too, and its characters differ in
    // width: files and diffs could not be laid out in it, so it is refused.
    type_name(&mut harness, "Helvetica");
    harness.update(|plait, _, _| {
        assert_eq!(plait.code_font.fit("Helvetica"), Some(Fit::Uneven));
        assert_eq!(mono().as_ref(), "Menlo");
        assert_eq!(plait.preferences.value.code_font(), Some("Menlo"));
    });
    // Opening Settings again shows the choice, not the name left in the field.
    harness.update(|plait, _, cx| plait.prepare_code_font(cx));
    fonts_settled(&mut harness);
    harness.update(|plait, _, cx| {
        let field = plait.code_font.field.clone().expect("the field");
        assert_eq!(field.read(cx).text(), "Menlo");
        assert_eq!(mono().as_ref(), "Menlo");
    });
    // An empty field is the system's font again.
    type_name(&mut harness, "");
    harness.update(|plait, _, _| {
        assert_eq!(mono().as_ref(), SYSTEM_MONO);
        assert_eq!(plait.preferences.value.code_font(), None);
    });
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Wait for the installed fonts to be read and for every check of a family
/// to finish: both run in the background.
#[cfg(target_os = "macos")]
fn fonts_settled(harness: &mut Harness) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        harness.settle();
        let settled = harness
            .window
            .update(&mut harness.cx, |plait, _, _| plait.code_font.settled())
            .unwrap();
        if settled {
            // What the last result set off has run.
            harness.settle();
            return;
        }
        assert!(Instant::now() < deadline, "the fonts were not read");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Settings can open before the preferences have been read. The field then
/// shows the default, which must not be taken for a choice: the saved font
/// is still the one drawn, and the one on disk.
#[cfg(target_os = "macos")]
#[core::prelude::v1::test]
fn a_saved_code_font_outlasts_settings_opening_before_the_preferences_load() {
    use super::theme::{SYSTEM_MONO, mono};
    if !isolated("a_saved_code_font_outlasts_settings_opening_before_the_preferences_load") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-font-load-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 1380., 860.);
    harness.settle();
    // Settings opens, and the installed fonts are read, with nothing saved yet.
    harness.update(|plait, _, cx| plait.prepare_code_font(cx));
    fonts_settled(&mut harness);
    assert_eq!(mono().as_ref(), SYSTEM_MONO);
    // The file names a font, and is read as it is at launch.
    let path = plait_core::preferences::default_path().expect("scratch preferences");
    std::fs::create_dir_all(path.parent().expect("a folder")).unwrap();
    std::fs::write(&path, r#"{"code_font":"Menlo"}"#).unwrap();
    harness.update(|plait, _, cx| plait.load_preferences(cx));
    let deadline = Instant::now() + Duration::from_secs(10);
    while mono().as_ref() != "Menlo" {
        assert!(Instant::now() < deadline, "the saved font was not drawn");
        std::thread::sleep(Duration::from_millis(5));
        fonts_settled(&mut harness);
    }
    harness.update(|plait, _, cx| {
        assert_eq!(plait.preferences.value.code_font(), Some("Menlo"));
        let field = plait.code_font.field.clone().expect("the field");
        assert_eq!(field.read(cx).text(), "Menlo");
    });
    let saved = plait_core::preferences::load(&path).expect("the file reads");
    assert_eq!(saved.code_font(), Some("Menlo"));
    let _ = std::fs::remove_dir_all(&scratch);
}

#[core::prelude::v1::test]
fn changed_files_count_with_git_hide_on_request_and_open_in_changes() {
    // Hiding saves a preference; only a scratch home may receive it.
    if !isolated("changed_files_count_with_git_hide_on_request_and_open_in_changes") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-files-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 1380., 860.);
    harness.settle();
    let root = repository.canonicalize().unwrap();
    harness.update(move |plait, _, cx| {
        plait.host.connection.connected = true;
        plait.host.connection.status = Some(status());
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        plait.host.thread.selected = Some(THREAD.into());
        plait
            .workspace
            .thread_files
            .inject(THREAD, root, Vec::new());
        plait.workspace.thread_files.set_paths(vec![
            "src/review.rs".into(),
            "notes.md".into(),
            "untouched.txt".into(),
        ]);
        plait.thread_files_status(cx);
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        harness.settle();
        let shown = harness
            .window
            .update(&mut harness.cx, |plait, _, cx| {
                plait.thread_files_card(cx).is_some()
            })
            .unwrap();
        if shown {
            break;
        }
        assert!(Instant::now() < deadline, "the counts did not arrive");
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.update(|plait, window, cx| {
        let paths: Vec<_> = plait
            .workspace
            .thread_files
            .counted()
            .map(|c| c.path.clone())
            .collect();
        assert_eq!(
            paths,
            [PathBuf::from("src/review.rs"), PathBuf::from("notes.md")],
            "clean files are left out"
        );
        plait.set_changed_files_hidden(true, cx);
        assert!(plait.thread_files_card(cx).is_none());
        plait.preferences.value.hide_changed_files = false;
        plait.open_thread_file("src/review.rs".into(), window, cx);
        assert_eq!(plait.workspace.tab, super::Workspace::Changes);
        let repo = &plait.state.repository;
        let key = repo.selected_diff().unwrap();
        assert_eq!(
            repo.snapshot().ready().unwrap().changes[key.file].path,
            PathBuf::from("src/review.rs")
        );
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn streaming_text_flows_in_over_display_frames_and_catches_up() {
    let scratch = std::env::temp_dir().join(format!("plait-reveal-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 1380., 860.);
    harness.connect(&repository, false);
    let trailing = |harness: &mut Harness| {
        harness
            .window
            .update(&mut harness.cx, |plait, _, _| {
                let blocks = &plait.workspace.formatted.live.as_ref().unwrap().blocks;
                plait
                    .workspace
                    .reveal
                    .pending_text(plait_core::reveal::total(blocks))
            })
            .unwrap()
    };
    // Text already there when the thread is shown appears at once.
    harness.draw();
    assert!(!trailing(&mut harness));
    // A burst spreads over the following frames.
    harness.change_snapshot(|snapshot| snapshot.live_text.push_str(&" and more words".repeat(60)));
    harness.draw();
    assert!(trailing(&mut harness), "a burst does not land in one frame");
    // The reveal asks for each next frame itself, until it has caught up.
    let window = harness.window.into();
    let mut frames = 0;
    while trailing(&mut harness) {
        std::thread::sleep(Duration::from_millis(8));
        let ran = harness
            .cx
            .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        // The test window also redraws when the callback notifies.
        assert!(ran >= 1, "each frame asks for the next");
        harness.draw();
        frames += 1;
        assert!(frames < 200, "the reveal did not catch up");
    }
    assert!(frames > 3, "{frames}");
    // Caught up, it stops asking: an idle conversation draws no frames. The
    // test window's own redraw may have left one request from the last frame.
    harness
        .cx
        .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
        .unwrap();
    harness.draw();
    let ran = harness
        .cx
        .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
        .unwrap();
    assert_eq!(ran, 0);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// The index of the shown diff's first added line.
fn first_added(plait: &Plait) -> usize {
    let Load::Ready(diff) = &plait.state.repository.diff() else {
        panic!("the diff is read");
    };
    diff.lines
        .iter()
        .position(|line| line.kind == plait_core::model::DiffKind::Added)
        .expect("an added line")
}

#[core::prelude::v1::test]
fn asking_about_picked_lines_quotes_them_into_the_thread_composer() {
    let scratch = std::env::temp_dir().join(format!("plait-ask-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 1380., 860.);
    harness.update(|plait, window, cx| plait.select_file(1, DiffMode::Unstaged, window, cx));
    harness.settle();
    harness.update(|plait, window, cx| {
        let first = first_added(plait);
        plait.pick_line(first, false, cx);
        plait.pick_line(first - 1, true, cx);
        // Without a thread there is nobody to ask; the pick stays.
        plait.ask_about_pick(window, cx);
        assert!(plait.workspace.line_pick.is_some());

        plait.host.thread.selected = Some(THREAD.into());
        let composer = cx.new(|cx| Composer::new(window, cx));
        composer.update(cx, |composer, cx| {
            composer.set_enabled(true, None, cx);
            composer.type_text("Why this?", window, cx);
        });
        plait
            .workspace
            .composers
            .insert(THREAD.into(), composer.clone());
        plait.ask_about_pick(window, cx);
        let text = composer.read(cx).text().to_owned();
        assert!(text.starts_with("Why this?\n\n`"), "{text}");
        assert!(text.contains("` (unstaged):\n```diff\n"), "{text}");
        assert!(text.ends_with("```\n\n"), "{text}");
        assert!(plait.workspace.line_pick.is_none());
        assert_eq!(plait.workspace.tab, super::Workspace::Conversation);

        // A pick belongs to its file and side.
        plait.show_tab(super::Workspace::Changes, window, cx);
        plait.pick_line(first, false, cx);
        assert!(plait.workspace.line_pick.is_some());
        plait.select_file(1, DiffMode::Staged, window, cx);
        assert!(plait.current_pick(usize::MAX).is_none());
        // Clicking a lone picked line again clears it.
        plait.select_file(1, DiffMode::Unstaged, window, cx);
    });
    harness.settle();
    harness.update(|plait, _, cx| {
        let first = first_added(plait);
        assert!(plait.current_pick(usize::MAX).is_some(), "back on its side");
        plait.pick_line(first, false, cx);
        assert!(plait.workspace.line_pick.is_none());
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Draw, then deliver one key as the platform would: bindings first, then text input.
fn press(harness: &mut Harness, key: &'static str) {
    harness.capture("keys");
    let window = harness.window.into();
    harness
        .cx
        .update_window(window, |_, window, cx| {
            window.dispatch_keystroke(Keystroke::parse(key).expect("keystroke"), cx);
        })
        .expect("key");
    harness.settle();
}

fn shown(harness: &mut Harness) -> Vec<String> {
    harness
        .window
        .update(&mut harness.cx, |plait, _, cx| {
            let overlay = plait.overlay.as_ref().expect("open overlay");
            overlay.palette.read(cx).shown()
        })
        .expect("window update")
}

/// Real key events reach the palette: Cmd-K opens it, typing filters, the
/// arrows move, Enter runs, and Escape returns focus.
#[core::prelude::v1::test]
fn keys_drive_the_palette_and_picker() {
    if !isolated("keys_drive_the_palette_and_picker") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-keys-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    let focused = |harness: &mut Harness| {
        harness
            .window
            .update(&mut harness.cx, |_, window, cx| window.focused(cx))
            .expect("window update")
    };
    let before = focused(&mut harness);

    press(&mut harness, "cmd-k");
    assert_eq!(
        shown(&mut harness).len(),
        2 + super::actions::ACTIONS
            .iter()
            .filter(|action| action.place != super::actions::Place::Hidden
                && action.id != "palette.toggle")
            .count()
            + 1
            + 5,
        "threads, commands, projects, models"
    );
    press(&mut harness, "escape");
    harness.update(|plait, _, _| assert!(plait.overlay.is_none()));
    assert!(focused(&mut harness) == before, "Escape returns focus");

    // Typed letters go to the search field and rank the list.
    press(&mut harness, "cmd-k");
    for key in ["s", "i", "d", "e"] {
        press(&mut harness, key);
    }
    assert_eq!(shown(&mut harness)[0], "Toggle Sidebar");
    press(&mut harness, "backspace");
    press(&mut harness, "e");
    press(&mut harness, "enter");
    harness.update(|plait, _, _| {
        assert!(plait.overlay.is_none());
        assert!(!plait.sidebar, "Enter ran the selected command");
    });

    // Down moves to the second thread; Enter opens it and focuses its composer.
    press(&mut harness, "cmd-k");
    press(&mut harness, "down");
    press(&mut harness, "enter");
    harness.update(|plait, window, cx| {
        assert_eq!(plait.host.thread.selected.as_deref(), Some("thr_0c44a9d1"));
        let composer = plait.workspace.composers["thr_0c44a9d1"].focus_handle(cx);
        assert!(window.focused(cx) == Some(composer));
        // Selecting dropped the sample connection; restore it for the picker.
        plait.host.connection.connected = true;
        plait.host.connection.error = None;
    });

    // Cmd-Shift-N asks for a model. Tab stays inside the overlay.
    press(&mut harness, "cmd-shift-n");
    assert_eq!(shown(&mut harness).len(), 42, "the whole catalog is listed");
    let field = focused(&mut harness);
    press(&mut harness, "tab");
    assert!(focused(&mut harness) == field);
    for key in ["a", "s", "t", "r", "a"] {
        press(&mut harness, key);
    }
    assert_eq!(shown(&mut harness), vec!["GPT-6 Astra"]);
    // Cmd-Return saves the default without closing; no host answers here.
    press(&mut harness, "cmd-enter");
    harness.update(|plait, _, _| {
        assert!(plait.overlay.is_some());
        let astra = ModelChoice {
            provider: "openai-codex".into(),
            model_id: "gpt-6-astra".into(),
        };
        assert_eq!(plait.host.connection.settings_model, Some(astra));
        // The host's default is unchanged, and the state says so.
        assert_eq!(
            plait.host.connection.status.as_ref().unwrap().default_model,
            Some(choice())
        );
        let (state, _) = plait.default_model_state().expect("save state");
        assert!(state.contains("not saved"), "{state}");
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Every view at the default size and at the minimum window size.
fn walk(output: Option<&Path>) {
    // Renders share one fixture path so it reads the same in every PNG. Another
    // render process would delete it mid-walk, so wait for that one to finish.
    let _render_lock = output.map(|_| {
        let lock = std::fs::File::create(std::env::temp_dir().join("plait-snapshot-render.lock"))
            .expect("render lock file");
        lock.lock().expect("render lock");
        lock
    });
    let name = if output.is_some() {
        "plait-snapshot-render".to_owned()
    } else {
        format!("plait-snapshot-{}-draw", std::process::id())
    };
    let scratch = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);

    for (suffix, width, height) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        tabs::walk(&repository, output, suffix, width, height);
        let mut harness = Harness::new(&repository, output, width, height);
        // No socket in tests, so the host starts disconnected.
        harness.capture(&format!("home-disconnected-{suffix}"));
        harness.update(|plait, window, cx| {
            plait.select_file(1, DiffMode::Unstaged, window, cx);
        });
        harness.capture(&format!("changes-{suffix}"));
        // A typed message and the result of an earlier write, without writing:
        // every scene shares this fixture repository.
        harness.update(|plait, _, cx| {
            plait.workspace.commit_message.update(cx, |field, cx| {
                field.set_text("Queue the status refresh", cx)
            });
            plait.state.repository.write_mut().note = Some(("Staged 1 file".into(), false));
        });
        harness.capture(&format!("changes-commit-{suffix}"));
        // Resting on the hunk header reveals its Stage hunk control.
        // It is the diff's first row: the path-only header lines are not drawn.
        // In the narrow window the diff header's actions wrap to a second line.
        let hunk_row = if width > 1000. {
            point(px(700.), px(98.))
        } else {
            point(px(600.), px(127.))
        };
        harness.pointer(PlatformInput::MouseMove(MouseMoveEvent {
            position: hunk_row,
            pressed_button: None,
            modifiers: Modifiers::default(),
        }));
        harness.capture(&format!("changes-hunk-{suffix}"));
        // After a discard, its result offers Undo. Injected, not performed.
        harness.update(|plait, _, _| {
            let write = plait.state.repository.write_mut();
            write.note = Some(("Discarded changes to review.rs".into(), false));
            write.undo = Some(plait_core::git::Backup {
                path: "src/review.rs".into(),
                blob: "0".repeat(40),
                executable: false,
            });
        });
        harness.capture(&format!("changes-undo-{suffix}"));
        harness.update(|plait, _, _| {
            let write = plait.state.repository.write_mut();
            write.note = Some(("Staged 1 file".into(), false));
            write.undo = None;
        });
        // Lines picked by their numbers, ready to ask about.
        harness.update(|plait, _, cx| {
            let first = first_added(plait);
            plait.pick_line(first - 1, false, cx);
            plait.pick_line(first + 1, true, cx);
        });
        harness.capture(&format!("changes-pick-{suffix}"));
        harness.update(|plait, _, cx| {
            plait.workspace.line_pick = None;
            cx.notify();
        });
        harness.update(|plait, window, cx| {
            plait
                .workspace
                .commit_message
                .update(cx, |field, cx| field.set_text("", cx));
            plait.state.repository.write_mut().note = None;
            plait.show_tab(super::Workspace::History, window, cx);
        });
        harness.capture(&format!("history-list-{suffix}"));
        harness.update(|plait, _, cx| {
            let id = plait
                .state
                .repository
                .commit_list()
                .ready()
                .and_then(|history| history.commits.first())
                .map(|commit| commit.id.clone())
                .expect("the fixture has a commit");
            plait.state.repository.select_commit(id);
            plait.pump(cx);
        });
        harness.capture(&format!("history-commit-{suffix}"));
        // The root commit adds two files: each patch begins under its heading.
        harness.update(|plait, _, cx| {
            let id = plait
                .state
                .repository
                .commit_list()
                .ready()
                .and_then(|history| history.commits.last())
                .map(|commit| commit.id.clone())
                .expect("the fixture has a root commit");
            plait.state.repository.select_commit(id);
            plait.workspace.commit_scroll = UniformListScrollHandle::new();
            plait.pump(cx);
        });
        harness.capture(&format!("history-files-{suffix}"));
        harness.update(|plait, _, _| {
            plait.workspace.tab = super::Workspace::Conversation;
            plait.host.connection.connecting = false;
            plait.host.connection.connected = true;
            plait.host.connection.status = Some(status());
        });
        harness.capture(&format!("home-empty-{suffix}"));
        harness.connect(&repository, false);
        harness.capture(&format!("live-{suffix}"));
        harness.update(|plait, _, _| {
            plait.host.connection.connected = false;
            plait.host.connection.error =
                Some("Local host connection failed: sample error.".into());
        });
        harness.capture(&format!("live-disconnected-{suffix}"));
        harness.connect(&repository, true);
        harness.update(|plait, _, cx| plait.toggle_live_tool("t1", cx));
        harness.capture(&format!("live-approval-{suffix}"));
        streaming_walk(&mut harness, &repository, suffix);
        tool_states_walk(&mut harness, &repository, suffix);
        reading_walk(&mut harness, &repository, suffix);
        messages::walk(&mut harness, &repository, suffix);
        find::find_walk(&mut harness, &repository, suffix);
        undo::undo_walk(&mut harness, &repository, suffix);
        turn_review::walk(&mut harness, &repository, suffix);
        history_walk(&mut harness, &repository, suffix);
        tool_cards_walk(&mut harness, &repository, suffix);
        draft_walk(&mut harness, &repository, suffix);
        composer_walk(&mut harness, &repository, suffix);
        images::walk(&mut harness, &repository, suffix);
        composer_v3_walk(&mut harness, &repository, suffix);
        settings_modal::walk(&mut harness, &repository, suffix);
        settings_walk(&mut harness, &repository, suffix);
        settings_controls_walk(&mut harness, &repository, suffix);
        mcp::mcp_walk(&mut harness, &repository, suffix);
        files_walk(&mut harness, suffix);
        ways_out::ways_out_walk(&mut harness, &repository, suffix);
        theme_walk(&mut harness, &repository, suffix);
        palette_walk(&mut harness, &repository, suffix);
        picker_walk(&mut harness, &repository, suffix);
        new_thread_walk(&mut harness, &repository, suffix);
        harnesses_walk(&mut harness, &repository, suffix);
        claude_walk(&mut harness, &repository, suffix);
        draft_scenes_walk(&mut harness, &repository, suffix);
        branches::branches_walk(&mut harness, &repository, suffix);
        inspector_walk(&mut harness, &repository, suffix);
        navigation_walk(&mut harness, &repository, suffix);
        // Threads exist, but none is open.
        harness.update(|plait, _, _| {
            plait.workspace.host_watch = None;
            plait.host.select(None);
        });
        harness.capture(&format!("home-threads-{suffix}"));
        // Cmd-2 selects the second thread; next/previous wrap around the list.
        harness.update(|plait, window, cx| {
            plait.jump_to(2, window, cx);
            assert_eq!(plait.host.thread.selected.as_deref(), Some("thr_0c44a9d1"));
            plait.step_thread(true, window, cx);
            assert_eq!(plait.host.thread.selected.as_deref(), Some(THREAD));
            plait.step_thread(false, window, cx);
            assert_eq!(plait.host.thread.selected.as_deref(), Some("thr_0c44a9d1"));
            plait.jump_to(9, window, cx);
            assert_eq!(plait.host.thread.selected.as_deref(), Some("thr_0c44a9d1"));
            plait.sidebar = false;
            plait.diagnostics = true;
        });
        harness.capture(&format!("no-sidebar-{suffix}"));
        ownership_scenes(&repository, output, suffix, width, height);
        isolation::walk(&repository, output, suffix, width, height);
        storage::walk(&repository, output, suffix, width, height);
        worktrees_view::walk(&repository, output, suffix, width, height);
    }
    let _ = std::fs::remove_dir_all(&scratch);
}

fn streaming_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.change_snapshot(|snapshot| {
        snapshot.messages.extend((3..40).map(|index| Message {
            id: format!("history-{index}"),
            role: MessageRole::Assistant,
            text: format!(
                "Sample history row {index}. This is a rendering fixture, not an agent run."
            ),
            ..Default::default()
        }));
    });
    harness.update(|plait, _, cx| {
        plait.toggle_live_tool("t2", cx);
        plait.workspace.live_scroll.scroll_to(ListOffset {
            item_ix: 12,
            offset_in_item: px(7.),
        });
    });
    harness.capture(&format!("stream-history-{suffix}"));
    let anchor = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            plait.workspace.live_scroll.logical_scroll_top()
        })
        .unwrap();
    for _ in 0..20 {
        harness.update(|plait, _, _| {
            let measured = plait.workspace.live_scroll.bounds_for_item(anchor.item_ix);
            assert!(
                measured.is_some(),
                "visible history row must already be measured"
            );
            let previous = plait.host.thread.snapshot.clone().unwrap();
            Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap())
                .live_text
                .push_str(" another streamed word");
            plait.workspace.formatted = Arc::new(plait_core::markdown::Transcript::update(
                &plait.workspace.formatted,
                plait.host.thread.snapshot.as_ref().unwrap(),
            ));
            plait.refresh_live_list(Some(&previous));
            assert_eq!(
                plait.workspace.live_scroll.bounds_for_item(anchor.item_ix),
                measured
            );
            let current = plait.workspace.live_scroll.logical_scroll_top();
            assert_eq!(current.item_ix, anchor.item_ix);
            assert_eq!(current.offset_in_item, anchor.offset_in_item);
            assert!(plait.workspace.expanded_live_tools.contains("t2"));
        });
    }
    // Text arrived while the reader was elsewhere in the history.
    harness.capture(&format!("stream-new-activity-{suffix}"));
    harness.change_snapshot(|snapshot| {
        snapshot.messages.remove(0);
        snapshot.messages.push(Message {
            id: "new-user".into(),
            role: MessageRole::User,
            text: "Sample follow-up".into(),
            ..Default::default()
        });
        snapshot.live_text.clear();
        snapshot.tools[1].output = "Sample test output: 24 tests passed.".into();
    });
    harness.update(|plait, _, _| {
        let current = plait.workspace.live_scroll.logical_scroll_top();
        assert_eq!(current.item_ix, anchor.item_ix - 1);
        assert_eq!(current.offset_in_item, anchor.offset_in_item);
        assert!(plait.workspace.expanded_live_tools.contains("t2"));
        assert!(!plait.workspace.expanded_live_tools.contains("t1"));
        plait.workspace.live_scroll.scroll_to_end();
    });
    harness.capture(&format!("stream-tool-{suffix}"));
    harness.change_snapshot(|snapshot| {
        snapshot.tools.remove(1);
    });
    harness.update(|plait, _, _| assert!(!plait.workspace.expanded_live_tools.contains("t2")));
}

/// Done, failed, and interrupted tool calls, two of them expanded, with the
/// host's error line and the clipped-history note.
fn tool_states_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.change_snapshot(tool_states);
    harness.update(|plait, _, cx| {
        // The sidebar and composer follow the thread's status, as a host update would set it.
        plait.host.connection.threads[0].status = ThreadStatus::Error;
        plait.sync_composer(cx);
        plait.toggle_live_tool("t2", cx);
        plait.toggle_live_tool("t3", cx);
        plait.workspace.live_scroll.scroll_to_end();
    });
    harness.capture(&format!("live-tools-{suffix}"));
    // Later scenes show the usual thread list again.
    harness.connect(repository, false);
}

/// Prose, a table, links, a selection across blocks, and highlighted code.
fn reading_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    use super::selection::Owner;
    harness.connect(repository, false);
    harness.change_snapshot(reading);
    let at = |item_ix| ListOffset {
        item_ix,
        offset_in_item: px(0.),
    };
    harness.update(|plait, _, cx| {
        plait.host.connection.threads[0].status = ThreadStatus::Idle;
        plait.sync_composer(cx);
        plait.workspace.live_scroll.scroll_to(at(0));
    });
    harness.capture(&format!("read-prose-{suffix}"));
    let reply = || Owner::Message("r2".into());
    // Where the caret before a byte of one of the reply's blocks is on screen.
    let caret = |harness: &mut Harness, block: usize, offset: usize| {
        let found = harness.window.update(&mut harness.cx, |plait, _, _| {
            plait.text_point(&reply(), block, offset)
        });
        found.unwrap().expect("text is on screen")
    };
    let press = |position, click_count| {
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count,
            first_mouse: false,
        })
    };
    let release = |position| {
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        })
    };
    let moved = |position, pressed_button| {
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button,
            modifiers: Modifiers::default(),
        })
    };
    let selected = |harness: &mut Harness| {
        harness
            .window
            .update(&mut harness.cx, |plait, _, _| plait.selected_text())
            .unwrap()
    };

    // Resting on a link shows where it leads, and the row's quiet controls.
    let link = caret(
        harness,
        1,
        "This is a rendering fixture, not an agent run. See the G".len(),
    );
    harness.pointer(moved(link, None));
    harness.update(|plait, _, _| {
        assert_eq!(
            plait.workspace.reading.hovered_link.as_deref(),
            Some("https://gpui.rs/")
        );
    });
    harness.capture(&format!("read-hover-{suffix}"));
    // A click that starts and ends on the link follows it and selects nothing.
    harness.pointer(press(link, 1));
    harness.pointer(release(link));
    harness.capture(&format!("read-hover-{suffix}"));
    assert_eq!(selected(harness), None);

    // Drag from inside the first paragraph into a later block: the table when
    // the window is tall enough to show it, otherwise a list item.
    let from = caret(
        harness,
        1,
        "This is a rendering fixture, not an agent run. ".len(),
    );
    let (block, offset, ending) = if suffix == "wide" {
        (
            10,
            "Crate\tRole\tLines\nplait\tGPUI presentation".len(),
            "plait\tGPUI presentation",
        )
    } else {
        (5, "2. Second".len(), "\n2. Second")
    };
    let to = caret(harness, block, offset);
    harness.pointer(press(from, 1));
    harness.pointer(moved(to, Some(MouseButton::Left)));
    harness.capture(&format!("read-selection-{suffix}"));
    harness.pointer(release(to));
    let copied = selected(harness).expect("dragged selection");
    assert!(copied.starts_with("See the GPUI site"), "{copied}");
    assert!(copied.contains("1. First step\n◦ nested point"), "{copied}");
    assert!(copied.ends_with(ending), "{copied}");
    // Cmd-C reaches the conversation because the press moved focus to it.
    harness.update(|_, _, cx| super::selection::bind_keys(cx));
    harness.keystroke("cmd-c");
    harness.update(|_, _, cx| {
        let clipboard = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(clipboard.as_deref(), Some(copied.as_str()));
    });

    // A double click takes a word; a shift-click extends from where it began.
    let word = caret(harness, 5, "2. Sec".len());
    harness.pointer(press(word, 2));
    harness.pointer(release(word));
    harness.capture(&format!("read-word-{suffix}"));
    assert_eq!(selected(harness).as_deref(), Some("Second"));
    let end = caret(harness, 5, "2. Second step".len());
    harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position: end,
        modifiers: Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        click_count: 1,
        first_mouse: false,
    }));
    harness.pointer(release(end));
    harness.capture(&format!("read-word-{suffix}"));
    assert_eq!(selected(harness).as_deref(), Some("Second step"));
    // A triple click takes the block, without its list marker.
    let item = caret(harness, 2, "1. Fir".len());
    harness.pointer(press(item, 3));
    harness.pointer(release(item));
    harness.capture(&format!("read-word-{suffix}"));
    assert_eq!(selected(harness).as_deref(), Some("First step"));
    harness.pointer(press(from, 1));
    harness.pointer(release(from));
    harness.capture(&format!("read-word-{suffix}"));
    assert_eq!(
        selected(harness),
        None,
        "a plain click clears the selection"
    );

    // Dragging past the bottom edge scrolls, and the selection follows.
    let (below, before) = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            let list = plait.workspace.live_scroll.viewport_bounds();
            let offset = plait
                .workspace
                .live_scroll
                .scroll_px_offset_for_scrollbar()
                .y;
            (point(from.x, list.bottom() + px(60.)), offset)
        })
        .unwrap();
    harness.pointer(press(from, 1));
    for _ in 0..16 {
        harness.pointer(moved(below, Some(MouseButton::Left)));
        harness.draw();
    }
    harness.capture(&format!("read-drag-scroll-{suffix}"));
    harness.pointer(release(below));
    harness.update(|plait, _, _| {
        let offset = plait
            .workspace
            .live_scroll
            .scroll_px_offset_for_scrollbar()
            .y;
        assert!(offset < before - px(200.), "{offset} {before}");
        let copied = plait.selected_text().expect("selection across rows");
        assert!(copied.starts_with("See the GPUI site"), "{copied}");
        assert!(
            copied.contains("\n\nAnd the code samples.\n\n// Sample code"),
            "{copied}"
        );
    });

    harness.update(|plait, _, _| plait.workspace.live_scroll.scroll_to(at(3)));
    harness.capture(&format!("read-code-{suffix}"));
    // In code, a triple click takes one line.
    let code = Owner::Message("r4".into());
    let line = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            plait.text_point(&code, 0, "// Sample code\npub fn re".len())
        })
        .unwrap()
        .expect("code is on screen");
    harness.pointer(press(line, 3));
    harness.pointer(release(line));
    harness.capture(&format!("read-code-{suffix}"));
    assert_eq!(
        selected(harness).as_deref(),
        Some("pub fn refresh<'a>(queue: &'a mut Queue) -> Result<usize, Error> {")
    );
    // The control over the list returns to the newest row.
    let jump = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            assert!(
                plait.workspace.live_scroll.logical_scroll_top().item_ix
                    < plait.workspace.live_count
            );
            let list = plait.workspace.live_scroll.viewport_bounds();
            // It ends at the right edge of the text column.
            let column = (list.size.width - px(48.)).min(px(760.));
            point(
                list.center().x + column * 0.5 - px(40.),
                list.bottom() - px(24.),
            )
        })
        .unwrap();
    harness.pointer(press(jump, 1));
    harness.pointer(release(jump));
    harness.capture(&format!("read-latest-{suffix}"));
    harness.update(|plait, _, _| {
        let top = plait.workspace.live_scroll.logical_scroll_top();
        assert_eq!(top.item_ix, plait.workspace.live_count, "at the newest row");
    });
    harness.capture(&format!("read-data-{suffix}"));
    harness.connect(repository, false);
}

/// Each tool's presentation, then a folded and an opened run of calls.
fn tool_cards_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.change_snapshot(tool_cards);
    harness.update(|plait, _, cx| {
        plait.host.connection.threads[0].status = ThreadStatus::Idle;
        plait.sync_composer(cx);
        for id in ["c1", "c2", "c3"] {
            plait.toggle_live_tool(id, cx);
        }
        plait.workspace.live_scroll.scroll_to_end();
    });
    harness.capture(&format!("tools-files-{suffix}"));
    harness.update(|plait, _, cx| {
        for id in ["c1", "c2", "c3", "c4", "c5", "c6"] {
            plait.toggle_live_tool(id, cx);
        }
        plait.workspace.live_scroll.scroll_to_end();
    });
    harness.capture(&format!("tools-commands-{suffix}"));

    harness.connect(repository, false);
    harness.change_snapshot(|snapshot| long_run(snapshot, false));
    harness.update(|plait, _, _| plait.workspace.live_scroll.scroll_to_end());
    harness.capture(&format!("tools-group-{suffix}"));
    // How the group's first two rows were last drawn: (hidden, leads the group).
    let drawn = |plait: &Plait| {
        let folds = plait.workspace.reading.folds.borrow();
        (folds["g0"], folds["g1"])
    };
    harness.update(|plait, _, _| {
        assert_eq!(drawn(plait), ((true, true), (true, false)));
    });
    harness.update(|plait, _, cx| {
        plait.toggle_tool_group("g0", cx);
        plait.toggle_live_tool("g3", cx);
        plait.workspace.live_scroll.scroll_to_end();
    });
    harness.capture(&format!("tools-group-open-{suffix}"));
    harness.update(|plait, _, _| {
        assert_eq!(drawn(plait), ((false, true), (false, false)));
        assert!(plait.tool_details_shown("g3"));
    });
    harness.update(|plait, _, cx| plait.toggle_tool_group("g0", cx));
    harness.capture(&format!("tools-group-{suffix}"));
    harness.update(|plait, _, _| {
        assert_eq!(drawn(plait), ((true, true), (true, false)));
        // Its details stay open for when the group is, but are not on screen.
        assert!(!plait.tool_details_shown("g3"));
    });

    harness.connect(repository, false);
    harness.change_snapshot(|snapshot| long_run(snapshot, true));
    harness.update(|plait, _, _| plait.workspace.live_scroll.scroll_to_end());
    harness.capture(&format!("tools-running-{suffix}"));
    harness.connect(repository, false);
}

/// A finished turn with an unsent draft, so the composer is ready to send.
fn draft_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.change_snapshot(finished_turn);
    harness.update(|plait, _, cx| {
        plait.host.connection.threads[0].status = ThreadStatus::Idle;
        plait.workspace.composers[THREAD].update(cx, |composer, cx| {
            composer.restore_draft(DRAFT.into(), 1, None, cx);
        });
        plait.sync_composer(cx);
        plait.workspace.live_scroll.scroll_to_end();
    });
    harness.capture(&format!("live-draft-{suffix}"));
    harness.connect(repository, false);
}

/// Sample checkout paths for the file-suggestion scenes. Not read from disk.
fn sample_files() -> Arc<plait_core::git::FileList> {
    let paths = [
        "Cargo.toml",
        "README.md",
        "crates/plait-core/src/git.rs",
        "crates/plait-core/src/git/tests.rs",
        "crates/plait-core/src/host_state.rs",
        "crates/plait-core/src/state.rs",
        "crates/plait/src/ui/changes.rs",
        "crates/plait/src/ui/composer/mod.rs",
        "crates/plait/src/ui/host.rs",
        "crates/plait/src/ui/session.rs",
        "crates/plait/src/ui/settings.rs",
        "docs/ARCHITECTURE.md",
        "docs/HOST.md",
        "scripts/snapshots.sh",
    ];
    Arc::new(plait_core::git::FileList {
        paths: paths.map(String::from).to_vec(),
        ..Default::default()
    })
}

impl Harness {
    /// A ready-to-send composer with nothing typed, on a finished turn.
    fn fresh_composer(&mut self, repository: &Path) {
        self.connect(repository, false);
        self.change_snapshot(finished_turn);
        self.update(|plait, _, cx| {
            plait.host.connection.threads[0].status = ThreadStatus::Idle;
            plait.sync_composer(cx);
            plait.workspace.live_scroll.scroll_to_end();
        });
    }

    fn composer<R>(
        &mut self,
        change: impl FnOnce(&mut Composer, &mut Window, &mut Context<Composer>) -> R,
    ) -> R {
        let result = self
            .window
            .update(&mut self.cx, |plait, window, cx| {
                plait.workspace.composers[THREAD]
                    .update(cx, |composer, cx| change(composer, window, cx))
            })
            .expect("window update");
        self.settle();
        result
    }

    /// Wait for the file read and the background ranking behind a suggestion list.
    fn suggestions(&mut self) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let names = self.composer(|composer, _, _| composer.suggestion_names());
            if !names.is_empty() {
                return names;
            }
            assert!(Instant::now() < deadline, "suggestions did not arrive");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// The composer at each height, and its two suggestion lists.
fn composer_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.fresh_composer(repository);
    harness.capture(&format!("composer-empty-{suffix}"));

    // Up walks the thread's own user messages; Down returns the empty draft.
    harness.composer(|composer, window, cx| {
        composer.press_arrow(false, window, cx);
        assert_eq!(composer.text(), "Go ahead.");
        assert_eq!(composer.saved_draft().0, "");
        composer.press_arrow(false, window, cx);
        assert!(composer.text().starts_with("Queue the status refresh"));
        composer.press_arrow(true, window, cx);
        composer.press_arrow(true, window, cx);
        assert_eq!(composer.text(), "");
    });
    // A chosen command reaches the window, which performs it.
    harness.update(|plait, window, cx| {
        let event =
            super::composer::ComposerEvent::Command(super::composer::CommandId::ShowChanges);
        plait.composer_event(&event, window, cx);
        assert_eq!(plait.workspace.tab, super::Workspace::Changes);
        let event =
            super::composer::ComposerEvent::Command(super::composer::CommandId::ShowConversation);
        plait.composer_event(&event, window, cx);
        assert_eq!(plait.workspace.tab, super::Workspace::Conversation);
    });

    let lines = |count: usize| {
        (1..=count)
            .map(|line| format!("Sample draft line {line}: keep the public signature as it is."))
            .collect::<Vec<_>>()
            .join("\n")
    };
    harness.composer(|composer, _, cx| composer.restore_draft(lines(6), 1, None, cx));
    harness.capture(&format!("composer-lines-{suffix}"));
    harness.update(|plait, window, cx| {
        window.focus(&plait.workspace.composers[THREAD].focus_handle(cx), cx);
    });
    harness.draw();
    // Both adjacent controls must lead back to the composer through normal Tab order.
    for (leave, return_key) in [("shift-tab", "tab"), ("tab", "shift-tab")] {
        harness.keystroke(leave);
        harness.update(|plait, window, cx| {
            assert_ne!(
                window.focused(cx),
                Some(plait.workspace.composers[THREAD].focus_handle(cx)),
                "{leave} leaves the composer when no completion is open"
            );
        });
        harness.keystroke(return_key);
        harness.update(|plait, window, cx| {
            assert_eq!(
                window.focused(cx),
                Some(plait.workspace.composers[THREAD].focus_handle(cx)),
                "{return_key} reaches the composer from its adjacent control"
            );
        });
    }
    // Far more lines than fit: the input stops growing and scrolls to its cursor.
    harness.composer(|composer, _, cx| composer.restore_draft(lines(60), 2, None, cx));
    harness.capture(&format!("composer-tall-{suffix}"));

    // A real mention: the list comes from the fixture repository through Git.
    harness.composer(|composer, window, cx| {
        composer.restore_draft(String::new(), 3, None, cx);
        composer.type_text("Compare @rev", window, cx);
    });
    harness.update(|plait, _, cx| plait.supply_files(cx));
    assert_eq!(harness.suggestions(), ["src/review.rs"]);
    harness.composer(|composer, window, cx| {
        assert!(composer.accept_suggestion(window, cx));
        assert_eq!(composer.text(), "Compare src/review.rs ");
        assert!(composer.suggestion_names().is_empty());
    });

    // A fuller sample list, to see eight rows, emphasis, and the list's edges.
    harness.composer(|composer, window, cx| {
        composer.restore_draft(String::new(), 4, None, cx);
        composer.set_files(super::composer::MentionFiles::Ready(sample_files()), cx);
        composer.type_text("Why does @s", window, cx);
    });
    assert!(harness.suggestions().len() > 8);
    harness.capture(&format!("composer-mention-{suffix}"));
    harness.composer(|composer, window, cx| {
        composer.restore_draft(String::new(), 5, None, cx);
        composer.type_text("@ui/se", window, cx);
    });
    assert_eq!(harness.suggestions()[0], "crates/plait/src/ui/session.rs");
    harness.capture(&format!("composer-mention-filtered-{suffix}"));

    harness.composer(|composer, window, cx| {
        composer.restore_draft(String::new(), 6, None, cx);
        composer.type_text("/", window, cx);
    });
    assert_eq!(harness.suggestions().len(), 5);
    harness.capture(&format!("composer-command-{suffix}"));
    harness.composer(|composer, window, cx| {
        composer.type_text("set", window, cx);
        assert_eq!(composer.suggestion_names()[0], "/settings: Open settings");
    });
    harness.capture(&format!("composer-command-filtered-{suffix}"));

    // Sample skills and templates follow the app's own commands and only insert text.
    harness.composer(|composer, window, cx| {
        use super::composer::AgentCommand;
        composer.set_agent_commands(
            std::sync::Arc::from([
                AgentCommand::new(
                    "skill:release-notes".into(),
                    "Write release notes from merged changes".into(),
                ),
                AgentCommand::new("review".into(), "<path> · Review a change".into()),
                AgentCommand::new("new".into(), "Shadowed by the app's own /new".into()),
            ]),
            cx,
        );
        composer.restore_draft(String::new(), 7, None, cx);
        composer.type_text("/", window, cx);
    });
    let listed = harness.suggestions();
    assert_eq!(
        listed.len(),
        7,
        "five local commands, then two of the person's"
    );
    assert!(listed[5].starts_with("/skill:release-notes") && listed[6].starts_with("/review"));
    harness.capture(&format!("composer-agent-commands-{suffix}"));
    harness.composer(|composer, window, cx| {
        composer.type_text("revi", window, cx);
        assert_eq!(
            composer.suggestion_names(),
            ["/review: <path> · Review a change"]
        );
        assert!(composer.accept_suggestion(window, cx));
        assert_eq!(
            composer.text(),
            "/review ",
            "choosing inserts the command and runs nothing"
        );
        assert!(composer.suggestion_names().is_empty());
    });
    harness.update(|plait, window, cx| window.focus(&plait.focus, cx));
    harness.connect(repository, false);
}

/// The light theme across the main views, and the theme settings in both.
fn theme_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    let force = |harness: &mut Harness, light: bool| {
        harness.update(move |plait, _, cx| {
            plait.themes.forced = Some(light);
            plait.apply_theme(cx);
        })
    };
    harness.connect(repository, true);
    force(harness, true);
    harness.capture(&format!("light-conversation-{suffix}"));
    harness.update(|plait, window, cx| plait.select_file(1, DiffMode::Unstaged, window, cx));
    harness.capture(&format!("light-changes-{suffix}"));
    harness.update(|plait, _, _| {
        plait.settings_open = true;
        plait.settings_tab = SettingsTab::Appearance;
    });
    harness.capture(&format!("light-settings-appearance-{suffix}"));
    force(harness, false);
    harness.capture(&format!("settings-appearance-{suffix}"));
    harness.update(|plait, _, _| plait.settings_open = false);
    // In memory only: scenes never write the preferences file.
    harness.update(|plait, _, cx| {
        plait.preferences.value.dark_theme = Some("Plait Dark".into());
        plait.apply_theme(cx);
    });
    harness.capture(&format!("plait-dark-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.preferences.value.dark_theme = None;
        plait.apply_theme(cx);
    });
    harness.connect(repository, false);
}

/// Settings sections, with no signed-in model, and after a default-model save failed.
fn settings_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.update(|plait, _, _| plait.settings_open = true);
    harness.capture(&format!("settings-{suffix}"));
    harness.update(|plait, _, _| {
        for model in &mut plait.host.connection.status.as_mut().unwrap().models {
            model.authenticated = false;
        }
    });
    harness.capture(&format!("settings-no-auth-{suffix}"));
    harness.update(|plait, _, _| {
        plait.host.connection.status = Some(status());
        // As a failed save leaves it: the choice is kept, the host's default unchanged.
        plait.host.connection.settings_model = Some(other_choice());
        plait
            .host
            .finish_save(Err("sample error; the host did not answer.".into()));
    });
    harness.capture(&format!("settings-save-failed-{suffix}"));
    harness.update(|plait, _, _| {
        plait.host.connection.settings_model = Some(choice());
        plait.host.connection.save_message = None;
        plait.settings_open = false;
    });
}

fn set_query(harness: &mut Harness, query: &'static str) {
    harness.update(move |plait, _, cx| {
        let overlay = plait.overlay.as_ref().expect("open overlay");
        overlay
            .palette
            .update(cx, |palette, cx| palette.set_query(query, cx));
    });
}

/// The command palette: everything grouped, a ranked query, and no matches.
fn palette_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.update(|plait, window, cx| plait.toggle_command_palette(window, cx));
    harness.capture(&format!("palette-{suffix}"));
    set_query(harness, "new");
    harness.capture(&format!("palette-query-{suffix}"));
    set_query(harness, "zzzz");
    harness.capture(&format!("palette-no-matches-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.toggle_command_palette(window, cx);
        assert!(plait.overlay.is_none());
    });
}

/// The model picker: favorites, recents, and provider groups; a search that
/// mixes signed-in and signed-out models; the signed-out groups; no matches.
fn picker_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.update(|plait, window, cx| {
        // In memory only: the scene does not write a preferences file.
        let preferences = &mut plait.preferences.value;
        preferences.toggle_favorite(&other_choice());
        preferences.toggle_favorite(&ModelChoice {
            provider: "google".into(),
            model_id: "gemini-3-pro".into(),
        });
        preferences.record_recent(&ModelChoice {
            provider: "openai-codex".into(),
            model_id: "gpt-6-astra".into(),
        });
        plait.open_model_picker(ModelPurpose::NewThread, window, cx);
    });
    harness.capture(&format!("model-picker-{suffix}"));
    set_query(harness, "pro");
    harness.capture(&format!("model-picker-query-{suffix}"));
    set_query(harness, "zzzz");
    harness.capture(&format!("model-picker-no-matches-{suffix}"));
    set_query(harness, "");
    harness.update(|plait, _, cx| {
        let overlay = plait.overlay.as_ref().expect("open picker");
        overlay.palette.update(cx, |palette, cx| {
            palette.move_by(true, 12, cx);
            palette.move_by(true, 12, cx);
        });
    });
    harness.capture(&format!("model-picker-scrolled-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        // Disconnected: the picker says why it has nothing to list.
        plait.host.connection.connected = false;
        plait.open_model_picker(ModelPurpose::DefaultModel, window, cx);
    });
    harness.capture(&format!("model-picker-disconnected-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.preferences.value = Default::default();
    });
}

/// A thread creation the host has not confirmed, with its retry.
fn new_thread_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.update(|plait, _, _| {
        plait.host.creation.pending_create = Some(("sample-request".into(), choice()));
        plait.host.connection.error =
            Some("Sample error. Retry creation to reuse the same request ID.".into());
    });
    harness.capture(&format!("new-thread-unconfirmed-{suffix}"));
    harness.update(|plait, _, _| {
        plait.host.creation.pending_create = None;
        plait.host.connection.error = None;
    });
}

/// The new-thread draft: its header row in both themes, each menu it opens,
/// a Codex draft, an unconfirmed creation, and the sidebar around it.
fn draft_scenes_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    // A second sidebar project, so the project list has a choice.
    let other = repository.parent().unwrap().join("sample-site");
    std::fs::create_dir_all(&other).unwrap();
    let other = other.canonicalize().unwrap();
    harness.update(move |plait, window, cx| {
        use plait_core::host::ThinkingLevel::*;
        let status = plait.host.connection.status.as_mut().unwrap();
        status.protocol_version = 4;
        status.models.truncate(6);
        // A reasoning model, so the header shows its thinking chip.
        let model = &mut status.models[0];
        model.reasoning = Some(true);
        model.thinking_levels = Some(vec![Off, Minimal, Low, Medium, High, Xhigh]);
        plait.inject_harnesses(Some(sample_harnesses("ready", Some(NEWER))));
        // In memory only: the scene does not write a preferences file.
        plait.preferences.value.navigation.open(&other);
        plait.rebuild_projects(cx);
        plait.open_draft(window, cx);
    });
    harness.type_draft("Add retries with backoff to the status refresh");
    harness.capture(&format!("draft-{suffix}"));
    let forced = harness
        .window
        .update(&mut harness.cx, |plait, _, _| plait.themes.forced)
        .expect("window update");
    let theme = move |harness: &mut Harness, light: Option<bool>| {
        harness.update(move |plait, _, cx| {
            plait.themes.forced = light;
            plait.apply_theme(cx);
        });
    };
    theme(harness, Some(true));
    harness.capture(&format!("light-draft-{suffix}"));
    theme(harness, forced);
    harness.update(|plait, window, cx| plait.open_draft_thinking(window, cx));
    capture_both(harness, &format!("draft-thinking-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.open_draft_projects(window, cx);
    });
    harness.capture(&format!("draft-projects-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.inject_harnesses(Some(sample_harnesses(
            "not_installed",
            Some("Install Codex, or choose its executable in Settings → Agents."),
        )));
        plait.open_draft_harnesses(window, cx);
    });
    harness.capture(&format!("draft-harnesses-unready-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.inject_harnesses(Some(sample_harnesses("ready", Some(NEWER))));
        plait.open_draft_harnesses(window, cx);
    });
    harness.capture(&format!("draft-harnesses-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.choose_draft_harness(HarnessId::Codex, cx);
        plait.focus_composer(window, cx);
    });
    harness.capture(&format!("draft-codex-{suffix}"));
    theme(harness, Some(true));
    harness.capture(&format!("light-draft-codex-{suffix}"));
    theme(harness, forced);
    harness
        .update(|plait, window, cx| plait.open_model_picker(ModelPurpose::NewThread, window, cx));
    harness.capture(&format!("draft-models-codex-{suffix}"));
    // Sent, and the host has not confirmed the thread. Injected, not sent.
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.choose_draft_harness(HarnessId::Pi, cx);
        many_drafts::set_pending(plait, "sample-request");
        plait.draft.pending.as_mut().unwrap().creating = false;
        plait.draft.error =
            Some("Sample error. Retry creation to reuse the same request ID.".into());
        plait.draft.sending = Some(super::new_thread::Sending {
            images: Vec::new(),
            generation: plait.draft.generation,
            request_id: "sample-request".into(),
            text: "Add retries with backoff to the status refresh".into(),
            revision: 1,
            approval: Some(ApprovalMode::Auto),
            choice: None,
        });
        plait.sync_composer(cx);
    });
    harness.capture(&format!("draft-unconfirmed-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.draft.pending = None;
        plait.draft.error = None;
        plait.draft.sending = None;
        plait.sync_composer(cx);
        // Set aside, so the sidebar shows threads without a draft.
        plait.discard_draft(window, cx);
    });
    // Discard selects its neighbour. Supply that thread's reply without a host.
    harness.connect(repository, false);
    // Resting on a project row shows its New thread button.
    harness.capture(&format!("sidebar-{suffix}"));
    harness.pointer(PlatformInput::MouseMove(MouseMoveEvent {
        position: point(px(120.), px(PROJECT_ROW_Y)),
        pressed_button: None,
        modifiers: Modifiers::default(),
    }));
    harness.capture(&format!("sidebar-project-hover-{suffix}"));
    // With Codex as the default in Settings → New threads, a new draft opens on it.
    harness.update(|plait, window, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 4;
        plait.inject_harnesses(Some(sample_harnesses("ready", Some(NEWER))));
        // In memory only: the scene does not write a preferences file.
        plait.preferences.value.default_harness = HarnessId::Codex;
        plait.open_draft(window, cx);
        assert_eq!(plait.draft_harness(), HarnessId::Codex);
    });
    harness.capture(&format!("draft-default-codex-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.discard_draft(window, cx);
        plait.preferences.value.default_harness = HarnessId::Pi;
    });
    harness.update(|plait, _, cx| {
        plait.inject_harnesses(None);
        let mut preferences = plait.preferences.value.clone();
        preferences.navigation.opened.clear();
        plait.preferences.value = preferences;
        plait.rebuild_projects(cx);
    });
    harness.connect(repository, false);
}

/// Where the first project row sits in the sidebar, below its controls.
const PROJECT_ROW_Y: f32 = 150.;

fn codex_choice() -> ModelChoice {
    ModelChoice {
        provider: "codex".into(),
        model_id: "sample-codex-model".into(),
    }
}

/// A version 4 host's harness list with Codex in `state`. Labeled sample data only.
fn sample_harnesses(state: &str, note: Option<&str>) -> plait_core::host::harness::Harnesses {
    let ready = state == "ready";
    let model = |id: &str, name: &str| {
        serde_json::json!({
            "provider": "codex", "model_id": id, "name": name, "authenticated": true,
            "reasoning": true, "thinking_levels": ["low", "medium", "high"],
            "context_window": 272000,
        })
    };
    serde_json::from_value(serde_json::json!({"harnesses": [
        {
            "id": "pi", "name": "Pi", "state": "ready", "note": null, "version": null,
            "supported": null, "path": null,
            "capabilities": {"steer": true, "approvals": true, "thinking": true, "usage": true},
            "protection": "Plait's command sandbox and approvals, and its guard on files you already changed.",
            "models": null, "modes": [], "default_mode": null,
        },
        {
            "id": "codex", "name": "Codex", "state": state, "note": note,
            "version": ready.then_some("0.170.0"),
            "supported": {"min": "0.169.0", "tested": "0.169.0"},
            "path": ready.then_some("/opt/sample/bin/codex"),
            "capabilities": {"steer": true, "approvals": true, "thinking": true, "usage": true},
            "protection": "Codex's own permission mode and sandbox. Plait's command policy and its guard on files you already changed do not apply.",
            "models": if ready {
                serde_json::json!([model("sample-codex-model", "Sample Codex Model"), model("sample-codex-mini", "Sample Codex Mini")])
            } else {
                serde_json::json!([])
            },
            "modes": [
                {"id": "read_only", "label": "Read only", "detail": "Sample: asks before any change; commands only read."},
                {"id": "ask", "label": "Ask", "detail": "Sample: asks when it needs more than its sandbox; commands can write this checkout, with no network."},
                {"id": "approve_for_me", "label": "Approve for me", "detail": "Sample: its own reviewer approves or blocks each request, inside the same sandbox."},
                {"id": "full_access", "label": "Full access", "detail": "Sample: never asks and has no sandbox; it can change any file and use the network."},
            ],
            "default_mode": "full_access",
        },
    ]}))
    .expect("sample harness list")
}

const NEWER: &str = "Newer than the 0.169.0 Plait was tested with. It should work; anything odd is worth reporting.";

/// Version 4 harnesses: a Codex thread's composer line, model menu, and
/// permissions, and Settings → Agents. The draft walk covers choosing one.
fn harnesses_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    use plait_core::host::{
        ThinkingLevel,
        harness::{HarnessId, ThreadHarness},
    };
    harness.fresh_composer(repository);
    harness.update(|plait, _, _| {
        let status = plait.host.connection.status.as_mut().unwrap();
        status.protocol_version = 4;
        // Only the named sample providers, so the Codex group is in view.
        status.models.truncate(6);
        plait.inject_harnesses(Some(sample_harnesses("ready", Some(NEWER))));
    });
    harness.change_snapshot(|snapshot| {
        snapshot.model = codex_choice();
        snapshot.thinking_level = Some(ThinkingLevel::Medium);
        snapshot.harness = Some(ThreadHarness {
            id: HarnessId::Codex,
            version: Some("0.169.0".into()),
            mode: Some("full_access".into()),
            reported: Some("full_access".into()),
        });
    });
    harness.update(|plait, _, cx| {
        plait.host.connection.threads[0].model = codex_choice();
        plait.host.connection.threads[0].harness = Some(HarnessId::Codex);
        plait.sync_composer(cx);
    });
    harness.capture(&format!("composer-codex-thread-{suffix}"));
    harness.update(|plait, window, cx| {
        assert!(plait.can_submit(), "a ready Codex model can send");
        // Plait's approvals are Pi's: a Codex message asks for none.
        assert_eq!(plait.next_approval_mode(), None);
        plait.open_thread_models(window, cx);
    });
    harness.capture(&format!("thread-models-codex-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.open_thread_permissions(window, cx);
    });
    harness.capture(&format!("thread-permissions-codex-{suffix}"));
    harness.update(|plait, window, cx| plait.close_overlay(true, window, cx));
    // The person asked for Ask; Codex's own configuration ran Read only.
    harness.change_snapshot(|snapshot| {
        let harness = snapshot.harness.as_mut().unwrap();
        harness.mode = Some("ask".into());
        harness.reported = Some("read_only".into());
    });
    harness.capture(&format!("composer-codex-reported-{suffix}"));
    harness.change_snapshot(|snapshot| {
        snapshot.harness.as_mut().unwrap().reported = Some("custom".into());
    });
    harness.capture(&format!("composer-codex-custom-{suffix}"));
    let status = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            plait.host.thread.snapshot.as_ref().unwrap().status
        })
        .expect("window update");
    // During a run the mode can still change; the line says when it applies.
    harness.change_snapshot(|snapshot| {
        let harness = snapshot.harness.as_mut().unwrap();
        harness.mode = Some("full_access".into());
        harness.reported = Some("full_access".into());
        snapshot.status = ThreadStatus::Running;
    });
    harness.capture(&format!("composer-codex-running-{suffix}"));
    harness.change_snapshot(move |snapshot| snapshot.status = status);
    harness.update(|plait, _, cx| {
        plait.inject_harnesses(Some(sample_harnesses(
            "signed_out",
            Some("Run codex login in a terminal, then Refresh in Settings → Agents."),
        )));
        plait.sync_composer(cx);
        assert!(!plait.can_submit(), "a signed-out Codex cannot send");
    });
    harness.capture(&format!("composer-codex-signed-out-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.inject_harnesses(Some(sample_harnesses("ready", Some(NEWER))));
        // Codex is the default for new threads. In memory only.
        plait.preferences.value.default_harness = HarnessId::Codex;
        plait.settings_open = true;
        plait.settings_tab = SettingsTab::Agents;
        plait.sync_composer(cx);
    });
    harness.capture(&format!("harness-settings-{suffix}"));
    // The default harness is a default for new threads, so it is under New threads.
    harness.update(|plait, _, _| plait.settings_tab = SettingsTab::NewThreads);
    harness.capture(&format!("harness-default-{suffix}"));
    harness.update(|plait, _, _| plait.settings_tab = SettingsTab::Agents);
    let forced = harness
        .window
        .update(&mut harness.cx, |plait, _, _| plait.themes.forced)
        .expect("window update");
    harness.update(|plait, _, cx| {
        plait.themes.forced = Some(true);
        plait.apply_theme(cx);
    });
    harness.capture(&format!("light-harness-settings-{suffix}"));
    harness.update(move |plait, _, cx| {
        plait.themes.forced = forced;
        plait.apply_theme(cx);
        plait.inject_harnesses(Some(sample_harnesses(
            "unsupported_version",
            Some("Codex 0.150.0 is older than Plait supports. Update Codex to 0.169.0 or newer."),
        )));
    });
    harness.capture(&format!("harness-settings-unready-{suffix}"));
    harness.update(|plait, _, _| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
    });
    harness.capture(&format!("harness-settings-old-host-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.inject_harnesses(None);
        plait.preferences.value.default_harness = HarnessId::Pi;
        plait.show_settings(false, window, cx);
        plait.settings_tab = SettingsTab::default();
    });
    harness.connect(repository, false);
}

fn claude_choice() -> ModelChoice {
    ModelChoice {
        provider: "claude".into(),
        model_id: "sample-claude-model".into(),
    }
}

/// What the host says before Claude Code threads are set up. Labeled sample data only.
const CLAUDE_SETUP: &str = "Sample: set up Claude Code threads to download Anthropic's Agent SDK from the npm registry to this Mac. Plait then runs your installed claude.";

/// The sample list with Claude Code after Codex, in `state`. Labeled sample data only.
fn claude_harnesses(state: &str, note: Option<&str>) -> plait_core::host::harness::Harnesses {
    let ready = state == "ready";
    let model = |id: &str, name: &str| {
        serde_json::json!({
            "provider": "claude", "model_id": id, "name": name, "authenticated": true,
            "reasoning": true, "thinking_levels": ["low", "medium", "high", "xhigh", "max"],
            "context_window": null, "input": ["text"],
        })
    };
    let mut list = sample_harnesses("ready", None);
    list.harnesses.push(
        serde_json::from_value(serde_json::json!({
            "id": "claude", "name": "Claude Code", "state": state, "note": note,
            "version": ready.then_some("2.1.326"),
            "supported": {"min": "2.1.290", "tested": "2.1.326"},
            "path": ready.then_some("/opt/sample/bin/claude"),
            "capabilities": {"steer": true, "approvals": true, "thinking": true, "usage": true},
            "protection": "Sample: the thread's Claude Code mode and your Claude Code settings. Plait's command sandbox and its guard on files you had already changed do not apply.",
            "models": if ready {
                serde_json::json!([model("sample-claude-model", "Sample Claude Model"), model("sample-claude-mini", "Sample Claude Mini")])
            } else {
                serde_json::json!([])
            },
            "modes": [
                {"id": "ask", "label": "Ask", "detail": "Sample: asks before each edit and command your settings do not already allow."},
                {"id": "accept_edits", "label": "Accept edits", "detail": "Sample: edits files in this checkout without asking, and asks before other commands."},
                {"id": "plan", "label": "Plan", "detail": "Sample: reads and plans without changing anything, and asks before it starts on the plan."},
                {"id": "auto", "label": "Auto", "detail": "Sample: its own safety check approves or blocks each request, and asks only when it cannot decide."},
                {"id": "bypass", "label": "Bypass", "detail": "Sample: never asks and has no sandbox; it can change any file and run any command."},
            ],
            "default_mode": "auto",
        }))
        .expect("sample Claude Code entry"),
    );
    list
}

/// Capture `name` in the current theme, then in Plait Light as `light-{name}`.
fn capture_both(harness: &mut Harness, name: &str) {
    harness.capture(name);
    let forced = harness
        .window
        .update(&mut harness.cx, |plait, _, _| plait.themes.forced)
        .expect("window update");
    harness.update(|plait, _, cx| {
        plait.themes.forced = Some(true);
        plait.apply_theme(cx);
    });
    harness.capture(&format!("light-{name}"));
    harness.update(move |plait, _, cx| {
        plait.themes.forced = forced;
        plait.apply_theme(cx);
    });
}

/// Settings → Agents in both themes. In a narrow window the Claude Code entry
/// sits below the window's height, so it is also captured scrolled to it.
fn claude_settings(harness: &mut Harness, state: &str, suffix: &str) {
    capture_both(
        harness,
        &format!("harness-settings-claude-{state}-{suffix}"),
    );
    if suffix == "narrow" {
        harness.update(|plait, _, _| plait.settings_scroll.set_offset(point(px(0.), px(-400.))));
        capture_both(
            harness,
            &format!("harness-settings-claude-{state}-scrolled-{suffix}"),
        );
        harness.update(|plait, _, _| plait.settings_scroll.set_offset(point(px(0.), px(0.))));
    }
}

/// Claude Code: Settings → Agents before, during, and after setup, a Claude
/// Code draft, and a Claude Code thread waiting for an approval.
fn claude_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    use plait_core::host::{ThinkingLevel, harness::ThreadHarness};
    harness.connect(repository, false);
    harness.update(|plait, _, cx| {
        let status = plait.host.connection.status.as_mut().unwrap();
        status.protocol_version = 4;
        status.models.truncate(6);
        plait.inject_harnesses(Some(claude_harnesses("setup_required", Some(CLAUDE_SETUP))));
        plait.settings_open = true;
        plait.settings_tab = SettingsTab::Agents;
        plait.sync_composer(cx);
    });
    claude_settings(harness, "setup", suffix);
    // Setting up: marked as sent, no host contacted.
    let request = harness
        .window
        .update(&mut harness.cx, |plait, _, cx| {
            let request = plait.begin_harnesses(Some(HarnessId::Claude));
            cx.notify();
            request
        })
        .expect("window update")
        .expect("a setup can start");
    harness.update(|plait, _, _| assert!(plait.harness_setting_up(HarnessId::Claude)));
    claude_settings(harness, "setting-up", suffix);
    harness.update(move |plait, _, cx| {
        let ready = super::harnesses::HarnessesView::new(claude_harnesses("ready", None));
        plait.finish_harnesses(request, Ok(ready), cx);
        assert!(!plait.harness_setting_up(HarnessId::Claude));
        // In memory only: the scene does not write a preferences file.
        plait.preferences.value.default_harness = HarnessId::Claude;
    });
    claude_settings(harness, "ready", suffix);
    // A new thread opens on Claude Code, the default now, in its default mode.
    harness.update(|plait, window, cx| {
        plait.show_settings(false, window, cx);
        plait.settings_tab = SettingsTab::default();
        plait.open_draft(window, cx);
        assert_eq!(plait.draft_harness(), HarnessId::Claude);
        assert_eq!(plait.draft_model(), Some(claude_choice()));
        assert_eq!(plait.draft_mode().as_deref(), Some("auto"));
        plait.focus_composer(window, cx);
    });
    harness.type_draft("Add retries with backoff to the status refresh");
    capture_both(harness, &format!("draft-claude-{suffix}"));
    harness.update(|plait, window, cx| plait.open_draft_permissions(window, cx));
    capture_both(harness, &format!("draft-permissions-claude-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.discard_draft(window, cx);
        plait.preferences.value.default_harness = HarnessId::Pi;
    });
    // An open Claude Code thread asks before a command.
    harness.connect(repository, true);
    harness.update(|plait, _, _| {
        let status = plait.host.connection.status.as_mut().unwrap();
        status.protocol_version = 4;
        status.models.truncate(6);
        plait.inject_harnesses(Some(claude_harnesses("ready", None)));
    });
    harness.change_snapshot(|snapshot| {
        snapshot.model = claude_choice();
        snapshot.thinking_level = Some(ThinkingLevel::High);
        snapshot.approval_mode = None;
        snapshot.harness = Some(ThreadHarness {
            id: HarnessId::Claude,
            version: Some("2.1.326".into()),
            mode: Some("ask".into()),
            reported: Some("ask".into()),
        });
        let approval = snapshot.pending_approval.as_mut().unwrap();
        approval.summary = "cargo test".into();
        approval.details = "Sample: run the test suite.".into();
        approval.risk = "Sample: Claude Code runs this command on your machine with your permissions, outside Plait's sandbox. Only this request is approved.".into();
    });
    harness.update(|plait, _, cx| {
        plait.host.connection.threads[0].model = claude_choice();
        plait.host.connection.threads[0].harness = Some(HarnessId::Claude);
        plait.sync_composer(cx);
        // Plait's approvals are Pi's: a Claude Code message asks for none.
        assert_eq!(plait.next_approval_mode(), None);
        let line = plait.thread_harness_line().unwrap();
        assert_eq!(line.label.as_ref(), "Claude Code 2.1.326");
        assert_eq!(line.mode.as_ref().unwrap().label.as_ref(), "Ask");
    });
    capture_both(harness, &format!("composer-claude-approval-{suffix}"));
    harness.update(|plait, window, cx| plait.open_thread_permissions(window, cx));
    capture_both(harness, &format!("thread-permissions-claude-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.close_overlay(true, window, cx);
        plait.inject_harnesses(None);
    });
    harness.connect(repository, false);
}

/// A setup reply lands only on the request and connection it answers, and a
/// failed setup shows what the host said.
#[core::prelude::v1::test]
fn a_setup_reply_lands_only_on_the_request_it_answers() {
    if !isolated("a_setup_reply_lands_only_on_the_request_it_answers") {
        return;
    }
    use super::harnesses::HarnessesView;
    use plait_core::host::Error;
    let scratch = std::env::temp_dir().join(format!("plait-setup-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    let view = |state: &str, note: Option<&str>| HarnessesView::new(claude_harnesses(state, note));
    let claude = |plait: &Plait| {
        let row = plait.harness_row(HarnessId::Claude).expect("listed");
        (
            row.ready,
            row.setup,
            row.note.as_ref().map(ToString::to_string),
        )
    };
    harness.update(|plait, _, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 4;
        plait.inject_harnesses(Some(claude_harnesses("setup_required", Some(CLAUDE_SETUP))));
        // An older host cannot be asked.
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        assert!(plait.begin_harnesses(Some(HarnessId::Claude)).is_none());
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 4;

        // One request at a time; the setup is shown on Claude Code's row only.
        let request = plait.begin_harnesses(Some(HarnessId::Claude)).unwrap();
        assert!(plait.harness_setting_up(HarnessId::Claude));
        assert!(!plait.harness_setting_up(HarnessId::Codex));
        assert!(plait.begin_harnesses(None).is_none());
        plait.finish_harnesses(request, Ok(view("ready", None)), cx);
        assert_eq!(claude(plait), (true, false, None));
        assert!(!plait.harness_setting_up(HarnessId::Claude));

        // A failed setup stays in setup with the host's note.
        plait.inject_harnesses(Some(claude_harnesses("setup_required", Some(CLAUDE_SETUP))));
        let request = plait.begin_harnesses(Some(HarnessId::Claude)).unwrap();
        let failed = "Sample: the download did not match the pinned checksum.";
        plait.finish_harnesses(request, Ok(view("setup_required", Some(failed))), cx);
        assert_eq!(claude(plait), (false, true, Some(failed.into())));

        // A request the host could not answer leaves the list and says why.
        let request = plait.begin_harnesses(Some(HarnessId::Claude)).unwrap();
        plait.finish_harnesses(request, Err(Error::Protocol("Sample failure.".into())), cx);
        assert_eq!(claude(plait), (false, true, Some(failed.into())));
        assert!(!plait.harness_setting_up(HarnessId::Claude));

        // The host reconnected while setting up: the reply is for the old connection.
        let request = plait.begin_harnesses(Some(HarnessId::Claude)).unwrap();
        plait.host.connection.epoch += 1;
        plait.reset_harnesses();
        plait.inject_harnesses(Some(claude_harnesses("setup_required", Some(CLAUDE_SETUP))));
        assert!(!plait.harness_setting_up(HarnessId::Claude));
        plait.finish_harnesses(request, Ok(view("ready", None)), cx);
        assert_eq!(claude(plait), (false, true, Some(CLAUDE_SETUP.into())));

        // A reply to an earlier request on this connection is dropped too.
        let earlier = plait.begin_harnesses(Some(HarnessId::Claude)).unwrap();
        plait.finish_harnesses(earlier, Err(Error::Protocol("Sample failure.".into())), cx);
        let latest = plait.begin_harnesses(None).unwrap();
        plait.finish_harnesses(earlier, Ok(view("ready", None)), cx);
        assert!(!claude(plait).0);
        plait.finish_harnesses(latest, Ok(view("ready", None)), cx);
        assert_eq!(claude(plait), (true, false, None));
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// The draft's harness chooses its models and permissions; Pi stays the default.
#[core::prelude::v1::test]
fn a_codex_draft_starts_a_codex_thread_in_its_mode() {
    if !isolated("a_codex_draft_starts_a_codex_thread_in_its_mode") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-harness-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 4;
        plait.inject_harnesses(Some(sample_harnesses("ready", None)));
        plait.open_draft(window, cx);
        // Pi and its saved default first.
        assert_eq!(plait.draft_harness(), HarnessId::Pi);
        assert_eq!(plait.draft_model(), Some(choice()));
        assert_eq!(plait.draft_mode(), None);
        // A Pi model is never offered as Codex, nor a Codex model as Pi.
        plait.choose_draft_model(HarnessId::Pi, codex_choice(), cx);
        assert_eq!(plait.draft_model(), Some(choice()));
        plait.choose_draft_approval(Some(ApprovalMode::Ask), cx);
        // Codex: its first model, unless one was used more recently; its default mode.
        plait.choose_draft_harness(HarnessId::Codex, cx);
        assert_eq!(plait.draft_model(), Some(codex_choice()));
        assert_eq!(
            plait.draft.approval, None,
            "permissions return to the default"
        );
        let mini = ModelChoice {
            provider: "codex".into(),
            model_id: "sample-codex-mini".into(),
        };
        // In memory only: the test does not write a preferences file.
        plait.preferences.value.record_recent(&mini);
        assert_eq!(plait.draft_model(), Some(mini.clone()));
        assert_eq!(plait.draft_mode().as_deref(), Some("full_access"));
        plait.choose_draft_mode(HarnessId::Codex, "yolo".into(), cx);
        assert_eq!(plait.draft_mode().as_deref(), Some("full_access"));
        plait.choose_draft_mode(HarnessId::Codex, "ask".into(), cx);
        assert_eq!(plait.draft_mode().as_deref(), Some("ask"));
    });
    harness.type_draft("Run in Codex");
    harness.keystroke("enter");
    harness.update(|plait, _, _| {
        assert!(plait.draft.pending.is_some());
        let request = &plait.draft.pending.as_ref().unwrap().request;
        assert!(matches!(request, plait_core::host::admission::Request::CreateAdmittedThread { harness: HarnessId::Codex, mode: Some(mode), model_id, .. } if mode == "ask" && model_id == "sample-codex-mini"));
        assert_eq!(plait.draft.sending.as_ref().unwrap().approval, None);
    });
    // No host runs here, so the creation stays unconfirmed; its retry keeps harness and mode.
    harness.creation_settles();
    harness.update(|plait, window, cx| {
        plait.retry_create(window, cx);
        assert!(matches!(&plait.draft.pending.as_ref().unwrap().request, plait_core::host::admission::Request::CreateAdmittedThread { harness: HarnessId::Codex, mode: Some(mode), .. } if mode == "ask"));
    });
    harness.creation_settles();
    harness.update(|plait, window, cx| {
        plait.discard_draft(window, cx);
        plait.host.creation.pending_create = None;
        plait.open_draft(window, cx);
        // An unready harness cannot be chosen.
        plait.inject_harnesses(Some(sample_harnesses("signed_out", None)));
        plait.choose_draft_harness(HarnessId::Codex, cx);
        assert_eq!(plait.draft_harness(), HarnessId::Pi);
        // An older host lists no harnesses, so only Pi is offered.
        plait.inject_harnesses(Some(sample_harnesses("ready", None)));
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        plait.choose_draft_harness(HarnessId::Codex, cx);
        assert_eq!(plait.draft_harness(), HarnessId::Pi);
        assert!(plait.harness_rows().is_empty());
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A draft starts in the default harness from Settings → New threads while it is
/// unavailable; a harness chosen in one draft leaves the default alone.
#[core::prelude::v1::test]
fn a_draft_keeps_its_default_harness_when_unavailable() {
    if !isolated("a_draft_keeps_its_default_harness_when_unavailable") {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-default-harness-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    let mini = ModelChoice {
        provider: "codex".into(),
        model_id: "sample-codex-mini".into(),
    };
    harness.update(|plait, window, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 4;
        plait.inject_harnesses(Some(sample_harnesses("ready", None)));
        // `Other` has no name to save, so it is never the default.
        plait.set_default_harness(HarnessId::Other, cx);
        assert_eq!(plait.preferences.value.default_harness, HarnessId::Pi);
        plait.set_default_harness(HarnessId::Codex, cx);
        assert_eq!(plait.preferences.value.default_harness, HarnessId::Codex);
        plait.open_draft(window, cx);
        // Codex's first model and its default mode.
        assert_eq!(plait.draft_harness(), HarnessId::Codex);
        assert_eq!(plait.draft_model(), Some(codex_choice()));
        assert_eq!(plait.draft_mode().as_deref(), Some("full_access"));
        // The sidebar's New thread row names the model the draft starts with.
        let named = plait.new_thread_model().map(|model| model.choice());
        assert_eq!(named, Some(codex_choice()));
        // The model of Codex used most recently, once there is one.
        plait.preferences.value.record_recent(&mini);
        assert_eq!(plait.draft_model(), Some(mini.clone()));
        // Choosing Pi in this draft keeps the default, and Pi's own defaults apply.
        plait.choose_draft_harness(HarnessId::Pi, cx);
        assert_eq!(plait.draft_harness(), HarnessId::Pi);
        assert_eq!(plait.draft_model(), Some(choice()));
        assert_eq!(plait.draft_mode(), None);
        assert_eq!(plait.preferences.value.default_harness, HarnessId::Codex);
        // The next draft starts in the default again.
        plait.discard_draft(window, cx);
        plait.open_draft(window, cx);
        assert_eq!(plait.draft_harness(), HarnessId::Codex);
        // A choice made in the draft keeps its harness when the default changes.
        plait.choose_draft_mode(HarnessId::Codex, "ask".into(), cx);
        plait.set_default_harness(HarnessId::Pi, cx);
        assert_eq!(plait.draft_harness(), HarnessId::Codex);
        assert_eq!(plait.draft_mode().as_deref(), Some("ask"));
        plait.set_default_harness(HarnessId::Codex, cx);
        plait.discard_draft(window, cx);
        plait.open_draft(window, cx);
        // Not ready: keep the requested harness and block sending.
        plait.inject_harnesses(Some(sample_harnesses("signed_out", None)));
        assert_eq!(plait.draft_harness(), HarnessId::Codex);
        assert!(plait.draft_blocked().is_some());
        assert_eq!(plait.preferences.value.default_harness, HarnessId::Codex);
        // Ready again before anything was chosen: the draft follows the default.
        plait.inject_harnesses(Some(sample_harnesses("ready", None)));
        assert_eq!(plait.draft_harness(), HarnessId::Codex);
        // An older host cannot silently replace the configured harness.
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        assert_eq!(plait.draft_harness(), HarnessId::Codex);
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn a_mode_reply_lands_only_on_the_thread_it_was_sent_for() {
    if !isolated("a_mode_reply_lands_only_on_the_thread_it_was_sent_for") {
        return;
    }
    use plait_core::host::{
        Error, ThreadReply,
        harness::{HarnessId, ThreadHarness},
    };
    let scratch = std::env::temp_dir().join(format!("plait-mode-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, _, _| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 4;
        plait.inject_harnesses(Some(sample_harnesses("ready", None)));
    });
    let codex = |mode: &str, reported: Option<&str>| ThreadHarness {
        id: HarnessId::Codex,
        version: Some("0.169.0".into()),
        mode: Some(mode.into()),
        reported: reported.map(Into::into),
    };
    harness.change_snapshot(|snapshot| {
        snapshot.model = codex_choice();
        snapshot.harness = Some(codex("full_access", Some("full_access")));
    });
    let reply = |mode: &str| -> Result<ThreadReply, Error> {
        Ok(ThreadReply {
            thread: Snapshot {
                id: THREAD.into(),
                checkout: repository.clone(),
                // The host clears the report when the mode changes.
                harness: Some(codex(mode, None)),
                ..Default::default()
            },
        })
    };
    let shown = |plait: &Plait| {
        let mode = plait.thread_harness_line().unwrap().mode.as_ref().unwrap();
        (mode.id.clone(), mode.overridden)
    };
    harness.update(|plait, _, cx| {
        // Only a mode the harness lists is sent, and a run does not block a change.
        assert!(plait.host.thread.snapshot.as_ref().unwrap().status.busy());
        assert!(plait.begin_thread_mode("yolo".into()).is_none());
        let request = plait.begin_thread_mode("ask".into()).unwrap();
        // One change at a time.
        assert!(plait.begin_thread_mode("read_only".into()).is_none());
        plait.finish_thread_mode(request, reply("ask"), cx);
        // The latest run's report was for the old mode, so it is not an override.
        assert_eq!(shown(plait), ("ask".into(), false));
        assert!(plait.workspace.thread_controls.note.is_none());

        // A reply naming another mode is reported, not shown as chosen.
        let request = plait.begin_thread_mode("read_only".into()).unwrap();
        plait.finish_thread_mode(request, reply("ask"), cx);
        assert_eq!(shown(plait), ("ask".into(), false));
        assert!(plait.workspace.thread_controls.note.as_ref().unwrap().2);
        plait.workspace.thread_controls.note = None;

        // Sent, then the person opened another thread and came back: the
        // reply belongs to a watch that has ended, so it changes nothing.
        let saved = plait.host.thread.snapshot.clone();
        let request = plait.begin_thread_mode("read_only".into()).unwrap();
        plait.host.select(Some("thr_0c44a9d1".into()));
        plait.host.select(Some(THREAD.into()));
        plait.host.thread.snapshot = saved;
        plait.finish_thread_mode(request, reply("read_only"), cx);
        assert_eq!(shown(plait), ("ask".into(), false));
        assert!(plait.workspace.thread_controls.note.is_none());
        // A failure for a thread no longer open says nothing here either.
        let request = plait.begin_thread_mode("read_only".into()).unwrap();
        plait.host.select(Some("thr_0c44a9d1".into()));
        plait.finish_thread_mode(request, Err(Error::Protocol("late".into())), cx);
        assert!(plait.workspace.thread_controls.note.is_none());
    });
    harness.update(|plait, _, cx| {
        plait.host.select(Some(THREAD.into()));
        plait.host.thread.snapshot = Some(Arc::new(thread_snapshot(&repository, false)));
        plait.sync_composer(cx);
    });
    // The next run reports Full access although Ask was chosen: the harness overrode it.
    harness.change_snapshot(|snapshot| {
        snapshot.model = codex_choice();
        snapshot.harness = Some(codex("ask", Some("full_access")));
        snapshot.status = ThreadStatus::Running;
    });
    harness.update(|plait, _, _| assert_eq!(shown(plait), ("ask".into(), true)));
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// The Git inspector hidden. A narrow window has no inspector, so the toggle
/// explains where Git is instead.
fn inspector_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    harness.update(|plait, window, cx| {
        plait.toggle_inspector(window, cx);
        let wide = Plait::wide(window);
        assert_eq!(plait.preferences.value.hide_inspector, wide);
        assert_eq!(plait.workspace.inspector_note, !wide);
    });
    harness.capture(&format!("inspector-hidden-{suffix}"));
    harness.update(|plait, window, cx| {
        plait.toggle_inspector(window, cx);
        assert!(!plait.preferences.value.hide_inspector);
        assert!(!plait.workspace.inspector_note);
    });
    // Widths saved by dragging, fitted so the conversation keeps its room.
    harness.update(|plait, _, _| {
        plait.preferences.value.sidebar_width = Some(380);
        plait.preferences.value.inspector_width = Some(520);
    });
    harness.capture(&format!("panels-resized-{suffix}"));
    harness.update(|plait, _, _| {
        plait.preferences.value.sidebar_width = None;
        plait.preferences.value.inspector_width = None;
    });
}

// The glob import above brings in `gpui::test`; these are plain tests.

/// Layout and paint run for every view. GPUI reports some misuse, such as a
/// second hover style on one element, only when the element is drawn.
/// How this platform spells `key`, so label checks follow the binding rather
/// than one platform's modifier glyphs.
fn keys(key: &'static str) -> String {
    super::actions::label(&[Keystroke::parse(key).unwrap()])
}

#[core::prelude::v1::test]
fn every_view_draws() {
    if !isolated("every_view_draws") {
        return;
    }
    walk(None);
}

#[core::prelude::v1::test]
#[ignore = "writes PNG files; set PLAIT_SNAPSHOT_DIR and pass --ignored"]
fn snapshot() {
    if !isolated("snapshot") {
        return;
    }
    let Some(output) = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from) else {
        panic!("Set PLAIT_SNAPSHOT_DIR to an output directory.");
    };
    std::fs::create_dir_all(&output).unwrap();
    walk(Some(&output));
}

/// Settings controls use injected catalogs and inert sign-in fixtures.
fn settings_controls_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    use super::{
        actions,
        settings_providers::{EndpointForm, Form},
        text_field::TextField,
    };
    use plait_core::host::{policy::CommandPolicy, providers::*, sign_in::*};
    harness.connect(repository, false);
    harness.update(|plait, window, cx| {
        plait.show_settings(true, window, cx);
        plait.settings_tab = SettingsTab::Keyboard;
        plait
            .keyboard
            .search
            .as_ref()
            .unwrap()
            .update(cx, |field, cx| field.set_text("sidebar", cx));
    });
    harness.capture(&format!("keyboard-filtered-{suffix}"));
    let sidebar = actions::shortcuts()
        .iter()
        .position(|row| row.actions[0].id == "view.toggle_sidebar")
        .unwrap();
    harness.update(|plait, window, cx| plait.record_shortcut(sidebar, window, cx));
    harness.capture(&format!("keyboard-recording-{suffix}"));
    harness.keystroke("cmd-k");
    harness.update(|plait, _, _| {
        assert!(
            plait.overlay.is_none(),
            "recording intercepted the command palette"
        )
    });
    harness.capture(&format!("keyboard-conflict-{suffix}"));
    harness.keystroke("escape");
    harness.update(|plait, _, _| {
        assert!(
            plait.keyboard.recording.is_none(),
            "Escape dismisses a pending conflict"
        )
    });
    harness.update(|plait, window, cx| plait.record_shortcut(sidebar, window, cx));
    harness.draw();
    harness.keystroke("ctrl-shift-b");
    harness.update(|plait, _, _| assert!(plait.sidebar));
    harness.draw();
    harness.keystroke("cmd-b");
    harness.update(|plait, _, _| assert!(plait.sidebar, "old shortcut no longer runs"));
    harness.keystroke("ctrl-shift-b");
    harness.update(|plait, window, cx| {
        assert!(
            plait.sidebar,
            "the modal shields the workspace from the new shortcut"
        );
        plait.show_settings(false, window, cx);
    });
    harness.draw();
    harness.keystroke("ctrl-shift-b");
    harness.update(|plait, window, cx| {
        assert!(!plait.sidebar, "new shortcut runs after dismissal");
        plait.show_settings(true, window, cx);
    });
    harness.capture(&format!("keyboard-customized-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.sidebar = true;
        plait.preferences.value.keymap.clear();
        actions::install(cx, &plait.preferences.value.keymap);
        plait
            .keyboard
            .search
            .as_ref()
            .unwrap()
            .update(cx, |field, cx| field.set_text("", cx));
    });
    harness.capture(&format!("keyboard-default-{suffix}"));
    harness.update(|plait, _, _| {
        plait.settings_tab = SettingsTab::Providers;
        plait.providers.loaded = true;
        plait.providers.rows = vec![
            Provider {
                id: "sample-subscription".into(),
                name: "Sample subscription provider".into(),
                kind: ProviderKind::Builtin,
                authenticated: true,
                credential: Credential::SignIn,
                model_count: 3,
                sign_in: Some(SignInMethod {
                    name: "Sign in".into(),
                    subscription: true,
                }),
                api_key: Some(KeyMethod {
                    name: "API key".into(),
                }),
                endpoint: None,
            },
            Provider {
                id: "openrouter".into(),
                name: "Sample API provider".into(),
                kind: ProviderKind::Builtin,
                authenticated: false,
                credential: Credential::None,
                model_count: 8,
                sign_in: None,
                api_key: Some(KeyMethod {
                    name: "API key".into(),
                }),
                endpoint: None,
            },
        ];
    });
    harness.capture(&format!("providers-list-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.providers.form = Some(Form::Key {
            provider: "sample-provider".into(),
            field: cx.new(|cx| TextField::new("Provider API key", "Paste here", cx).secret()),
        });
    });
    harness.update(|plait, window, cx| {
        let Some(Form::Key { field, .. }) = &plait.providers.form else {
            panic!("key form");
        };
        field.update(cx, |field, cx| field.set_text("synthetic credential", cx));
        window.focus(&field.focus_handle(cx), cx);
        cx.write_to_clipboard(ClipboardItem::new_string(
            "untouched fixture clipboard".into(),
        ));
    });
    harness.draw();
    harness.keystroke("cmd-a");
    harness.keystroke("cmd-c");
    harness.update(|plait, _, cx| {
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("untouched fixture clipboard")
        );
        let Some(Form::Key { field, .. }) = &plait.providers.form else {
            panic!("key form");
        };
        let sent = field.update(cx, |field, cx| field.take_secret(cx));
        assert!(!sent.is_empty());
        assert!(field.read(cx).text().is_empty());
    });
    harness.keystroke("cmd-z");
    harness.update(|plait, _, cx| {
        let Some(Form::Key { field, .. }) = &plait.providers.form else {
            panic!("key form");
        };
        assert!(
            field.read(cx).text().is_empty(),
            "a sent key cannot be restored with undo"
        );
    });
    harness.capture(&format!("provider-api-key-{suffix}"));
    for (name, state, code, input, error) in [
        ("browser", SignInState::Waiting, None, None, None),
        (
            "device",
            SignInState::Waiting,
            Some("SAMPLE-CODE".to_owned()),
            None,
            None,
        ),
        (
            "paste",
            SignInState::Waiting,
            None,
            Some(SignInInput {
                kind: InputKind::Code,
                message: "Paste the sample sign-in code".into(),
                placeholder: None,
                required: false,
                choices: vec![],
            }),
            None,
        ),
        (
            "failed",
            SignInState::Failed,
            None,
            None,
            Some(Failure {
                code: "sample_failure".into(),
                message: "Sample sign-in was not completed.".into(),
            }),
        ),
    ] {
        harness.update(|plait, _, cx| {
            plait.providers.rows.clear();
            plait.providers.form = Some(Form::Flow {
                state: SignIn {
                    id: "sample-flow".into(),
                    provider: "sample-provider".into(),
                    state,
                    url: Some("https://example.invalid/sign-in".into()),
                    user_code: code,
                    input,
                    message: None,
                    expires_in_seconds: 120,
                    error,
                },
                field: cx.new(|cx| TextField::new("Sign-in answer", "Paste here", cx).secret()),
            });
        });
        harness.capture(&format!("provider-sign-in-{name}-{suffix}"));
    }
    harness.update(|plait, _, cx| {
        let field = |label, text: &str, cx: &mut Context<Plait>| {
            cx.new(|cx| {
                let mut f = TextField::new(label, "", cx);
                f.set_text(text, cx);
                f
            })
        };
        plait.providers.form = Some(Form::Endpoint(EndpointForm {
            id: None,
            name: field("Endpoint name", "Sample local server", cx),
            url: field("Endpoint base URL", "http://localhost:8000/v1", cx),
            models: field("Endpoint model IDs", "sample-model", cx),
            key: cx.new(|cx| TextField::new("Endpoint API key", "Optional", cx).secret()),
            checked: Some((
                "http://localhost:8000/v1".into(),
                EndpointCheck {
                    reachable: true,
                    reason: None,
                    status: Some(200),
                    model_ids: vec!["sample-model".into(), "sample-alternative".into()],
                    truncated: false,
                },
            )),
        }));
    });
    harness.capture(&format!("provider-endpoint-{suffix}"));
    harness.update(|plait, window, cx| {
        let Some(Form::Endpoint(form)) = &plait.providers.form else {
            panic!("endpoint form");
        };
        window.focus(&form.name.focus_handle(cx), cx);
    });
    harness.draw();
    for (keystroke, field) in [
        ("tab", "address"),
        ("tab", "models"),
        ("tab", "key"),
        ("shift-tab", "models"),
        ("shift-tab", "address"),
        ("shift-tab", "name"),
    ] {
        harness.keystroke(keystroke);
        harness.update(|plait, window, cx| {
            let Some(Form::Endpoint(form)) = &plait.providers.form else {
                panic!("endpoint form");
            };
            let target = match field {
                "name" => &form.name,
                "address" => &form.url,
                "models" => &form.models,
                "key" => &form.key,
                _ => unreachable!(),
            };
            assert_eq!(
                window.focused(cx),
                Some(target.focus_handle(cx)),
                "{keystroke} moves to the endpoint {field} field"
            );
        });
    }
    harness.update(|plait, _, cx| {
        let Some(Form::Endpoint(form)) = &plait.providers.form else {
            panic!("endpoint form");
        };
        assert!(form.current_check(cx).is_some());
        form.url.update(cx, |field, cx| {
            field.set_text("http://localhost:9000/v1", cx)
        });
        assert!(
            form.current_check(cx).is_none(),
            "a completed check only describes its tested address"
        );
    });
    harness.capture(&format!("provider-endpoint-edited-{suffix}"));
    if suffix == "narrow" {
        harness.pointer(PlatformInput::Scroll(ScrollEvent {
            position: point(px(600.), px(500.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-450.))),
            ..Default::default()
        }));
        harness.draw();
        harness.update(|plait, _, _| {
            assert!(
                plait.settings_scroll.offset().y < px(0.),
                "the endpoint form scrolls to its lower actions"
            );
        });
        harness.capture("provider-endpoint-scrolled-narrow");
    }
    // The command policy is the project's, on the project's own page.
    harness.update(|plait, window, cx| {
        plait.show_project_settings(window, cx);
        assert!(plait.providers.form.is_none(), "the provider form closes");
        assert_eq!(plait.settings_page, SettingsPage::Project);
        plait.policy.key = Some((
            plait.host.connection.epoch,
            plait.host.creation.checkout.clone(),
        ));
        plait.policy.value = Some(CommandPolicy::Sandboxed);
    });
    harness.update(|plait, _, _| {
        assert_eq!(plait.command_policy(), Some(CommandPolicy::Sandboxed));
        plait.policy.key.as_mut().unwrap().0 += 1;
        assert_eq!(
            plait.command_policy(),
            None,
            "a stale policy must not label this connection sandboxed"
        );
        plait.policy.key.as_mut().unwrap().0 -= 1;
    });
    harness.capture(&format!("policy-sandboxed-{suffix}"));
    harness.update(|plait, _, _| plait.policy.confirm = true);
    harness.capture(&format!("policy-confirm-trusted-{suffix}"));
    harness.update(|plait, _, _| {
        plait.policy.confirm = false;
        plait.policy.value = Some(CommandPolicy::Trusted);
    });
    harness.capture(&format!("policy-trusted-{suffix}"));
    harness.update(|plait, _, _| {
        use plait_core::host::agent::*;
        let skill = |name: &str, description: &str, origin, model_invocation| Skill {
            name: name.into(),
            description: description.into(),
            origin,
            path: format!("/opt/sample-agent/skills/{name}/SKILL.md").into(),
            model_invocation,
        };
        let sample = AgentResources {
            mcp_servers: vec![],
            checkout: plait.host.creation.checkout.clone(),
            trusted: false,
            instructions: Some(Instructions {
                path: "/opt/sample-agent/AGENTS.md".into(),
                bytes: 2150,
            }),
            skills: (0..14)
                .map(|index| {
                    skill(
                        &format!("sample-skill-{index:02}"),
                        "A sample skill description, long enough to show how one line is cut short when a description runs on past the width of the settings column.",
                        Origin::User,
                        index != 1,
                    )
                })
                .chain([skill("packaged", "From an installed Pi package.", Origin::Package, true)])
                .collect(),
            templates: vec![Template {
                name: "review".into(),
                description: "Review a change".into(),
                argument_hint: Some("<path> [focus]".into()),
                origin: Origin::User,
                path: "/opt/sample-agent/prompts/review.md".into(),
            }],
            problems: vec![Problem {
                path: Some("/opt/sample-agent/skills/broken/SKILL.md".into()),
                message: "description is required".into(),
            }],
        };
        plait.agent.view = Some(std::sync::Arc::new(super::settings_agent::AgentView::new(sample)));
    });
    harness.capture(&format!("project-settings-{suffix}"));
    // The Pi setup's lists, below the command policy.
    harness.update(|plait, _, _| plait.settings_scroll.set_offset(point(px(0.), px(-260.))));
    harness.capture(&format!("agent-settings-{suffix}"));
    harness.update(|plait, window, cx| {
        // Cmd-comma from the project's page opens the application's, not closes.
        plait.toggle_settings(window, cx);
        assert!(plait.settings_open);
        assert_eq!(plait.settings_page, SettingsPage::Application);
        plait.toggle_settings(window, cx);
        assert!(!plait.settings_open);
        plait.settings_tab = SettingsTab::default();
    });
}

/// Version 3 input and context controls. All state is synthetic and no socket is opened.
fn composer_v3_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    use super::composer::ComposerEvent;
    use plait_core::host::{QueueMode, Queued, ThinkingLevel, Usage};
    harness.fresh_composer(repository);
    harness.update(|plait, _, cx| {
        let status = plait.host.connection.status.as_mut().unwrap();
        status.protocol_version = 3;
        let model = status
            .models
            .iter_mut()
            .find(|m| m.choice() == choice())
            .unwrap();
        model.name = "Sample model with an exceptionally long readable model name".into();
        model.reasoning = Some(true);
        model.thinking_levels = Some(vec![
            ThinkingLevel::Off,
            ThinkingLevel::Low,
            ThinkingLevel::High,
        ]);
        model.context_window = Some(200_000);
        Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap()).thinking_level =
            Some(ThinkingLevel::Low);
        plait.host.submissions = Default::default();
        plait.sync_composer(cx);
    });
    harness.capture(&format!("composer-v3-idle-{suffix}"));
    // What the thread changed, above its composer. Injected sample counts.
    let root = repository.to_owned();
    harness.update(move |plait, _, cx| {
        let count = |path: &str, added, removed, untracked| plait_core::git::LineCount {
            path: path.into(),
            added,
            removed,
            untracked,
        };
        plait.workspace.thread_files.inject(
            THREAD,
            root,
            vec![
                count(
                    "crates/plait-core/src/git/worktree.rs",
                    Some(102),
                    Some(0),
                    true,
                ),
                count("crates/plait/src/ui/worktree.rs", Some(137), Some(4), false),
                count("docs/ROADMAP.md", Some(12), Some(3), false),
                count("assets/icon.png", None, None, false),
            ],
        );
        cx.notify();
    });
    harness.capture(&format!("composer-v3-changed-files-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.workspace.thread_files.collapsed = true;
        cx.notify();
    });
    harness.capture(&format!("composer-v3-changed-files-collapsed-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.workspace.thread_files = Default::default();
        cx.notify();
    });
    harness.composer(|composer, window, cx| {
        let revision = composer.saved_draft().1;
        assert!(composer.restore_cancelled("Restored exact message", revision, cx));
        composer.restore_draft(String::new(), revision + 2, None, cx);
        let empty_revision = composer.saved_draft().1;
        composer.type_text("Newer draft", window, cx);
        assert!(!composer.restore_cancelled("Earlier cancelled message", empty_revision, cx));
        assert_eq!(composer.text(), "Newer draft");
        composer.restore_draft(String::new(), empty_revision + 2, None, cx);
    });
    harness.update(|plait, window, cx| plait.open_thread_models(window, cx));
    harness.capture(&format!("composer-v3-model-picker-{suffix}"));
    harness.update(|plait, window, cx| plait.close_overlay(true, window, cx));
    harness.update(|plait, window, cx| plait.open_thinking(window, cx));
    harness.capture(&format!("composer-v3-thinking-picker-{suffix}"));
    harness.update(|plait, window, cx| plait.close_overlay(true, window, cx));
    // A thread's own approval mode overrides the default for what it sends,
    // and returning to the default follows Settings again.
    harness.update(|plait, _, cx| {
        let id = plait.host.thread.selected.clone().expect("open thread");
        let default = plait.default_approval();
        let other = match default {
            ApprovalMode::Ask => ApprovalMode::Auto,
            ApprovalMode::Auto => ApprovalMode::Ask,
        };
        plait.set_thread_approval(id.clone(), Some(other), cx);
        assert_eq!(plait.next_approval_mode(), Some(other));
        plait.set_thread_approval(id.clone(), None, cx);
        assert_eq!(plait.next_approval_mode(), Some(default));
        plait.set_thread_approval(id, Some(ApprovalMode::Ask), cx);
    });
    harness.update(|plait, window, cx| plait.open_run_settings(window, cx));
    harness.capture(&format!("composer-v3-run-settings-{suffix}"));
    harness.update(|plait, window, cx| plait.close_overlay(true, window, cx));
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Running;
        snapshot.queued = [QueueMode::FollowUp, QueueMode::Steer]
            .into_iter()
            .enumerate()
            .map(|(i, mode)| Queued {
                id: format!("request-queue-{i}"),
                request_id: format!("queue-{i}"),
                text: if i == 0 {
                    "Then check the edge cases in the parser.".into()
                } else {
                    "Keep the existing public API unchanged.".into()
                },
                mode,
                approval: ApprovalMode::Ask,
            })
            .collect();
    });
    let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorded = events.clone();
    harness.update(move |plait, _, cx| {
        let composer = plait.workspace.composers[THREAD].clone();
        cx.subscribe(&composer, move |_, _, event, _| {
            if let ComposerEvent::Submit {
                text,
                revision,
                steer,
            } = event
            {
                recorded
                    .borrow_mut()
                    .push((text.clone(), *revision, *steer));
            }
        })
        .detach();
        plait.sync_composer(cx);
    });
    harness
        .composer(|composer, window, cx| composer.type_text("Sample follow-up draft", window, cx));
    harness.capture(&format!("composer-v3-queued-{suffix}"));
    harness.keystroke("enter");
    harness.draw();
    harness.keystroke("alt-enter");
    assert_eq!(
        events.borrow().len(),
        2,
        "both busy gestures emit without a host"
    );
    assert!(!events.borrow()[0].2);
    assert!(events.borrow()[1].2);
    assert_eq!(events.borrow()[0].0, "Sample follow-up draft");
    assert_eq!(events.borrow()[0].1, events.borrow()[1].1);
    harness.change_snapshot(|snapshot| snapshot.status = ThreadStatus::Interrupted);
    harness.update(|plait, _, cx| plait.sync_composer(cx));
    harness.capture(&format!("composer-v3-held-{suffix}"));
    harness.update(|plait, _, cx| {
        let pending = plait
            .host
            .submissions
            .begin_with(
                THREAD,
                "Sample message with an unconfirmed response".into(),
                Some(ApprovalMode::Ask),
                Some(QueueMode::FollowUp),
                Some(900),
            )
            .unwrap();
        plait
            .host
            .submissions
            .uncertain(&pending.request_id, "Sample disconnected socket".into());
        plait.sync_composer(cx);
    });
    harness.capture(&format!("composer-v3-uncertain-{suffix}"));
    harness.update(|plait, _, cx| {
        let pending = plait
            .host
            .submissions
            .pending(THREAD)
            .unwrap()
            .request_id
            .clone();
        plait.host.submissions.start_send(THREAD, &pending);
        plait.sync_composer(cx);
    });
    harness.capture(&format!("composer-v3-sending-{suffix}"));
    harness.update(|plait, _, _| {
        for queued in &plait.host.thread.snapshot.as_ref().unwrap().queued {
            plait.host.submissions.restore_accepted(
                THREAD.into(),
                plait_core::host_state::PendingSubmit {
                    checkout_wait: false,
                    images: Vec::new(),
                    request_id: queued.request_id.clone(),
                    text: queued.text.clone(),
                    approval: Some(queued.approval),
                    when_busy: Some(queued.mode),
                    draft_revision: Some(100),
                },
            );
        }
    });
    harness.composer(|composer, _, cx| composer.restore_draft(String::new(), 1000, None, cx));
    harness.capture(&format!("composer-v3-edit-queued-{suffix}"));
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Running;
        let sample = snapshot.queued[0].clone();
        snapshot.queued = (0..16)
            .map(|index| Queued {
                request_id: format!("full-{index}"),
                id: format!("request-full-{index}"),
                ..sample.clone()
            })
            .collect();
    });
    harness.update(|plait, _, cx| plait.sync_composer(cx));
    harness.capture(&format!("composer-v3-full-queue-{suffix}"));
    harness.update(|plait, _, _| plait.host.submissions = Default::default());
    harness.change_snapshot(|snapshot| {
        snapshot.queued.clear();
        snapshot.status = ThreadStatus::Idle;
        snapshot.usage = Some(Usage {
            input: 1000,
            output: 500,
            cache_read: 8500,
            cache_write: 0,
            reasoning: Some(300),
            total: 10000,
            context_used: Some(10000),
            context_window: Some(200000),
        });
    });
    harness.update(|plait, _, cx| plait.sync_composer(cx));
    harness.capture(&format!("composer-v3-context-low-{suffix}"));
    harness
        .change_snapshot(|snapshot| snapshot.usage.as_mut().unwrap().context_used = Some(160000));
    harness.capture(&format!("composer-v3-context-high-{suffix}"));
    harness
        .change_snapshot(|snapshot| snapshot.usage.as_mut().unwrap().context_used = Some(190000));
    harness.capture(&format!("composer-v3-context-full-{suffix}"));
    harness.update(|plait, _, _| plait.workspace.thread_controls.usage_open = true);
    harness.capture(&format!("composer-v3-context-details-{suffix}"));
    harness.update(|plait, _, _| plait.workspace.thread_controls.usage_open = false);
    harness.update(|plait, _, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .models
            .iter_mut()
            .for_each(|m| m.authenticated = false);
        plait.sync_composer(cx);
    });
    harness.capture(&format!("composer-v3-model-unavailable-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.host.connection.connected = false;
        plait.sync_composer(cx);
    });
    harness.capture(&format!("composer-v3-disconnected-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.session.required = true;
        plait.session.loading = true;
        plait.sync_composer(cx);
    });
    harness.capture(&format!("composer-v3-recovery-{suffix}"));
    harness.update(|plait, _, _| {
        plait.session.required = false;
        plait.session.loading = false;
    });
    harness.connect(repository, false);
}

fn stored_page(
    items: Vec<plait_core::host::HistoryItem>,
    before: Option<&str>,
) -> plait_core::history::Page {
    plait_core::history::prepare_page(
        plait_core::host::HistoryPage {
            items,
            before: before.map(str::to_owned),
        },
        None,
        |_, _| unreachable!("fixture has inline text"),
    )
    .unwrap()
}

fn install_history(plait: &mut Plait, cache: plait_core::history::History) {
    let snapshot = cache.project(plait.host.thread.snapshot.as_ref().unwrap());
    let stored = snapshot.tools.iter().map(|tool| tool.id.clone()).collect();
    plait.workspace.formatted = Arc::new(plait_core::markdown::Transcript::update_stored(
        &plait.workspace.formatted,
        &snapshot,
        &stored,
    ));
    let previous = plait.workspace.history.display.replace(Arc::new(snapshot));
    plait.workspace.history.cache = Arc::new(cache);
    plait.workspace.history.enabled = true;
    plait
        .host
        .connection
        .status
        .as_mut()
        .unwrap()
        .protocol_version = 3;
    plait.reconcile_live_list(previous.as_deref());
}

fn stored_message(
    id: &str,
    role: MessageRole,
    text: &str,
    reasoning: &str,
) -> plait_core::host::HistoryItem {
    plait_core::host::HistoryItem::Message {
        images: Vec::new(),
        id: id.into(),
        role,
        text: text.into(),
        reasoning: reasoning.into(),
        more: vec![],
    }
}

fn history_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    use super::selection::Owner;
    use plait_core::{history::History, host::HistoryItem};
    harness.connect(repository, false);
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Idle;
        snapshot.messages = vec![Message {
            images: Vec::new(),
            id: "h-answer".into(),
            role: MessageRole::Assistant,
            text: "The saved reply is complete.".into(),
            reasoning: String::new(),
        }];
        snapshot.tools.clear();
        snapshot.live_text.clear();
    });
    let items = || {
        vec![
        stored_message("h-user", MessageRole::User, "Review the queue and describe the change.", ""),
        stored_message("h-think", MessageRole::Assistant, "", "Check the queue ownership first.\n\nThen follow the empty-queue transition and verify cancellation."),
        HistoryItem::Tool { id: "h-tool".into(), name: "edit".into(), status: ToolStatus::Done, input: "{\"path\":\"src/queue.rs\",\"old_text\":\"notify();\",\"new_text\":\"notify_when_ready();\"}".into(), output: "Replaced one occurrence.".into(), more: vec![] },
        stored_message("h-answer", MessageRole::Assistant, "The saved reply is complete.\n\nThe queue now notifies only after the next item becomes runnable. The reader keeps its place while earlier history arrives.", ""),
    ]
    };
    harness.update(|plait, _, _| {
        let mut cache = History::default();
        cache.latest(stored_page(items(), None));
        install_history(plait, cache);
        plait.workspace.live_scroll.scroll_to(ListOffset::default());
    });
    harness.capture(&format!("history-reasoning-collapsed-{suffix}"));
    harness.update(|plait, _, cx| plait.toggle_reasoning("h-think", cx));
    harness.capture(&format!("history-reasoning-expanded-{suffix}"));
    let position = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            plait.text_point(&Owner::Reasoning("h-think".into()), 0, 2)
        })
        .unwrap()
        .expect("reasoning visible");
    harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
        button: MouseButton::Left,
        position,
        click_count: 2,
        ..Default::default()
    }));
    harness.pointer(PlatformInput::MouseUp(MouseUpEvent {
        button: MouseButton::Left,
        position,
        click_count: 2,
        ..Default::default()
    }));
    harness.keystroke("cmd-c");
    harness.update(|plait, _, cx| {
        assert_eq!(plait.selected_text().as_deref(), Some("Check"));
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some("Check")
        );
    });
    harness.capture(&format!("history-reasoning-selection-{suffix}"));
    harness.update(|plait, _, _| {
        plait.clear_reading_selection();
        let mut cache = History::default();
        let long = format!("Beginning of the complete reply.\n\n{}\n\nEnd of the complete reply.", "This paragraph remains in stored history even when the live snapshot no longer contains its beginning.\n\n".repeat(65));
        cache.latest(stored_page(vec![stored_message("h-answer", MessageRole::Assistant, &long, "")], None));
        install_history(plait, cache); plait.workspace.live_scroll.scroll_to(ListOffset::default());
        assert!(plait.conversation_snapshot().unwrap().messages[0].text.len() > 4096);
    });
    harness.capture(&format!("history-full-message-{suffix}"));
    harness.update(|plait, _, _| {
        let text = "This is the first displayed part of a much larger stored message. Each part is explicit, and copying copies only the part on screen.";
        let item = HistoryItem::Message {
 images: Vec::new(), id: "h-answer".into(), role: MessageRole::Assistant, text: text.into(), reasoning: String::new(), more: vec![plait_core::host::More { field: plait_core::host::HistoryField::Text, reference: "fixture-part".into(), offset: text.len() as u64, length: 500_000 }] };
        let entry = plait_core::history::Entry::from_inline(item, None).unwrap();
        let mut cache = History::default(); cache.latest(plait_core::history::Page { entries: vec![Arc::new(entry)], before: None });
        install_history(plait, cache);
    });
    harness.capture(&format!("history-text-parts-{suffix}"));

    harness.update(|plait, _, _| {
        plait.workspace.history.loading = Some(super::history::Load::Older);
    });
    harness.capture(&format!("history-loading-{suffix}"));
    harness.update(|plait, _, _| {
        plait.workspace.history.loading = None;
        let mut cache = History::default();
        cache.latest(stored_page((10..35).map(|i| stored_message(&format!("history-{i}"), if i % 2 == 0 { MessageRole::User } else { MessageRole::Assistant }, &format!("Sample history message {i}. This is stored fixture content, not an agent run."), "")).collect(), Some("earlier")));
        install_history(plait, cache); plait.workspace.live_scroll.scroll_to(ListOffset { item_ix: 10, offset_in_item: px(5.) });
        // No host request from the fixture's scroll callback.
        plait.workspace.history.loading = Some(super::history::Load::Older);
    });
    harness.capture(&format!("history-scrolled-{suffix}"));
    harness.pointer(PlatformInput::Scroll(ScrollEvent {
        position: point(px(500.), px(200.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(100.))),
        ..Default::default()
    }));
    harness.draw();
    harness.update(|plait, _, _| {
        let before = plait.workspace.live_scroll.logical_scroll_top();
        let mut cache = (*plait.workspace.history.cache).clone();
        cache.prepend(stored_page(
            (0..10)
                .map(|i| {
                    stored_message(
                        &format!("history-{i}"),
                        MessageRole::Assistant,
                        &format!("Earlier fixture message {i}."),
                        "",
                    )
                })
                .collect(),
            None,
        ));
        install_history(plait, cache);
        let after = plait.workspace.live_scroll.logical_scroll_top();
        assert_eq!(after.item_ix, before.item_ix + 10);
        assert_eq!(after.offset_in_item, before.offset_in_item);
        plait.workspace.history.loading = None;
    });
    harness.capture(&format!("history-prepended-{suffix}"));
    harness.keystroke("home");
    harness.capture(&format!("history-beginning-{suffix}"));
    harness.update(|plait, _, _| {
        assert_eq!(plait.workspace.live_scroll.logical_scroll_top().item_ix, 0)
    });
    harness.keystroke("end");
    harness.update(|plait, _, _| {
        assert_eq!(
            plait.workspace.live_scroll.logical_scroll_top().item_ix,
            plait.workspace.live_count
        )
    });
    harness.connect(repository, false);
    harness.change_snapshot(|snapshot| { snapshot.live_text.clear(); snapshot.live_reasoning = "Checking how cancellation interacts with the next queued input.\n\nThe writer lease must be released before another run is admitted.".into(); });
    harness.update(|plait, _, _| {
        plait.workspace.history.enabled = true;
    });
    harness.capture(&format!("history-live-reasoning-{suffix}"));
    for (name, state, error) in [
        ("waiting", ThreadStatus::Running, None),
        ("finished", ThreadStatus::Idle, None),
        ("stopped", ThreadStatus::Stopped, None),
        ("interrupted", ThreadStatus::Interrupted, None),
        (
            "failed",
            ThreadStatus::Error,
            Some(HostError {
                code: "runtime_error".into(),
                message: "The provider did not complete the request. Try another message.".into(),
            }),
        ),
        (
            "turn-limit",
            ThreadStatus::Error,
            Some(HostError {
                code: "turn_limit".into(),
                message: "Limit".into(),
            }),
        ),
    ] {
        harness.change_snapshot(|snapshot| {
            snapshot.live_reasoning.clear();
            snapshot.live_text.clear();
            snapshot.tools.clear();
            snapshot.status = state;
            snapshot.error = error;
        });
        harness.capture(&format!("history-{name}-{suffix}"));
    }
    harness.change_snapshot(|snapshot| {
        snapshot.status = ThreadStatus::Running;
        snapshot.error = None;
        snapshot.retry = Some(plait_core::host::Retry {
            attempt: 2,
            at: 5000,
        });
    });
    harness.update(|plait, _, _| {
        plait.workspace.history.now = 2000;
    });
    harness.capture(&format!("history-retry-{suffix}"));
}

#[core::prelude::v1::test]
fn delayed_history_cannot_land_after_a_thread_switch() {
    if !isolated("delayed_history_cannot_land_after_a_thread_switch") {
        return;
    }
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };
    let scratch = std::env::temp_dir().join(format!("plait-history-guard-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = store.join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(socket.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.contains("history"));
        std::thread::sleep(Duration::from_millis(10));
        socket.write_all(b"{\"ok\":true,\"result\":{\"items\":[{\"type\":\"message\",\"id\":\"old\",\"role\":\"assistant\",\"text\":\"Wrong thread\",\"reasoning\":\"\"}],\"before\":null}}\n").unwrap();
    });
    harness
        .window
        .update(&mut harness.cx, |plait, window, cx| {
            plait.workspace.history.enabled = true;
            plait.load_history(super::history::Load::Latest, window, cx);
            // The request was scheduled for this thread; switch before its reply.
            plait.host.select(Some("other-thread".into()));
            plait.workspace.history = Default::default();
        })
        .unwrap();
    harness.settle();
    server.join().unwrap();
    for _ in 0..10 {
        harness.settle();
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.update(|plait, _, _| {
        assert!(!plait.workspace.history.cache.loaded);
        assert!(plait.workspace.history.display.is_none());
    });
    drop(harness);
    std::fs::remove_file(socket).unwrap();
    std::fs::remove_dir_all(scratch).unwrap();
}

fn navigation_walk(harness: &mut Harness, repository: &Path, suffix: &str) {
    harness.connect(repository, false);
    let root = repository.to_owned();
    harness.update(move |plait, _, cx| {
        plait.sidebar = true;
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        plait.preferences.value.navigation = Default::default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        plait.host.connection.threads = (0..28)
            .map(|index| {
                let checkout = match index % 3 {
                    0 => root.clone(),
                    1 => root.join("sample-api"),
                    _ => root.join("sample-long-project-name-that-must-truncate"),
                };
                ThreadSummary {
                    checkout_queue: None,
                    id: if index == 0 {
                        THREAD.into()
                    } else {
                        format!("sample-navigation-{index}")
                    },
                    checkout,
                    model: choice(),
                    title: Some(
                        [
                            "Review parser recovery and Unicode boundaries",
                            "Repair the failing tests",
                            "Explain deployment configuration",
                            "Update documentation",
                        ][index % 4]
                            .into(),
                    ),
                    status: [
                        ThreadStatus::Running,
                        ThreadStatus::Idle,
                        ThreadStatus::AwaitingApproval,
                        ThreadStatus::Error,
                        ThreadStatus::Stopped,
                        ThreadStatus::Interrupted,
                    ][index % 6],
                    created_at: Some(now - 86_400_000),
                    updated_at: Some(now - index as u64 * 180_000),
                    archived: Some(index == 3),
                    pending_approval: Some(index % 6 == 2),
                    harness: None,
                }
            })
            .collect();
        plait
            .preferences
            .value
            .navigation
            .observe(&plait.host.connection.threads, None, true);
        plait
            .preferences
            .value
            .navigation
            .unread
            .insert("sample-navigation-1".into());
        plait
            .preferences
            .value
            .navigation
            .unread
            .insert("sample-navigation-3".into());
        plait.rebuild_projects(cx);
        for project in &plait.state.projects {
            plait
                .navigation
                .available
                .insert(project.checkout.clone(), project.checkout == root);
        }
    });
    harness.capture(&format!("navigation-projects-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.navigation.attention = true;
        cx.notify();
    });
    harness.capture(&format!("navigation-attention-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.navigation.attention = false;
        plait.navigation.archived = true;
        cx.notify();
    });
    harness.capture(&format!("navigation-archived-{suffix}"));
    harness.update(|plait, window, cx| plait.begin_rename(THREAD.into(), window, cx));
    harness.capture(&format!("navigation-rename-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.navigation.rename = None;
        plait
            .navigation
            .filter
            .update(cx, |field, cx| field.set_text("repair", cx));
    });
    harness.capture(&format!("navigation-filter-{suffix}"));
    harness.update(|plait, _, cx| {
        plait
            .navigation
            .filter
            .update(cx, |field, cx| field.set_text("nothing matches this", cx))
    });
    harness.capture(&format!("navigation-empty-{suffix}"));
    harness.update(|plait, _, cx| {
        plait
            .navigation
            .filter
            .update(cx, |field, cx| field.set_text("", cx));
        plait.navigation.menu = Some(THREAD.into());
    });
    harness.capture(&format!("navigation-actions-{suffix}"));
    let project = format!("project:{}", repository.display());
    harness.update(move |plait, _, _| plait.navigation.menu = Some(project));
    harness.capture(&format!("navigation-project-actions-{suffix}"));
    // Threads in a Plait worktree list under the project it was made from.
    let main = repository.to_owned();
    harness.update(move |plait, _, cx| {
        plait.preferences.value.navigation.remember_worktree(
            main.join("sample-api"),
            main.clone(),
            "plait/3f9a2c".into(),
        );
        plait.rebuild_projects(cx);
        plait.navigation.menu = Some("sample-navigation-1".into());
    });
    harness.capture(&format!("navigation-worktree-{suffix}"));
    harness.update(|plait, _, cx| {
        plait.preferences.value.navigation.worktrees.clear();
        plait.rebuild_projects(cx);
    });
    harness.update(|plait, _, _| {
        plait.navigation.menu = None;
        plait.navigation.error = Some(
            "Thread updates paused: the host disconnected. Refresh to load current states.".into(),
        );
    });
    harness.capture(&format!("navigation-paused-{suffix}"));
    harness.update(|plait, _, _| plait.navigation.error = None);

    harness.update(|plait, _, cx| {
        plait.navigation.menu = None;
        plait.navigation.archived = false;
        plait.preferences.value.navigation = Default::default();
        plait.navigation.available.clear();
        plait.navigation.collapsed.clear();
        cx.notify();
    });
    harness.connect(repository, false);
    harness.update(|plait, _, cx| plait.rebuild_projects(cx));
    harness.capture(&format!("navigation-v2-{suffix}"));
}

#[core::prelude::v1::test]
fn project_switching_keeps_each_draft_and_rejects_the_old_watch() {
    if !isolated("project_switching_keeps_each_draft_and_rejects_the_old_watch") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-projects-ui-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    let other = repository.join("other");
    harness.update(|plait, window, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        plait.host.connection.threads.push(ThreadSummary {
            id: "other-thread".into(),
            checkout: other.clone(),
            model: choice(),
            title: Some("Other work".into()),
            ..Default::default()
        });
        plait
            .workspace
            .composers
            .get(THREAD)
            .unwrap()
            .update(cx, |composer, cx| {
                composer.restore_draft("Original draft".into(), 8, None, cx)
            });
        let old_epoch = plait.host.connection.epoch;
        let old_watch = plait.host.thread.watch_epoch;
        plait.host.connection.saving = true;
        plait.host.connection.save_message = Some("Saving default model…".into());
        plait.select_host_thread("other-thread".into(), window, cx);
        assert_eq!(plait.host.creation.checkout, other);
        assert_eq!(plait.state.repository.path(), other);
        assert!(!plait.host.accepts_watch(old_epoch, old_watch, THREAD));
        assert_eq!(plait.host.connection.epoch, old_epoch);
        assert!(plait.host.connection.saving);
        assert_eq!(
            plait.host.connection.save_message.as_deref(),
            Some("Saving default model…")
        );
        plait
            .workspace
            .composers
            .get("other-thread")
            .unwrap()
            .update(cx, |composer, cx| {
                composer.restore_draft("Separate draft".into(), 3, None, cx)
            });
        plait.select_host_thread(THREAD.into(), window, cx);
        assert_eq!(plait.host.creation.checkout, repository);
        assert_eq!(
            plait.workspace.composers[THREAD].read(cx).saved_draft().0,
            "Original draft"
        );
        assert_eq!(
            plait.session.data.as_ref().unwrap().threads["other-thread"]
                .draft
                .text,
            "Separate draft"
        );
        assert_eq!(plait.host.connection.threads.len(), 3);
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// Project Settings… targets a project without switching the thread behind it.
#[core::prelude::v1::test]
fn project_settings_open_for_the_chosen_project() {
    if !isolated("project_settings_open_for_the_chosen_project") {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-project-settings-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    let other = repository.join("other");
    let chosen = other.clone();
    harness.update(move |plait, window, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        // Even pending creation keeps its checkout while another project is configured.
        plait.host.creation.creating = true;
        plait.show_settings_for_project(chosen.clone(), window, cx);
        assert!(plait.settings_open);
        assert_eq!(plait.settings_checkout(), chosen);
        assert_ne!(plait.host.creation.checkout, chosen);
        plait.host.creation.creating = false;
        plait.notice = None;
        plait.show_settings_for_project(chosen.clone(), window, cx);
        assert_ne!(plait.host.creation.checkout, chosen);
        assert_eq!(
            plait.settings_checkout(),
            chosen,
            "the modal targets the chosen project"
        );
        assert!(plait.settings_open);
        assert_eq!(plait.settings_page, SettingsPage::Project);
    });
    harness.draw();
    // The same project again opens its page without changing checkout.
    let epoch = harness
        .window
        .update(&mut harness.cx, |plait, window, cx| {
            plait.show_settings(false, window, cx);
            assert!(!plait.settings_open);
            plait.host.connection.epoch
        })
        .expect("window update");
    harness.update(move |plait, window, cx| {
        plait.show_settings_for_project(other.clone(), window, cx);
        assert_eq!(plait.host.connection.epoch, epoch, "no checkout change");
        assert_eq!(plait.settings_page, SettingsPage::Project);
        plait.show_settings(false, window, cx);
        // The menu bar's Project Settings… opens the active project's page.
        window.dispatch_action(Box::new(super::ProjectSettings), cx);
    });
    harness.update(|plait, window, cx| {
        assert!(plait.settings_open);
        assert_eq!(plait.settings_page, SettingsPage::Project);
        window.dispatch_action(Box::new(super::Settings), cx);
    });
    harness.update(|plait, _, _| {
        assert!(
            plait.settings_open,
            "Cmd-comma opens Settings over the project page"
        );
        assert_eq!(plait.settings_page, SettingsPage::Application);
    });
    harness.draw();
    let action = super::actions::ACTIONS
        .iter()
        .find(|action| action.id == "project.settings")
        .expect("Project Settings… is in the action table");
    assert_eq!(action.title, "Project Settings…");
    assert_eq!(action.key, None, "no default shortcut");
    assert_eq!(action.category, "File");
    assert_ne!(action.place, super::actions::Place::Hidden);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

/// A Pi thread's run-settings menu lists only that thread's approval choices.
#[core::prelude::v1::test]
fn the_thread_menu_lists_only_the_threads_approvals() {
    if !isolated("the_thread_menu_lists_only_the_threads_approvals") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-run-menu-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        use plait_core::host::policy::CommandPolicy;
        // Trusted commands no longer add a project item to the thread's menu.
        plait.policy.key = Some((
            plait.host.connection.epoch,
            plait.host.creation.checkout.clone(),
        ));
        plait.policy.value = Some(CommandPolicy::Trusted);
        plait.open_run_settings(window, cx);
    });
    harness.draw();
    let titles = shown(&mut harness);
    assert_eq!(titles.len(), 3, "{titles:?}");
    assert!(titles[0].starts_with("Use default ("), "{titles:?}");
    assert_eq!(titles[1..], ["Ask before actions", "Auto-approve actions"]);
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn thread_management_preserves_newer_edits_and_activity() {
    if !isolated("thread_management_preserves_newer_edits_and_activity") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-management-ui-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        plait.sidebar = false;
        plait.begin_rename(THREAD.into(), window, cx);
        assert!(plait.sidebar, "global rename reveals its field");
        let field = plait.navigation.rename.as_ref().unwrap().1.clone();
        field.update(cx, |field, cx| field.set_text("Submitted title", cx));
        let operation = plait
            .begin_thread_management(
                THREAD.into(),
                &plait_core::host::Request::rename_thread(THREAD.into(), "Submitted title".into()),
                cx,
            )
            .unwrap();
        field.update(cx, |field, cx| field.set_text("A newer edit", cx));
        plait.navigation.menu = Some("thr_0c44a9d1".into());
        let focus = plait.navigation.filter.focus_handle(cx);
        window.focus(&focus, cx);

        let summary = plait
            .host
            .connection
            .threads
            .iter_mut()
            .find(|thread| thread.id == THREAD)
            .unwrap();
        summary.updated_at = Some(20);
        summary.title = Some("Newer live title".into());
        summary.status = ThreadStatus::Running;
        let reply = |updated_at| {
            Ok(plait_core::host::ThreadReply {
                thread: Snapshot {
                    id: THREAD.into(),
                    checkout: repository.clone(),
                    model: choice(),
                    title: Some("Submitted title".into()),
                    updated_at: Some(updated_at),
                    status: ThreadStatus::Idle,
                    ..Default::default()
                },
            })
        };
        plait.finish_thread_management(operation, reply(10), window, cx);
        assert!(plait.navigation.managing.is_none());
        assert_eq!(
            plait.navigation.rename.as_ref().unwrap().1.read(cx).text(),
            "A newer edit"
        );
        assert_eq!(plait.navigation.menu.as_deref(), Some("thr_0c44a9d1"));
        assert!(focus.is_focused(window));
        let summary = plait
            .host
            .connection
            .threads
            .iter()
            .find(|thread| thread.id == THREAD)
            .unwrap();
        assert_eq!(summary.title.as_deref(), Some("Newer live title"));
        assert_eq!(summary.status, ThreadStatus::Running);

        // Reopening the same thread makes a new editor even if its text matches.
        let operation = plait
            .begin_thread_management(
                THREAD.into(),
                &plait_core::host::Request::rename_thread(THREAD.into(), "Submitted title".into()),
                cx,
            )
            .unwrap();
        plait.begin_rename(THREAD.into(), window, cx);
        let replacement = plait.navigation.rename.as_ref().unwrap().1.clone();
        replacement.update(cx, |field, cx| field.set_text("A newer edit", cx));
        plait.finish_thread_management(operation, reply(30), window, cx);
        assert_eq!(plait.navigation.rename.as_ref().unwrap().1, replacement);
        assert!(replacement.focus_handle(cx).is_focused(window));

        // Archiving a thread does not acknowledge its unsaved rename field.
        let operation = plait
            .begin_thread_management(
                THREAD.into(),
                &plait_core::host::Request::archive_thread(THREAD.into(), true),
                cx,
            )
            .unwrap();
        plait.finish_thread_management(operation, reply(35), window, cx);
        assert_eq!(plait.navigation.rename.as_ref().unwrap().1, replacement);
        assert!(replacement.focus_handle(cx).is_focused(window));

        // The original editor can close after success without stealing focus.
        let operation = plait
            .begin_thread_management(
                THREAD.into(),
                &plait_core::host::Request::rename_thread(THREAD.into(), "Submitted title".into()),
                cx,
            )
            .unwrap();
        window.focus(&focus, cx);
        plait.finish_thread_management(operation, reply(40), window, cx);
        assert!(plait.navigation.rename.is_none());
        assert!(focus.is_focused(window));
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn reconnect_releases_management_and_rejects_its_late_reply() {
    if !isolated("reconnect_releases_management_and_rejects_its_late_reply") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-reconnect-ui-{}", std::process::id()));
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, window, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
        plait.begin_rename(THREAD.into(), window, cx);
        let operation = plait
            .begin_thread_management(
                THREAD.into(),
                &plait_core::host::Request::rename_thread(THREAD.into(), "Submitted title".into()),
                cx,
            )
            .unwrap();
        let field = plait.navigation.rename.as_ref().unwrap().1.clone();
        plait.navigation.loading = true;
        plait.reconnect_host(window, cx);
        assert!(plait.navigation.managing.is_none());
        assert!(!plait.navigation.loading);
        assert!(
            plait
                .navigation
                .error
                .as_deref()
                .unwrap()
                .contains("not confirmed")
        );
        // A different operation in the new epoch must remain pending.
        plait.host.connection.connected = true;
        let _new = plait
            .begin_thread_management(
                THREAD.into(),
                &plait_core::host::Request::rename_thread(THREAD.into(), "Submitted title".into()),
                cx,
            )
            .unwrap();
        plait.finish_thread_management(
            operation,
            Ok(plait_core::host::ThreadReply {
                thread: Snapshot {
                    id: THREAD.into(),
                    checkout: repository.clone(),
                    ..Default::default()
                },
            }),
            window,
            cx,
        );
        assert!(plait.navigation.managing.is_some());
        assert_eq!(plait.navigation.rename.as_ref().unwrap().1, field);
        assert!(field.focus_handle(cx).is_focused(window));
    });
    drop(harness);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[path = "frame_bench.rs"]
mod frame_bench;

/// The rail strip and the header's empty space arm a window move; controls in
/// the bar and the rail's resize handle keep their presses. The test platform
/// cannot move a window, so this stops before the drag.
#[core::prelude::v1::test]
fn the_window_bar_moves_the_window_only_from_empty_space() {
    if !isolated("the_window_bar_moves_the_window_only_from_empty_space") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-window-bar-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let repository = fixture_repository(&scratch);
    let mut harness = Harness::new(&repository, None, 1380., 860.);
    let rail = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            plait.panel_width(super::panels::Panel::Sidebar)
        })
        .unwrap();
    // Whether a press at `x` on the bar's center line arms a move. Each press
    // is released, which always disarms.
    let arms = |harness: &mut Harness, x: f32| {
        harness.draw();
        let position = point(px(x), px(20.));
        harness.pointer(PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }));
        let armed = harness
            .window
            .update(&mut harness.cx, |plait, _, _| plait.window_drag)
            .unwrap();
        harness.pointer(PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        }));
        let released = harness
            .window
            .update(&mut harness.cx, |plait, _, _| plait.window_drag)
            .unwrap();
        assert!(!released, "a release disarms the move");
        armed
    };
    let sidebar = |harness: &mut Harness| {
        harness
            .window
            .update(&mut harness.cx, |plait, _, _| plait.sidebar)
            .unwrap()
    };
    assert!(arms(&mut harness, 40.), "the rail's empty strip");
    assert!(
        arms(&mut harness, rail - 2.),
        "the empty bar before the first tab, above the rail resize handle"
    );
    assert!(arms(&mut harness, rail + 400.), "the header's empty middle");
    // A control in the bar keeps its click: this one hides the rail.
    assert!(!arms(&mut harness, 98.), "the sidebar toggle");
    assert!(!sidebar(&mut harness));
    // With the rail hidden the header starts under the lights, so its toggle
    // moves clear of them and the space it leaves still moves the window.
    assert!(arms(&mut harness, 22.), "the space under the lights");
    assert!(!arms(&mut harness, 98.), "the inset sidebar toggle");
    assert!(sidebar(&mut harness));
    // Full screen hides the lights, so the toggle returns to the edge.
    harness.update(|plait, window, _| {
        plait.sidebar = false;
        window.toggle_fullscreen();
    });
    assert!(!arms(&mut harness, 22.), "the toggle in full screen");
    assert!(sidebar(&mut harness));
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Heavy-scene frame cost. Run it with `bash scripts/bench-frames.sh`.
#[core::prelude::v1::test]
#[ignore = "timing run; use scripts/bench-frames.sh"]
fn frame_bench() {
    if !isolated("frame_bench") {
        return;
    }
    let profile = if cfg!(debug_assertions) {
        "debug (unoptimized)"
    } else {
        "release (optimized)"
    };
    let report = frame_bench::table(&frame_bench::run(), profile);
    println!("{report}");
    // The parent process discards the child's output, so the script reads a file.
    let Some(path) = std::env::var_os("PLAIT_BENCH_OUT") else {
        panic!("Set PLAIT_BENCH_OUT to a file for the table.");
    };
    std::fs::write(path, report).unwrap();
}

#[path = "snapshot/images.rs"]
mod images;

#[core::prelude::v1::test]
fn image_drafts_survive_reopen_and_refusal_keeps_the_draft() {
    if !isolated("image_drafts_survive_reopen_and_refusal_keeps_the_draft") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-draft-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    images::install(&mut harness, 1);
    let bytes = include_bytes!("../../../../runtime/test/support/images/0.png");
    let reference = plait_core::images::persist(bytes).unwrap();
    let fixture = images::fixtures();
    harness.composer(|composer, window, cx| {
        composer.type_text("Keep this draft", window, cx);
        composer.scene_images(
            vec![reference.clone()],
            std::collections::HashMap::from([(reference.id.clone(), fixture[0].1.clone())]),
            cx,
        );
        let revision = composer.saved_draft().1;
        composer.scene_refuse(vec![0; plait_core::images::IMAGE_BYTES + 1], cx);
        assert_eq!(composer.saved_draft().0, "Keep this draft");
        assert_eq!(composer.saved_images(), std::slice::from_ref(&reference));
        assert_eq!(composer.saved_draft().1, revision);
        composer.set_image_support(6, Some("no"), cx);
        composer.scene_submit(cx);
        assert!(composer.saved_draft().2.is_none());
        composer.set_image_support(5, Some("yes"), cx);
        composer.scene_submit(cx);
        assert!(composer.saved_draft().2.is_none());
    });
    harness.update(|plait, window, cx| {
        plait.open_draft(window, cx);
        let composer = plait.draft.composer.clone().unwrap();
        composer.update(cx, |composer, cx| {
            composer.type_text("New image draft", window, cx);
            composer.scene_images(vec![reference.clone()], Default::default(), cx);
        });
    });
    let (directory, host) = plait_core::session::locations().unwrap();
    let (journal, mut saved) =
        plait_core::session::Journal::open(&directory, host.clone()).unwrap();
    harness.update(|plait, _, cx| {
        plait.capture_session(cx);
        saved = plait.session.data.as_ref().unwrap().clone();
        saved.host_store = host.clone();
    });
    journal.queue(saved);
    journal.flush().unwrap();
    drop(journal);
    let (_, reopened) = plait_core::session::Journal::open(&directory, host).unwrap();
    let draft = &reopened.threads[THREAD].draft;
    assert_eq!(draft.text, "Keep this draft");
    assert_eq!(draft.images, std::slice::from_ref(&reference));
    assert_eq!(plait_core::images::local(&reference).unwrap(), bytes);
    let (checkout, new_draft) = reopened.new_draft.as_ref().unwrap();
    assert_eq!(checkout, &repository);
    assert_eq!(new_draft.text, "New image draft");
    assert_eq!(new_draft.images, std::slice::from_ref(&reference));
    let _ = std::fs::remove_dir_all(root);
}

#[core::prelude::v1::test]
fn image_replies_require_the_original_host_thread_and_generation() {
    if !isolated("image_replies_require_the_original_host_thread_and_generation") {
        return;
    }
    let expected = (4, 8, "first".to_owned());
    let mut cache = super::images::ImageCache::scene(expected.clone(), Default::default());
    assert!(cache.complete(
        &expected,
        &expected,
        "original".into(),
        Err("fixture".into())
    ));
    for actual in [
        (4, 8, "second".to_owned()),
        (4, 9, "first".to_owned()),
        (5, 8, "first".to_owned()),
    ] {
        assert!(!cache.complete(&expected, &actual, "late".into(), Err("fixture".into())));
        assert_eq!(cache.len(), 1);
    }
    for index in 0..150 {
        assert!(cache.complete(
            &expected,
            &expected,
            index.to_string(),
            Err("fixture".into())
        ));
    }
    assert_eq!(cache.len(), super::images::THUMBNAILS);
}

#[core::prelude::v1::test]
fn image_paste_and_drop_prepare_originals_and_keep_text_paste() {
    if !isolated("image_paste_and_drop_prepare_originals_and_keep_text_paste") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-paste-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    images::install(&mut harness, 1);
    let bytes = include_bytes!("../../../../runtime/test/support/images/0.png");
    let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, bytes.to_vec());
    harness.update(|plait, window, cx| {
        plait.focus_composer(window, cx);
        cx.write_to_clipboard(ClipboardItem::new_image(&image));
    });
    harness.keystroke("cmd-v");
    let wait = |harness: &mut Harness, count| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while harness.composer(|composer, _, _| composer.saved_images().len()) != count {
            assert!(
                Instant::now() < deadline,
                "image preparation did not finish"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait(&mut harness, 1);
    let original = harness.composer(|composer, _, _| composer.saved_images()[0].clone());
    assert_eq!(plait_core::images::local(&original).unwrap(), bytes);
    harness.update(|_, _, cx| {
        cx.write_to_clipboard(ClipboardItem {
            entries: vec![
                ClipboardEntry::Image(image),
                ClipboardEntry::String(ClipboardString::new("plain text".into())),
            ],
        })
    });
    harness.keystroke("cmd-v");
    harness.composer(|composer, _, _| {
        assert_eq!(composer.saved_draft().0, "plain text");
        assert_eq!(composer.saved_images().len(), 1);
    });
    let path = root.join("misleading.jpg");
    let second = include_bytes!("../../../../runtime/test/support/images/1.png");
    std::fs::write(&path, second).unwrap();
    harness.composer(|composer, _, cx| composer.scene_drop(vec![path], cx));
    wait(&mut harness, 2);
    let dropped = harness.composer(|composer, _, _| composer.saved_images()[1].clone());
    assert_eq!(dropped.media_type, "image/png");
    assert_eq!(plait_core::images::local(&dropped).unwrap(), second);
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn removing_an_attachment_collects_only_unreferenced_originals() {
    if !isolated("removing_an_attachment_collects_only_unreferenced_originals") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-remove-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    let (directory, host) = plait_core::session::locations().unwrap();
    let (journal, _) = plait_core::session::Journal::open(&directory, host.clone()).unwrap();
    harness.update(|plait, _, _| {
        plait.session.journal = Some(journal.clone());
        plait.session.data.as_mut().unwrap().host_store = host;
    });
    let reference = plait_core::images::persist(include_bytes!(
        "../../../../runtime/test/support/images/0.png"
    ))
    .unwrap();
    harness.composer(|composer, _, cx| {
        composer.scene_images(vec![reference.clone()], Default::default(), cx)
    });
    harness.update(|plait, _, cx| plait.capture_session(cx));
    journal.flush().unwrap();
    // An uncertain submission owns the original even after removing it from the draft.
    let mut pending = None;
    harness.update(|plait, _, cx| {
        let submission = plait
            .host
            .submissions
            .begin_images(
                THREAD,
                "look".into(),
                None,
                None,
                None,
                vec![reference.clone()],
            )
            .unwrap();
        plait.capture_session(cx);
        pending = Some(submission);
    });
    let pending = pending.unwrap();
    harness.composer(|composer, _, cx| composer.scene_remove(&reference.id, cx));
    harness.update(|plait, _, cx| plait.capture_session(cx));
    journal.flush().unwrap();
    assert!(plait_core::images::local(&reference).is_ok());
    harness.update(|plait, _, cx| {
        plait.host.submissions.finish(THREAD, &pending.request_id);
        plait.capture_session(cx);
    });
    journal.flush().unwrap();
    assert!(!directory.join("images").join(&reference.id).exists());
    // Removing an ordinary unsent attachment needs no host acknowledgement.
    plait_core::images::persist(include_bytes!(
        "../../../../runtime/test/support/images/0.png"
    ))
    .unwrap();
    harness.composer(|composer, _, cx| {
        composer.scene_images(vec![reference.clone()], Default::default(), cx)
    });
    harness.update(|plait, _, cx| plait.capture_session(cx));
    journal.flush().unwrap();
    harness.composer(|composer, _, cx| composer.scene_remove(&reference.id, cx));
    harness.update(|plait, _, cx| plait.capture_session(cx));
    journal.flush().unwrap();
    assert!(!directory.join("images").join(&reference.id).exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn host_acceptance_collects_the_original_after_clearing_the_draft() {
    if !isolated("host_acceptance_collects_the_original_after_clearing_the_draft") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-accepted-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    let (directory, host) = plait_core::session::locations().unwrap();
    let (journal, _) = plait_core::session::Journal::open(&directory, host.clone()).unwrap();
    harness.update(|plait, _, _| {
        plait.session.journal = Some(journal.clone());
        plait.session.data.as_mut().unwrap().host_store = host;
    });
    let reference = plait_core::images::persist(include_bytes!(
        "../../../../runtime/test/support/images/0.png"
    ))
    .unwrap();
    harness.composer(|composer, window, cx| {
        composer.type_text("look", window, cx);
        composer.scene_images(vec![reference.clone()], Default::default(), cx);
    });
    let revision = harness.composer(|composer, _, _| composer.saved_draft().1);
    let mut pending = None;
    harness.update(|plait, _, cx| {
        let submission = plait
            .host
            .submissions
            .begin_images(
                THREAD,
                "look".into(),
                None,
                None,
                Some(revision),
                vec![reference.clone()],
            )
            .unwrap();
        plait.capture_session(cx);
        pending = Some(submission);
    });
    let pending = pending.unwrap();
    journal.flush().unwrap();
    assert!(plait_core::images::local(&reference).is_ok());
    harness.update(|plait, _, cx| {
        plait.host.submissions.confirm(THREAD, &pending.request_id);
        plait
            .workspace
            .composers
            .get(THREAD)
            .unwrap()
            .update(cx, |composer, cx| {
                composer.acknowledge(&pending.text, pending.draft_revision, cx);
            });
        plait.capture_session(cx);
    });
    journal.flush().unwrap();
    harness.composer(|composer, _, _| {
        assert!(composer.saved_images().is_empty());
        assert!(composer.saved_draft().0.is_empty());
    });
    assert!(!directory.join("images").join(&reference.id).exists());
    // Editing an accepted queued message restores its now host-owned original.
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };
    let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = store.join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let bytes = include_bytes!("../../../../runtime/test/support/images/0.png");
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut data = String::new();
    for chunk in bytes.chunks(3) {
        let word = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            data.push(if index > chunk.len() {
                '='
            } else {
                alphabet[((word >> (18 - index * 6)) & 63) as usize] as char
            });
        }
    }
    let reply = serde_json::json!({"ok":true,"result":{"image":reference,"offset":0,"data":data}})
        .to_string()
        + "\n";
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(socket.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["op"], "image_read");
        assert_eq!(request["thread_id"], THREAD);
        socket.write_all(reply.as_bytes()).unwrap();
    });
    harness.composer(|composer, _, cx| {
        composer.restore_thread_images(vec![reference.clone()], THREAD.into(), cx)
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while plait_core::images::local(&reference).is_err() {
        assert!(Instant::now() < deadline, "accepted image was not restored");
        harness.settle();
        std::thread::sleep(Duration::from_millis(5));
    }
    server.join().unwrap();
    assert_eq!(plait_core::images::local(&reference).unwrap(), bytes);
    std::fs::remove_file(socket).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn image_lru_retains_hot_and_scrollback_pixels() {
    if !isolated("image_lru_retains_hot_and_scrollback_pixels") {
        return;
    }
    let key = (1, 1, THREAD.to_owned());
    let mut cache = super::images::ImageCache::scene(key.clone(), Default::default());
    let pixels = images::fixtures()[0].1.clone();
    let mut requests = 0;
    for index in (0..50).chain((0..50).rev()).chain(0..50) {
        let id = index.to_string();
        if !cache.contains(&id) {
            requests += 1;
            cache.complete(&key, &key, id.clone(), Ok(pixels.clone().into()));
        }
        cache.touch(&id);
    }
    assert_eq!(
        requests, 50,
        "scrolling back must not fetch or decode again"
    );
    for index in 50..super::images::THUMBNAILS {
        cache.complete(&key, &key, index.to_string(), Ok(pixels.clone().into()));
    }
    cache.touch("0");
    cache.complete(&key, &key, "new".into(), Ok(pixels.into()));
    assert!(cache.contains("0"));
    assert!(!cache.contains("1"));
    assert_eq!(cache.len(), super::images::THUMBNAILS);
}

#[core::prelude::v1::test]
fn image_disabled_refusal_and_prompt_history() {
    if !isolated("image_disabled_refusal_and_prompt_history") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-input-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    images::install(&mut harness, 1);
    let fixture = images::fixtures();
    let path = root.join("drop.png");
    let bytes = include_bytes!("../../../../runtime/test/support/images/0.png");
    std::fs::write(&path, bytes).unwrap();
    for reason in [
        "Creating thread",
        "Host disconnected",
        "Restoring saved work",
    ] {
        harness.composer(|composer, window, cx| {
            composer.set_enabled(false, Some(reason.into()), cx);
            let revision = composer.saved_draft().1;
            composer.scene_refuse(bytes.to_vec(), cx);
            composer.scene_drop(vec![path.clone()], cx);
            composer.type_text("ignored", window, cx);
            assert_eq!(composer.saved_draft().1, revision);
            assert!(composer.saved_images().is_empty());
            assert!(!composer.scene_image_status().0);
        });
    }
    harness.composer(|composer, window, cx| {
        composer.set_enabled(true, None, cx);
        composer.scene_refuse(vec![0; 16], cx);
        assert!(composer.scene_image_status().2.is_some());
        composer.type_text("Remember this illustrated prompt", window, cx);
        assert!(composer.scene_image_status().2.is_none());
        composer.scene_images(
            vec![fixture[0].0.clone()],
            std::collections::HashMap::from([(fixture[0].0.id.clone(), fixture[0].1.clone())]),
            cx,
        );
        composer.scene_submit(cx);
        assert!(
            composer
                .scene_history()
                .iter()
                .any(|text| text == "Remember this illustrated prompt")
        );
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn image_restore_failure_blocks_sending() {
    if !isolated("image_restore_failure_blocks_sending") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-failed-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    images::install(&mut harness, 1);
    let reference = images::fixtures()[0].0.clone();
    harness.composer(|composer, _, cx| composer.restore_images(vec![reference.clone()], cx));
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.composer(|composer, _, _| composer.scene_image_status().0) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.composer(|composer, _, cx| {
        let status = composer.scene_image_status();
        assert!(status.1);
        assert!(status.3.unwrap().contains("opened"));
        composer.scene_submit(cx);
        assert!(composer.saved_draft().2.is_none());
        assert_eq!(composer.saved_images(), &[reference]);
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn image_restore_failure_is_per_attachment() {
    if !isolated("image_restore_failure_is_per_attachment") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-partial-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    images::install(&mut harness, 1);
    let originals = [
        include_bytes!("../../../../runtime/test/support/images/0.png").as_slice(),
        include_bytes!("../../../../runtime/test/support/images/1.png").as_slice(),
        include_bytes!("../../../../runtime/test/support/images/2.png").as_slice(),
    ];
    let references = originals
        .iter()
        .map(|bytes| plait_core::images::inspect(bytes).unwrap())
        .collect::<Vec<_>>();
    let _pin = plait_core::images::pin(&references).unwrap();
    let directory = plait_core::session::locations().unwrap().0.join("images");
    // A missing middle image used to discard both earlier and later valid results.
    // Queued Edit uses the same loader with a host fallback; that can fail as well.
    for (thread, unreadable, broken) in [
        (false, false, 1),
        (false, true, 0),
        (true, false, 1),
        (true, true, 2),
    ] {
        for bytes in originals {
            plait_core::images::persist(bytes).unwrap();
        }
        let path = directory.join(&references[broken].id);
        std::fs::remove_file(&path).unwrap();
        if unreadable {
            // A link is deterministically unreadable even for a privileged test runner.
            std::os::unix::fs::symlink(directory.join(&references[(broken + 1) % 3].id), &path)
                .unwrap();
        }
        harness.composer(|composer, _, cx| {
            if thread {
                composer.restore_thread_images(references.clone(), THREAD.into(), cx);
            } else {
                composer.restore_images(references.clone(), cx);
            }
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while harness.composer(|composer, _, _| composer.scene_image_status().0) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        harness.composer(|composer, _, cx| {
            assert_eq!(composer.saved_images(), references);
            for (index, reference) in references.iter().enumerate() {
                let (pixels, error) = composer.scene_image_state(&reference.id);
                assert_eq!(pixels, index != broken);
                assert_eq!(error.is_some(), index == broken);
            }
            assert!(composer.scene_image_status().1);
            composer.scene_submit(cx);
            assert!(composer.saved_draft().2.is_none());
            composer.scene_remove(&references[broken].id, cx);
            assert_eq!(composer.saved_images().len(), 2);
            let status = composer.scene_image_status();
            assert!(!status.1, "removing the failed original must enable Send");
            assert!(status.3.is_none());
            assert_eq!(status.4, 2);
        });
        if unreadable {
            std::fs::remove_file(path).unwrap();
        }
    }
    harness.composer(|composer, _, cx| {
        composer.scene_submit(cx);
        assert!(composer.saved_draft().2.is_some());
        assert_eq!(composer.saved_images().len(), 2);
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn image_upload_state_blocks_duplicate_send() {
    if !isolated("image_upload_state_blocks_duplicate_send") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-upload-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    images::install(&mut harness, 1);
    let fixture = images::fixtures();
    harness.composer(|composer, _, cx| {
        composer.scene_images(
            vec![fixture[0].0.clone()],
            std::collections::HashMap::from([(fixture[0].0.id.clone(), fixture[0].1.clone())]),
            cx,
        )
    });
    let revision = harness.composer(|composer, _, _| composer.saved_draft().1);
    harness.update(|plait, _, cx| {
        let pending = plait
            .host
            .submissions
            .begin_images(
                THREAD,
                "".into(),
                None,
                None,
                Some(revision),
                vec![fixture[0].0.clone()],
            )
            .unwrap();
        plait
            .host
            .submissions
            .start_send(THREAD, &pending.request_id);
        plait.sync_composer(cx);
    });
    harness.composer(|composer, _, cx| {
        assert!(composer.scene_image_status().5);
        assert!(composer.scene_image_status().1);
        composer.scene_submit(cx);
        assert!(composer.saved_draft().2.is_none());
    });
    assert_eq!(
        super::thread_controls::pending_description("", 1),
        "1 image"
    );
    assert_eq!(
        super::thread_controls::pending_description("", 3),
        "3 images"
    );
    assert_eq!(
        super::thread_controls::pending_description("Look", 2),
        "Look · 2 images"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn image_drafts_stay_with_their_checkout() {
    if !isolated("image_drafts_stay_with_their_checkout") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-checkouts-{}", std::process::id()));
    let a = fixture_repository(&root.join("a"));
    let b = fixture_repository(&root.join("b"));
    let mut harness = Harness::new(&a, None, 900., 600.);
    harness.fresh_composer(&a);
    let (directory, host) = plait_core::session::locations().unwrap();
    let (journal, _) = plait_core::session::Journal::open(&directory, host.clone()).unwrap();
    harness.update(|plait, window, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 6;
        plait.session.journal = Some(journal.clone());
        plait.session.data.as_mut().unwrap().host_store = host;
        plait.open_draft(window, cx);
    });
    let fixture = images::fixtures();
    for (index, text) in ["Draft for A", "Draft for B"].into_iter().enumerate() {
        let bytes = [
            include_bytes!("../../../../runtime/test/support/images/0.png").as_slice(),
            include_bytes!("../../../../runtime/test/support/images/1.png").as_slice(),
        ][index];
        plait_core::images::persist(bytes).unwrap();
        harness.update(|plait, window, cx| {
            let composer = plait.draft.composer.clone().unwrap();
            composer.update(cx, |composer, cx| {
                assert!(composer.is_empty());
                composer.type_text(text, window, cx);
                composer.scene_images(
                    vec![fixture[index].0.clone()],
                    std::collections::HashMap::from([(
                        fixture[index].0.id.clone(),
                        fixture[index].1.clone(),
                    )]),
                    cx,
                );
            });
            plait.capture_session(cx);
            if index == 0 {
                plait.change_checkout(b.clone(), window, cx);
            } else {
                let id = plait
                    .tabs
                    .model
                    .tabs()
                    .iter()
                    .find(|tab| {
                        tab.session().target().is_draft()
                            && tab.session().target().checkout().root == a
                    })
                    .unwrap()
                    .id();
                plait.request_tab(super::tab_lifecycle::Intent::Activate(id), window, cx);
            }
        });
    }
    harness.update(|plait, _, cx| {
        let composer = plait.draft.composer.as_ref().unwrap().read(cx);
        assert_eq!(composer.saved_draft().0, "Draft for A");
        assert_eq!(composer.saved_images(), &[fixture[0].0.clone()]);
        plait.capture_session(cx);
    });
    journal.flush().unwrap();
    let saved: plait_core::session::Session =
        serde_json::from_slice(&std::fs::read(directory.join("desktop.json")).unwrap()).unwrap();
    assert_eq!(saved.new_draft_for(&a).unwrap().text, "Draft for A");
    assert_eq!(saved.new_draft_for(&b).unwrap().text, "Draft for B");
    assert_eq!(
        saved.new_draft_for(&b).unwrap().images,
        &[fixture[1].0.clone()]
    );
    // Discard must queue its own save, without another edit or close event.
    harness.update(|plait, window, cx| plait.discard_draft(window, cx));
    let deadline = Instant::now() + Duration::from_secs(5);
    while directory.join("images").join(&fixture[0].0.id).exists() {
        assert!(
            Instant::now() < deadline,
            "discard did not collect the original"
        );
        harness.settle();
        std::thread::sleep(Duration::from_millis(5));
    }
    let saved: plait_core::session::Session =
        serde_json::from_slice(&std::fs::read(directory.join("desktop.json")).unwrap()).unwrap();
    assert!(saved.new_draft_for(&a).is_none());
    assert!(saved.new_draft_for(&b).is_some());
    assert!(directory.join("images").join(&fixture[1].0.id).exists());
    std::fs::remove_dir_all(root).unwrap();
}

/// One controlled reply lets tests edit/remove while the production socket request is in flight.
fn delayed_image_reply(
    reply: serde_json::Value,
) -> (
    std::sync::mpsc::Receiver<serde_json::Value>,
    std::sync::mpsc::Sender<()>,
    std::thread::JoinHandle<()>,
) {
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
    };
    let store = PathBuf::from(std::env::var_os("PLAIT_HOST_STORE").unwrap());
    std::fs::create_dir_all(&store).unwrap();
    std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = store.join("host.sock");
    let listener = UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (arrived, received) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(socket.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        arrived.send(serde_json::from_str(&line).unwrap()).unwrap();
        wait.recv_timeout(Duration::from_secs(10)).unwrap();
        writeln!(socket, "{reply}").unwrap();
        std::fs::remove_file(path).unwrap();
    });
    (received, release, server)
}

#[core::prelude::v1::test]
fn image_queued_edit_preserves_attachments() {
    if !isolated("image_queued_edit_preserves_attachments") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-queued-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    images::install(&mut harness, 1);
    let reference = plait_core::images::persist(include_bytes!(
        "../../../../runtime/test/support/images/0.png"
    ))
    .unwrap();
    use plait_core::host::{QueueMode, Queued};
    let mut reply = serde_json::Value::Null;
    harness.update(|plait, _, cx| {
        let snapshot = Arc::make_mut(plait.host.thread.snapshot.as_mut().unwrap());
        reply = serde_json::json!({"ok":true,"result":{"thread":{
            "id":THREAD,"checkout":repository,"model":{"provider":"faux","model_id":"faux-1"},
            "status":"idle","messages":[],"tools":[],"live_text":"","pending_approval":null,"error":null,"truncated":false,"queued":[]
        }}});
        snapshot.queued.push(Queued {
            id: "request-queued-image".into(),
            request_id: "queued-image".into(),
            text: "Edit this image".into(),
            mode: QueueMode::FollowUp,
            approval: ApprovalMode::Ask,
        });
        plait.host.submissions.restore_accepted(
            THREAD.into(),
            plait_core::host_state::PendingSubmit {
            checkout_wait: false,
                text: "Edit this image".into(),
                images: vec![reference.clone()],
                request_id: "queued-image".into(),
                draft_revision: Some(1),
                approval: None,
                when_busy: Some(QueueMode::FollowUp),
            },
        );
        plait.sync_composer(cx);
    });
    let (received, release, server) = delayed_image_reply(reply);
    harness.update(|plait, window, cx| {
        plait.cancel_input("queued-image".into(), true, window, cx);
        plait.workspace.composers[THREAD].update(cx, |composer, cx| {
            let revision = composer.saved_draft().1;
            composer.type_text("An edit during cancellation", window, cx);
            composer.scene_refuse(
                include_bytes!("../../../../runtime/test/support/images/1.png").to_vec(),
                cx,
            );
            assert!(
                composer.is_empty(),
                "cancellation must reserve the empty draft"
            );
            assert_eq!(composer.saved_draft().1, revision);
        });
        // The deterministic executor runs the socket task after this UI turn.
        release.send(()).unwrap();
    });
    assert_eq!(
        received.recv_timeout(Duration::from_secs(5)).unwrap()["op"],
        "cancel_queued"
    );
    server.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.composer(|composer, _, _| {
        composer.saved_images().is_empty() || composer.scene_image_status().0
    }) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.composer(|composer, _, _| {
        assert_eq!(composer.saved_draft().0, "Edit this image");
        assert_eq!(composer.saved_images(), &[reference]);
        assert_eq!(composer.scene_image_status().4, 1);
        assert!(!composer.scene_image_status().1);
    });
    std::fs::remove_dir_all(root).unwrap();
}

/// A Wait message the person had accepted, with nothing else in the draft.
fn accepted_wait(
    harness: &mut Harness,
    request_id: &str,
    text: &str,
    images: Vec<plait_core::images::ImageRef>,
) -> PathBuf {
    let mut checkout = PathBuf::new();
    harness.update(|plait, _, cx| {
        plait.admission.queue = true;
        checkout = plait.host.connection.threads[0].checkout.clone();
        plait.host.submissions.restore_accepted(
            THREAD.into(),
            plait_core::host_state::PendingSubmit {
                checkout_wait: true,
                text: text.into(),
                images,
                request_id: request_id.into(),
                draft_revision: Some(1),
                approval: Some(ApprovalMode::Ask),
                when_busy: None,
            },
        );
        plait.sync_composer(cx);
    });
    checkout
}

fn wait_for_draft(harness: &mut Harness, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.composer(|composer, _, _| composer.saved_draft().0 != text) {
        assert!(
            Instant::now() < deadline,
            "the waiting message did not come back"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[core::prelude::v1::test]
fn cancelled_wait_returns_its_text_and_images_to_the_draft() {
    if !isolated("cancelled_wait_returns_its_text_and_images_to_the_draft") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-wait-cancel-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    images::install(&mut harness, 1);
    let reference = plait_core::images::persist(include_bytes!(
        "../../../../runtime/test/support/images/0.png"
    ))
    .unwrap();
    let checkout = accepted_wait(
        &mut harness,
        "wait-1",
        "Run this when the checkout frees",
        vec![reference.clone()],
    );
    harness.update(|plait, _, _| {
        plait.host.connection.threads[0].checkout_queue =
            Some(plait_core::host::checkout_queue::Summary::new(
                "wait-1".into(),
                plait_core::host::checkout_queue::State::Waiting,
                1,
                1,
            ));
    });
    let reply = serde_json::json!({"ok":true,"result":{"schema":1,"thread_id":THREAD,"checkout":checkout,
        "waiters":[{"request_id":"wait-1","state":"cancelled","position":0,"error":null}]}});
    let (received, release, server) = delayed_image_reply(reply);
    harness.update(|plait, window, cx| {
        let request = plait_core::host::Request::CancelCheckoutWait {
            thread_id: THREAD.into(),
            request_id: "wait-1".into(),
        };
        plait.change_checkout_wait(request, window, cx);
        release.send(()).unwrap();
    });
    assert_eq!(
        received.recv_timeout(Duration::from_secs(5)).unwrap()["op"],
        "cancel_checkout_wait"
    );
    server.join().unwrap();
    wait_for_draft(&mut harness, "Run this when the checkout frees");
    harness.composer(|composer, _, _| assert_eq!(composer.saved_images(), &[reference]));
    harness.update(|plait, _, _| {
        assert!(plait.host.submissions.accepted(THREAD).is_empty());
        let note = plait.workspace.thread_controls.note.clone().unwrap().1;
        assert!(note.contains("back in the draft"), "{note}");
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn wait_that_failed_while_waiting_returns_its_text_once() {
    if !isolated("wait_that_failed_while_waiting_returns_its_text_once") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-wait-failed-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    let checkout = accepted_wait(
        &mut harness,
        "wait-failed",
        "Refactor the parser",
        Vec::new(),
    );
    accepted_wait(
        &mut harness,
        "wait-started",
        "Already in the conversation",
        Vec::new(),
    );
    // The summary no longer lists a waiter: the host failed one and started the other.
    let reply = serde_json::json!({"ok":true,"result":{"schema":1,"thread_id":THREAD,"checkout":checkout,"waiters":[
        {"request_id":"wait-failed","state":"failed","position":0,"error":{"code":"queue_settings_changed","message":"The model changed while this message waited."}},
        {"request_id":"wait-started","state":"started","position":0,"error":null}]}});
    let (received, release, server) = delayed_image_reply(reply);
    harness.update(|plait, window, cx| {
        plait.reconcile_checkout_waits(window, cx);
        // One read per ended summary, however many updates arrive meanwhile.
        plait.reconcile_checkout_waits(window, cx);
        release.send(()).unwrap();
    });
    assert_eq!(
        received.recv_timeout(Duration::from_secs(5)).unwrap(),
        serde_json::json!({"op":"checkout_queue","thread_id":THREAD})
    );
    server.join().unwrap();
    wait_for_draft(&mut harness, "Refactor the parser");
    harness.update(|plait, window, cx| {
        assert!(plait.host.submissions.accepted(THREAD).is_empty());
        assert_eq!(plait.host.submissions.checkout_waits().count(), 0);
        let note = plait.workspace.thread_controls.note.clone().unwrap().1;
        assert!(
            note.starts_with("Not run. The model changed while this message waited."),
            "{note}"
        );
        // Nothing is left to read, so later summaries ask nothing.
        plait.reconcile_checkout_waits(window, cx);
        assert!(plait.admission.inspecting.is_empty());
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn image_removed_during_restore_drops_pixels() {
    if !isolated("image_removed_during_restore_drops_pixels") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-image-late-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.fresh_composer(&repository);
    images::install(&mut harness, 1);
    let reference = images::fixtures()[0].0.clone();
    let bytes = include_bytes!("../../../../runtime/test/support/images/0.png");
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut data = String::new();
    for chunk in bytes.chunks(3) {
        let word = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            data.push(if index > chunk.len() {
                '='
            } else {
                alphabet[((word >> (18 - index * 6)) & 63) as usize] as char
            });
        }
    }
    let (received, release, server) = delayed_image_reply(
        serde_json::json!({"ok":true,"result":{"image":reference,"offset":0,"data":data}}),
    );
    harness.composer(|composer, _, cx| {
        composer.restore_thread_images(vec![reference.clone()], THREAD.into(), cx);
        assert!(composer.scene_image_status().0);
        composer.scene_remove(&reference.id, cx);
        release.send(()).unwrap();
    });
    assert_eq!(
        received.recv_timeout(Duration::from_secs(5)).unwrap()["op"],
        "image_read"
    );
    server.join().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.composer(|composer, _, _| composer.scene_image_status().0) {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    harness.composer(|composer, _, _| {
        assert!(composer.saved_images().is_empty());
        assert_eq!(composer.scene_image_status().4, 0);
    });
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn thread_workspace_preserves_selection_and_reconnect_resets() {
    if !isolated("thread_workspace_preserves_selection_and_reconnect_resets") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-workspace-reset-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 1380., 860.);
    harness.connect(&repository, false);
    let second_id = "second-thread".to_owned();
    harness.update(|plait, window, cx| {
        plait.host.connection.connected = false;
        let mut second = plait.host.connection.threads[0].clone();
        second.id = second_id.clone();
        second.status = ThreadStatus::Idle;
        second.title = Some("Second".into());
        plait.host.connection.threads.push(second);
        plait.workspace.tab = super::Workspace::Files;
        plait.workspace.history.error = Some("old error".into());
        plait
            .workspace
            .expanded_live_tools
            .insert("tool-before-switch".into());
        plait.workspace.composers[THREAD].update(cx, |composer, cx| {
            composer.restore_draft("Keep this draft".into(), 7, None, cx);
        });
        plait.workspace.turn_review.target = Some(plait_core::review::recorded::Target {
            thread: THREAD.into(),
            checkout: plait.host.creation.checkout.clone(),
            record: "rec-1".into(),
            original: "rec-1".into(),
            path: None,
        });
        let review_gen = plait.workspace.turn_review.generation;
        plait.select_host_thread(second_id.clone(), window, cx);
        assert_eq!(plait.host.thread.selected.as_deref(), Some("second-thread"));
        assert_eq!(plait.workspace.tab, super::Workspace::Conversation);
        assert!(plait.workspace.history.error.is_none());
        assert!(plait.workspace.turn_review.target.is_none());
        assert!(plait.workspace.turn_review.generation > review_gen);
        assert!(plait.workspace.host_watch.is_none());
        assert!(plait.workspace.live_layout.is_empty());
        assert_eq!(plait.workspace.live_count, 0);
        assert_eq!(plait.workspace.live_scroll.item_count(), 0);
        assert!(plait.workspace.expanded_live_tools.is_empty());
        assert_eq!(
            plait.session.data.as_ref().unwrap().threads[THREAD]
                .draft
                .text,
            "Keep this draft"
        );
        assert!(plait.workspace.composers.contains_key("second-thread"));
        // Reconnect resets workspace connection-bound caches and advances review generation.
        plait.workspace.history.error = Some("stale".into());
        let before_reconnect = plait.workspace.turn_review.generation;
        plait.workspace.reset_for_reconnect();
        assert!(plait.workspace.history.error.is_none());
        assert_eq!(plait.workspace.turn_review.generation, before_reconnect + 1);
    });
    drop(harness);
    std::fs::remove_dir_all(root).unwrap();
}

fn ownership_scenes(
    repository: &Path,
    output: Option<&Path>,
    suffix: &str,
    width: f32,
    height: f32,
) {
    let mut harness = Harness::new(repository, output, width, height);
    harness.connect(repository, false);
    harness.update(|plait, window, cx| {
        let stale = plait
            .host
            .thread
            .snapshot
            .as_ref()
            .unwrap()
            .as_ref()
            .clone();
        let stamp = plait.host.read_stamp();
        plait.host.select(Some("second-thread".into()));
        let mut current = stale.clone();
        current.id = "second-thread".into();
        current.title = Some("Current thread".into());
        current.live_text.clear();
        current.messages[0].text =
            "Keep this thread in view while an earlier reply arrives.".into();
        assert!(plait.host.apply_snapshot(
            plait.host.connection.epoch,
            plait.host.thread.watch_epoch,
            current
        ));
        assert!(
            !plait
                .host
                .apply_snapshot(stamp.connection, stamp.selection, stale)
        );
        plait.conversation_changed(window, cx);
    });
    harness.capture(&format!("tabs-stale-reply-{suffix}"));
    harness.update(|plait, _, _| {
        let key = plait.state.checkout_key().unwrap().clone();
        plait.host.bind_checkout("second-thread", &key).unwrap();
        let mut replaced = key;
        replaced.inode += 1;
        plait.notice = plait.host.bind_checkout("second-thread", &replaced).err();
        assert_eq!(plait.notice.as_deref(), Some("Checkout identity changed."));
    });
    harness.capture(&format!("tabs-checkout-identity-error-{suffix}"));
}

#[core::prelude::v1::test]
fn commit_acknowledgements_never_clear_another_thread_or_newer_draft() {
    if !isolated("commit_acknowledgements_never_clear_another_thread_or_newer_draft") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-commit-owner-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    harness.update(|plait, _, cx| {
        let owner = plait.state.checkout_id();
        plait
            .workspace
            .commit_message
            .update(cx, |field, cx| field.set_text("Original", cx));
        let thread = plait.host.read_stamp().thread;
        let revision = plait.workspace.commit_message.read(cx).revision();
        plait.commit_sends.insert(
            owner,
            super::commit_panel::CommitAttempt {
                thread: thread.clone(),
                revision,
                message: "Original".into(),
            },
        );
        plait.host.select(Some("b".into()));
        plait.state.repository.write_mut().committed = Some("oid".into());
        plait.after_git_write(owner, cx);
        assert_eq!(plait.workspace.commit_message.read(cx).text(), "Original");
        plait.host.select(Some(THREAD.into()));
        plait.commit_sends.insert(
            owner,
            super::commit_panel::CommitAttempt {
                thread,
                revision,
                message: "Original".into(),
            },
        );
        plait.workspace.commit_message.update(cx, |field, cx| {
            field.set_text("Edited", cx);
            field.set_text("Original", cx);
        });
        plait.state.repository.write_mut().committed = Some("oid".into());
        plait.after_git_write(owner, cx);
        assert_eq!(plait.workspace.commit_message.read(cx).text(), "Original");
    });
    drop(harness);
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn file_reads_cannot_follow_the_same_name_into_another_checkout() {
    if !isolated("file_reads_cannot_follow_the_same_name_into_another_checkout") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-file-owner-{}", std::process::id()));
    let a = fixture_repository(&root.join("a"));
    let b = fixture_repository(&root.join("b"));
    std::fs::write(a.join("same.txt"), "From A\n").unwrap();
    std::fs::write(b.join("same.txt"), "From B\n").unwrap();
    std::fs::write(b.join("src/review.rs"), "pub fn only_from_b() {}\n").unwrap();
    let mut harness = Harness::new(&a, None, 900., 600.);
    harness.connect(&a, false);
    harness.update(|plait, window, cx| {
        plait.open_file("same.txt".into(), None, cx);
        plait.change_checkout(b.clone(), window, cx);
        assert!(plait.workspace.file_viewer.path.is_none());
        assert!(plait.workspace.file_viewer.opened.is_none());
    });
    harness.update(|plait, _, cx| {
        assert!(plait.workspace.file_viewer.opened.is_none());
        plait.open_file("same.txt".into(), None, cx);
    });
    harness.update(|plait, _, _| {
        let opened = plait.workspace.file_viewer.opened.as_ref().unwrap();
        assert_eq!(
            plait.workspace.file_viewer.path,
            Some(PathBuf::from("same.txt"))
        );
        assert_eq!(plait.workspace.file_viewer.root, b);
        assert!(format!("{:?}", opened.contents).contains("From B"));
    });
    harness.update(|plait, window, cx| {
        plait.open_file("src/review.rs".into(), None, cx);
        let tab_b = plait.tabs.model.active().unwrap();
        plait.change_checkout(a.clone(), window, cx);
        plait.request_tab(super::tab_lifecycle::Intent::Activate(tab_b), window, cx);
    });
    harness.update(|plait, _, _| {
        // Returning to B restores B's file, never A's retired read.
        assert_eq!(plait.workspace.file_viewer.root, b);
        assert_eq!(
            plait.workspace.file_viewer.path,
            Some("src/review.rs".into())
        );
        assert!(
            format!(
                "{:?}",
                plait
                    .workspace
                    .file_viewer
                    .opened
                    .as_ref()
                    .unwrap()
                    .contents
            )
            .contains("only_from_b")
        );
    });
    drop(harness);
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn file_read_lands_after_a_host_reconnect() {
    if !isolated("file_read_lands_after_a_host_reconnect") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-file-reconnect-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    // A local file read does not depend on the host connection it overlapped.
    harness.update(|plait, _, cx| {
        plait.open_file("src/review.rs".into(), None, cx);
        plait.host.reconnect();
        // The status reply is not modeled; only the new connection epoch matters.
        plait.host.connection.connecting = false;
        plait.host.connection.connected = true;
    });
    harness.update(|plait, _, _| {
        let viewer = &plait.workspace.file_viewer;
        assert!(!viewer.loading, "the read must not stay loading");
        assert!(viewer.opened.is_some(), "the file must be shown");
    });
    drop(harness);
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn sending_does_not_wait_for_a_git_status() {
    if !isolated("sending_does_not_wait_for_a_git_status") {
        return;
    }
    // A folder that is not a repository: its Git status read fails.
    let root = std::env::temp_dir().join(format!("plait-send-plain-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let checkout = root.canonicalize().unwrap();
    let mut harness = Harness::new(&checkout, None, 900., 600.);
    harness.connect(&checkout, false);
    // A queued follow-up can be sent while the sample thread runs.
    harness.update(|plait, _, _| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
    });
    harness.update(|plait, _, _| {
        assert!(plait.state.checkout_key().is_none());
        assert!(
            plait.can_submit(),
            "an unread checkout identity is not a mismatch"
        );
    });
    drop(harness);
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn sending_stops_for_a_replaced_checkout() {
    if !isolated("sending_stops_for_a_replaced_checkout") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-send-replaced-{}", std::process::id()));
    let repository = fixture_repository(&root);
    let mut harness = Harness::new(&repository, None, 900., 600.);
    harness.connect(&repository, false);
    // A queued follow-up can be sent while the sample thread runs.
    harness.update(|plait, _, _| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 3;
    });
    harness.update(|plait, _, _| {
        assert!(plait.can_submit());
        // The other sample thread was bound to a directory since replaced.
        let other = "thr_0c44a9d1";
        let mut replaced = plait.state.checkout_key().unwrap().clone();
        replaced.inode += 1;
        plait.host.bind_checkout(other, &replaced).unwrap();
        plait.host.thread.selected = Some(other.into());
        let mut snapshot = plait.host.thread.snapshot.as_deref().unwrap().clone();
        snapshot.id = other.into();
        plait.host.thread.snapshot = Some(Arc::new(snapshot));
        assert!(!plait.can_submit(), "a known mismatch still blocks sending");
    });
    drop(harness);
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn policy_reply_cannot_return_to_an_earlier_checkout_lifetime() {
    if !isolated("policy_reply_cannot_return_to_an_earlier_checkout_lifetime") {
        return;
    }
    let root = std::env::temp_dir().join(format!("plait-policy-owner-{}", std::process::id()));
    let a = fixture_repository(&root.join("a"));
    let b = fixture_repository(&root.join("b"));
    let mut harness = Harness::new(&a, None, 900., 600.);
    harness.connect(&a, false);
    let (release, released) = std::sync::mpsc::channel();
    let host = undo::Host::start(move |request| {
        assert_eq!(request["op"], "command_policy");
        released.recv_timeout(Duration::from_secs(5)).unwrap();
        Some(serde_json::json!({"checkout":request["checkout"], "policy":"trusted"}))
    });
    harness.update(|plait, window, cx| {
        plait
            .host
            .connection
            .status
            .as_mut()
            .unwrap()
            .protocol_version = 6;
        let key = (plait.host.connection.epoch, a.clone());
        plait.policy.key = Some(key.clone());
        plait.request_policy(None, cx);
        plait.change_checkout(b.clone(), window, cx);
        plait.change_checkout(a.clone(), window, cx);
        plait.policy.key = Some(key);
        plait.policy.value = Some(plait_core::host::policy::CommandPolicy::Sandboxed);
        release.send(()).unwrap();
    });
    undo::wait(&mut harness, |_| !host.requests.lock().unwrap().is_empty());
    drop(host);
    std::thread::sleep(Duration::from_millis(60));
    harness.settle();
    harness.update(|plait, _, _| {
        assert_eq!(
            plait.policy.value,
            Some(plait_core::host::policy::CommandPolicy::Sandboxed)
        );
        assert!(plait.policy.note.is_none());
    });
    drop(harness);
    std::fs::remove_dir_all(root).unwrap();
}

#[core::prelude::v1::test]
fn tab_shortcuts_drive_window_transitions() {
    if !isolated("tab_shortcuts_drive_window_transitions") {
        return;
    }
    tabs::exercise();
}

#[core::prelude::v1::test]
fn cold_tabs_restore_views_and_reject_retired_reads() {
    if !isolated("cold_tabs_restore_views_and_reject_retired_reads") {
        return;
    }
    tabs::cold_workspaces();
}

#[core::prelude::v1::test]
fn tabs_restore_good_damaged_and_future_state() {
    if !isolated("tabs_restore_good_damaged_and_future_state") {
        return;
    }
    tabs::restore_preferences();
}

#[core::prelude::v1::test]
fn tab_journal_limit_preserves_open_work() {
    if !isolated("tab_journal_limit_preserves_open_work") {
        return;
    }
    tabs::journal_limit();
}

#[core::prelude::v1::test]
fn views_suspend_reads_and_keep_unloaded_bookmarks() {
    if !isolated("views_suspend_reads_and_keep_unloaded_bookmarks") {
        return;
    }
    tabs::unloaded_views();
}

#[core::prelude::v1::test]
fn parked_tab_line_counts_stay_with_their_thread() {
    if !isolated("parked_tab_line_counts_stay_with_their_thread") {
        return;
    }
    tabs::parked_counts();
}

#[core::prelude::v1::test]
fn created_thread_shows_in_its_front_tab_from_any_view() {
    if !isolated("created_thread_shows_in_its_front_tab_from_any_view") {
        return;
    }
    tabs::created_in_front();
}

#[core::prelude::v1::test]
fn tabs_save_while_the_draft_journal_is_over_its_limit() {
    if !isolated("tabs_save_while_the_draft_journal_is_over_its_limit") {
        return;
    }
    tabs::journal_limit_saves_tabs();
}

#[core::prelude::v1::test]
fn unchanged_large_tab_views_are_stored_once() {
    if !isolated("unchanged_large_tab_views_are_stored_once") {
        return;
    }
    tabs::stored_views();
}

#[core::prelude::v1::test]
fn restored_tabs_stay_beneath_open_settings() {
    if !isolated("restored_tabs_stay_beneath_open_settings") {
        return;
    }
    tabs::restore_beneath_settings();
}
