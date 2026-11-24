//! Frame cost bench, a child of `snapshot` so it can use the headless harness.
//!
//! `bash scripts/bench-frames.sh` runs `snapshot::frame_bench`. A frame here is
//! `Window::draw` on GPUI's test platform: root `render`, layout, prepaint, and
//! paint into a scene, with the real Core Text shaping. No renderer is
//! attached, so GPU submission and presentation are NOT included. Numbers are
//! CPU frame cost from a headless process, not the installed app.

use super::*;
use crate::ui::Workspace;
use plait_client::{
    git::{Commit, History as CommitHistory},
    model::{Diff, DiffKind, DiffLine},
    preferences::Turns,
    state::DiffKey,
};

const WARMUP: usize = 5;

pub struct Row {
    pub name: String,
    pub samples: Vec<Duration>,
    /// Median of the root view's own `render()` call, from `last_render_us`.
    pub render_ms: f64,
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.
}

fn frames() -> usize {
    std::env::var("PLAIT_BENCH_FRAMES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30)
}

fn percentile(sorted: &[Duration], fraction: f64) -> Duration {
    let rank = (sorted.len() as f64 * fraction).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub fn table(rows: &[Row], profile: &str) -> String {
    let mut out = format!(
        "profile: {profile}; CPU frame cost via Window::draw (render+layout+prepaint+paint), no GPU submission\n\
         {:<76} {:>3} {:>8} {:>8} {:>8} {:>8} {:>11}\n",
        "scenario", "n", "min ms", "med ms", "p95 ms", "max ms", "render() ms"
    );
    for row in rows {
        let mut sorted = row.samples.clone();
        sorted.sort();
        out += &format!(
            "{:<76} {:>3} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>11.2}\n",
            row.name,
            sorted.len(),
            ms(sorted[0]),
            ms(percentile(&sorted, 0.5)),
            ms(percentile(&sorted, 0.95)),
            ms(*sorted.last().unwrap()),
            row.render_ms,
        );
    }
    out
}

fn row(name: &str, samples: Vec<Duration>, mut renders: Vec<u128>) -> Row {
    renders.sort_unstable();
    Row {
        name: name.into(),
        samples,
        render_ms: renders[renders.len() / 2] as f64 / 1000.,
    }
}

impl Harness {
    fn render_us(&mut self) -> u128 {
        self.window
            .update(&mut self.cx, |plait, _, _| plait.last_render_us)
            .expect("window update")
    }

    /// One timed frame, plus what the root `render()` call alone cost.
    fn frame(&mut self) -> (Duration, u128) {
        let start = Instant::now();
        self.draw();
        let elapsed = start.elapsed();
        (elapsed, self.render_us())
    }

    /// Wheel scroll over `at`, delivered like the platform would.
    fn wheel(&mut self, at: Point<Pixels>, dy: f32) {
        let event = PlatformInput::Scroll(ScrollEvent {
            position: at,
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            ..Default::default()
        });
        let window = self.window.into();
        self.cx
            .update_window(window, |_, window, cx| window.dispatch_event(event, cx))
            .expect("scroll");
    }

    /// Steady state: the same frame redrawn, as after a plain `notify`.
    fn steady(&mut self, name: &str) -> Row {
        for _ in 0..WARMUP {
            self.draw();
        }
        let (mut samples, mut renders) = (Vec::new(), Vec::new());
        for _ in 0..frames() {
            let (frame, render) = self.frame();
            samples.push(frame);
            renders.push(render);
        }
        row(name, samples, renders)
    }

    /// Frames 8 ms apart, as a 120 Hz display asks for them, while streamed
    /// text is still being revealed; the wait is not counted.
    fn revealing(&mut self, name: &str) -> Row {
        let (mut samples, mut renders) = (Vec::new(), Vec::new());
        for _ in 0..frames() {
            std::thread::sleep(Duration::from_millis(8));
            let (frame, render) = self.frame();
            samples.push(frame);
            renders.push(render);
        }
        row(name, samples, renders)
    }

    /// Each sample scrolls, then draws, so new rows are built every frame.
    fn scrolling(&mut self, name: &str, at: Point<Pixels>, dy: f32) -> Row {
        for _ in 0..WARMUP {
            self.wheel(at, dy);
            self.draw();
        }
        let (mut samples, mut renders) = (Vec::new(), Vec::new());
        for _ in 0..frames() {
            let start = Instant::now();
            self.wheel(at, dy);
            self.draw();
            samples.push(start.elapsed());
            renders.push(self.render_us());
        }
        row(name, samples, renders)
    }

    /// Action (with its queued work) to the end of the next frame.
    fn switching(
        &mut self,
        name: &str,
        rounds: usize,
        mut action: impl FnMut(&mut Harness, usize),
    ) -> Row {
        let (mut samples, mut renders) = (Vec::new(), Vec::new());
        for round in 0..rounds {
            let start = Instant::now();
            action(self, round);
            self.draw();
            samples.push(start.elapsed());
            renders.push(self.render_us());
        }
        row(name, samples, renders)
    }
}

/// A patch of `count` lines in the shape Git produces.
fn big_diff(count: usize) -> Arc<Diff> {
    let (mut old, mut new) = (0, 0);
    let lines: Vec<DiffLine> = (0..count)
        .map(|index| {
            let (kind, text) = match index % 40 {
                0 => (
                    DiffKind::Hunk,
                    format!("@@ -{old},38 +{new},40 @@ fn sample_{index}()"),
                ),
                r if r % 7 == 3 => (
                    DiffKind::Removed,
                    format!("-    let value_{index} = compute(&input[{index}..], {index} * 3);"),
                ),
                r if r % 5 == 2 => (
                    DiffKind::Added,
                    format!(
                        "+    let value_{index} = compute_fast(&input[{index}..], {index} * 3, true);"
                    ),
                ),
                _ => (
                    DiffKind::Context,
                    format!("     result.push(Entry::new({index}, \"sample text for line {index}\"));"),
                ),
            };
            let (o, n) = match kind {
                DiffKind::Hunk => (None, None),
                DiffKind::Removed => {
                    old += 1;
                    (Some(old), None)
                }
                DiffKind::Added => {
                    new += 1;
                    (None, Some(new))
                }
                _ => {
                    old += 1;
                    new += 1;
                    (Some(old), Some(new))
                }
            };
            DiffLine {
                text: text.into(),
                kind,
                old_line: o,
                new_line: n,
            }
        })
        .collect();
    // Drawn rows and the widest of them, as the parser reports them.
    let plait_client::patch::Rows { rows, widest } =
        plait_client::patch::rows(&lines, plait_client::patch::FileHeaders::Omit);
    let hunks = (0..lines.len())
        .filter(|&index| matches!(lines[index].kind, DiffKind::Hunk))
        .collect();
    Arc::new(Diff {
        lines,
        rows,
        widest_row: widest,
        hunks,
        truncated: false,
        elapsed: Duration::ZERO,
    })
}

fn commits(count: usize) -> CommitHistory {
    let commits = (0..count)
        .map(|index| Commit {
            id: format!("{index:040x}"),
            short: format!("{index:07x}"),
            author: format!("Author {}", index % 17),
            time: 1_700_000_000 - index as i64 * 3_600,
            subject: format!("Sample commit {index}: adjust the queue handling and notes"),
        })
        .collect();
    CommitHistory {
        commits,
        more: false,
    }
}

/// `messages` alternating turns; each reply carries prose and code and is
/// followed by three tool calls.
fn long_thread(id: &str, checkout: &Path, messages: usize) -> Snapshot {
    let mut snapshot = thread_snapshot(checkout, false);
    snapshot.id = id.into();
    snapshot.status = ThreadStatus::Idle;
    snapshot.live_text.clear();
    snapshot.pending_approval = None;
    snapshot.messages.clear();
    snapshot.tools.clear();
    for index in 0..messages {
        let user = index % 2 == 0;
        let message_id = format!("{id}-m{index}");
        snapshot.messages.push(Message {
            id: message_id.clone(),
            role: if user {
                MessageRole::User
            } else {
                MessageRole::Assistant
            },
            text: if user {
                format!("Question {index}: please adjust the queue and explain the change.")
            } else {
                format!("{PROSE}\n\n{CODE}")
            },
            ..Default::default()
        });
        if user {
            continue;
        }
        for step in 0..3 {
            let (name, input, output) = if step == 1 {
                (
                    "command",
                    "{\"command\":\"cargo test --workspace\"}",
                    "Sample test output: 24 tests passed.",
                )
            } else {
                (
                    "read",
                    "{\"path\":\"src/review.rs\"}",
                    "pub fn sample() {}\n",
                )
            };
            let mut made = tool(
                &format!("{id}-t{index}-{step}"),
                name,
                ToolStatus::Done,
                input,
                output,
            );
            made.after = Some(Some(message_id.clone()));
            snapshot.tools.push(made);
        }
    }
    snapshot
}

fn rewrite(repository: &Path, text: &str, big: &str) {
    for dir in 0..20 {
        for file in 0..100 {
            let path = repository.join(format!("gen/d{dir}/file_{file}.rs"));
            std::fs::write(path, text).unwrap();
        }
    }
    let lines: String = (0..15_000)
        .map(|i| format!("line {i} {big} text here\n"))
        .collect();
    std::fs::write(repository.join("big.txt"), lines).unwrap();
}

/// The fixture plus 2,000 changed files (a third staged) and one 15k-line file.
fn heavy_repository(root: &Path) -> PathBuf {
    let repository = fixture_repository(root);
    for dir in 0..20 {
        std::fs::create_dir_all(repository.join(format!("gen/d{dir}"))).unwrap();
    }
    rewrite(&repository, "pub fn original() {}\n", "original");
    git(&repository, &["add", "gen", "big.txt"]);
    git(&repository, &["commit", "-qm", "bulk fixture"]);
    rewrite(
        &repository,
        "pub fn changed() {\n    let value = 1;\n}\n",
        "edited",
    );
    for dir in 0..7 {
        git(&repository, &["add", &format!("gen/d{dir}")]);
    }
    repository
}

fn scroll_conversation(harness: &mut Harness, fraction: f32) {
    harness.update(|plait, _, _| {
        let item_ix = (plait.workspace.live_count as f32 * fraction) as usize;
        plait.workspace.live_scroll.scroll_to(ListOffset {
            item_ix,
            offset_in_item: px(0.),
        });
    });
    harness.draw();
}

fn conversation_cases(harness: &mut Harness, label: &str, rows: &mut Vec<Row>) {
    let count = harness
        .window
        .update(&mut harness.cx, |plait, _, _| plait.workspace.live_count)
        .unwrap();
    scroll_conversation(harness, 1.0);
    rows.push(harness.steady(&format!("c1 {label} conversation, {count} rows, at bottom")));
    scroll_conversation(harness, 0.5);
    rows.push(harness.steady(&format!("c2 {label} conversation, middle (steady)")));
    rows.push(harness.scrolling(
        &format!("c3 {label} conversation, wheel-scrolling in the middle"),
        point(px(700.), px(400.)),
        -60.,
    ));
    // Find in that conversation: every reply's code matches, so each row
    // drawn carries highlights. Typing waits for the background search.
    let set_query = |harness: &mut Harness, query: &'static str| {
        harness.update(move |plait, _, cx| {
            let field = plait.workspace.conversation_find.field.clone().unwrap();
            field.update(cx, |field, cx| field.set_text(query, cx));
        });
    };
    harness.update(|plait, window, cx| plait.open_conversation_find(window, cx));
    set_query(harness, "sample code");
    scroll_conversation(harness, 0.5);
    rows.push(harness.steady(&format!(
        "c4 {label} conversation, middle, find with matches highlighted"
    )));
    let queries = ["explain", "fixture", "sample code"];
    rows.push(harness.switching(
        &format!("c5 {label} find, a new query over every row, to its frame"),
        9,
        |h, round| set_query(h, queries[round % 3]),
    ));
    harness.update(|plait, window, cx| plait.close_conversation_find(window, cx));
}

/// Isolate the work-fold cases so tab and repository timings retain their fixtures.
fn finished_work_cases(repository: &Path, rows: &mut Vec<Row>) {
    let mut h = super::this_turn::harness(repository, None, 1380., 860.);
    h.connect(repository, false);
    let mut snapshot = long_thread(THREAD, repository, 1500);
    let mut records = Vec::with_capacity(750);
    let (turns, _) = snapshot.messages.as_chunks_mut::<2>();
    let (runs, _) = snapshot.tools.as_chunks_mut::<3>();
    for ([request, _], tools) in turns.iter_mut().zip(runs) {
        let mut record = super::undo::record(&request.id, &[&request.id], "ready");
        record["outcome"] = "finished".into();
        records.push(record);
        request.id = format!("request-{}", request.id);
        for tool in tools {
            tool.after = Some(Some(request.id.clone()));
        }
    }
    records.reverse();
    h.change_snapshot(move |s| *s = snapshot);
    h.update(|p, _, _| {
        p.host.connection.status.as_mut().unwrap().protocol_version = 7;
        p.preferences.value.fold_earlier_turns = Some(false);
        p.preferences.value.fold_finished_work = Some(true);
        super::undo::inject(p, records);
        p.workspace.turn_records.totals =
            super::super::turn_records::TurnRecords::file_totals(&p.workspace.turn_records.data);
        p.reconcile_live_folding();
        assert_eq!(p.workspace.live_count, 3750);
        assert_eq!(p.workspace.live_layout.work.len(), 750);
    });
    conversation_cases(&mut h, "folded work", rows);
}

fn show(harness: &mut Harness, tab: Workspace) {
    harness.update(move |plait, window, cx| plait.show_tab(tab, window, cx));
}

fn select(harness: &mut Harness, index: usize) {
    harness.update(move |plait, window, cx| {
        plait.select_file(index, DiffMode::Unstaged, window, cx);
    });
}

/// Mirrors `select_host_thread`; the app builds `formatted` on the background
/// executor, so this includes work the real app does off the UI thread.
fn install_thread(harness: &mut Harness, id: &'static str, snapshot: Snapshot) {
    harness.update(move |plait, window, cx| {
        plait.host.thread.selected = Some(id.into());
        plait.workspace.history = Default::default();
        plait.host.thread.snapshot = Some(Arc::new(snapshot));
        plait.workspace.formatted = Arc::new(plait_client::markdown::Transcript::update(
            &Default::default(),
            plait.host.thread.snapshot.as_ref().unwrap(),
        ));
        plait.refresh_live_list(None);
        if !plait.workspace.composers.contains_key(id) {
            let composer = cx.new(|cx| Composer::new(window, cx));
            plait.workspace.composers.insert(id.into(), composer);
        }
        plait.sync_composer(cx);
    });
}

fn baseline(scratch: &Path, rows: &mut Vec<Row>) {
    let small = fixture_repository(&scratch.join("small"));
    for (style, label) in [(Turns::Bubbles, "bubbles"), (Turns::Line, "line")] {
        let mut harness = Harness::new(&small, None, 1380., 860.);
        harness.update(|plait, _, _| plait.preferences.value.turns = style);
        rows.push(harness.steady(&format!(
            "e1 {label} idle redraw, default view (disconnected home)"
        )));
        harness.connect(&small, false);
        rows.push(harness.steady(&format!(
            "e2 {label} idle redraw, small live conversation (3 msgs, 2 tools)"
        )));
        // A burst has landed and the reveal is drawing it in, one display frame at a time.
        harness.change_snapshot(|snapshot| {
            snapshot
                .live_text
                // Paragraphs of about 400 bytes, as replies are usually written.
                .push_str(
                    &"\n\nThe streamed reply keeps arriving in bursts. "
                        .repeat(8)
                        .repeat(25),
                )
        });
        rows.push(harness.revealing(&format!(
            "e3 {label} streaming reveal frame, 120 Hz pacing (small conversation)"
        )));
    }
}

pub fn run() -> Vec<Row> {
    let scratch = std::env::temp_dir().join(format!("plait-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    let mut rows = Vec::new();
    baseline(&scratch, &mut rows);
    {
        let repo = fixture_repository(&scratch.join("worktrees"));
        let mut h = Harness::new(&repo, None, 1380., 860.);
        h.connect(&repo, false);
        super::worktrees_view::install(&mut h, 51);
        rows.push(h.steady("worktrees: fifty lanes, cached graph and sizes, visible rows only"));
    }

    let heavy = heavy_repository(&scratch.join("heavy"));
    let mut harness = Harness::new(&heavy, None, 1380., 860.);
    harness.connect(&heavy, false);
    let changed = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            plait
                .state
                .repository
                .snapshot()
                .ready()
                .map_or(0, |s| s.changes.len())
        })
        .unwrap();
    assert!(
        changed >= 2000,
        "expected 2000+ changed files, got {changed}"
    );

    super::images::install(&mut harness, 50);
    rows.push(harness.steady("images: 50 attached images, prepared thumbnails, bottom"));
    rows.push(harness.scrolling(
        "images: 50 attached images, scrolling",
        point(px(700.), px(300.)),
        -12.,
    ));

    // c. Long conversation, the open thread while the other tabs are measured.
    let long = long_thread(THREAD, &heavy, 1500);
    harness.change_snapshot(move |snapshot| *snapshot = long);
    super::tabs::install(&mut harness, 20);
    rows.push(harness.steady("tabs20 conversation, twenty tabs, five running summaries"));
    rows.push(harness.switching(
        "tabs20 five running, burst of twenty summaries",
        frames(),
        |h, round| {
            h.update(|plait, _, cx| {
                let summaries = plait
                    .host
                    .connection
                    .threads()
                    .iter()
                    .enumerate()
                    .map(|(index, thread)| {
                        let mut thread = thread.clone();
                        thread.updated_at =
                            Some(thread.updated_at.unwrap_or_default().saturating_add(1));
                        thread.pending_approval = Some(false);
                        thread.status = if (index + round) % 20 < 5 {
                            ThreadStatus::Running
                        } else {
                            ThreadStatus::Idle
                        };
                        thread
                    })
                    .collect();
                plait_client::navigation::merge_threads(
                    plait.host.connection.threads_mut(),
                    summaries,
                )
                .unwrap();
                plait.refresh_tab_rows();
                plait.rebuild_projects(cx);
            });
        },
    ));
    super::tabs::install(&mut harness, 300);
    rows.push(harness.steady("tabs300 conversation, bounded visible strip, 299 cold tabs"));
    harness.update(|p, _, _| p.tabs.list_open = true);
    rows.push(harness.steady("tabs300 virtual overflow, visible rows only"));
    harness.update(|p, _, _| p.tabs.list_open = false);
    super::tabs::install(&mut harness, 1);
    for (style, label) in [(Turns::Line, "line"), (Turns::Bubbles, "bubbles")] {
        harness.update(|plait, _, _| {
            plait.preferences.value.turns = style;
            plait.workspace.live_scroll.remeasure();
        });
        conversation_cases(&mut harness, label, &mut rows);
    }
    scroll_conversation(&mut harness, 1.0);

    finished_work_cases(&heavy, &mut rows);

    // a. Changes: 2,000 files, an injected 20k-line diff.
    harness.update(|plait, _, cx| {
        plait.workspace.tab = Workspace::Changes;
        plait.state.repository.view.changes.diff = Load::Ready(big_diff(20_000));
        plait.state.repository.view.changes.selected = Some(DiffKey {
            path: plait.state.repository.snapshot().ready().unwrap().changes[0]
                .path
                .clone(),
            file: 0,
            mode: DiffMode::Unstaged,
        });
        plait.workspace.diff_scroll = UniformListScrollHandle::new();
        cx.notify();
    });
    rows.push(harness.steady("a1 changes, 2000 files + 20k-line diff, top"));
    harness.update(|plait, _, _| {
        plait
            .workspace
            .diff_scroll
            .scroll_to_item(10_000, ScrollStrategy::Top);
        plait
            .workspace
            .file_scroll
            .scroll_to_item(1_000, ScrollStrategy::Top);
    });
    rows.push(harness.steady("a2 changes, 2000 files + 20k-line diff, middle"));
    rows.push(harness.scrolling(
        "a3 changes, wheel-scrolling the diff",
        point(px(900.), px(400.)),
        -60.,
    ));
    rows.push(harness.scrolling(
        "a4 changes, wheel-scrolling the file list",
        point(px(120.), px(400.)),
        -60.,
    ));

    turn_review::scene(&mut harness, &heavy, "ready", Some(big_diff(5_000)));
    rows.push(harness.steady("recorded 5k-line diff, top"));
    harness.update(|p, _, _| {
        p.workspace
            .diff_scroll
            .scroll_to_item(2_500, ScrollStrategy::Top)
    });
    rows.push(harness.steady("recorded 5k-line diff, middle"));
    rows.push(harness.scrolling(
        "recorded 5k-line diff, scrolling",
        point(px(900.), px(400.)),
        -60.,
    ));
    harness.update(|p, _, _| p.clear_turn_review());

    // b. History: 5,000 commits, an injected 20k-line patch.
    harness.update(|plait, _, cx| {
        plait.workspace.tab = Workspace::History;
        plait.state.repository.data.commits.list = Load::Ready(commits(5_000));
        plait.state.repository.view.history.selected = Some(format!("{:040x}", 0));
        plait.state.repository.view.history.patch = Load::Ready(big_diff(20_000));
        plait.workspace.commit_scroll = UniformListScrollHandle::new();
        plait.workspace.history_scroll = UniformListScrollHandle::new();
        cx.notify();
    });
    rows.push(harness.steady("b1 history, 5000 commits + 20k-line patch, top"));
    harness.update(|plait, _, _| {
        plait
            .workspace
            .history_scroll
            .scroll_to_item(2_500, ScrollStrategy::Top);
        plait
            .workspace
            .commit_scroll
            .scroll_to_item(10_000, ScrollStrategy::Top);
    });
    rows.push(harness.steady("b2 history, middle of list and patch"));
    rows.push(harness.scrolling(
        "b3 history, wheel-scrolling the commit list",
        point(px(150.), px(400.)),
        -60.,
    ));
    rows.push(harness.scrolling(
        "b4 history, wheel-scrolling the patch",
        point(px(900.), px(400.)),
        -60.,
    ));

    // f. Files: a real 20,000-line Rust file, read and highlighted in the background.
    let source = (0..20_000)
        .map(|n| format!("    let value_{n} = compute(\"item {n}\", {n}); // step {n}\n"))
        .collect::<String>();
    std::fs::write(heavy.join("big.rs"), source).unwrap();
    harness.update(|plait, _, cx| plait.open_file("big.rs".into(), Some(10_000), cx));
    rows.push(harness.steady("f1 files, 20k-line highlighted file, middle"));
    rows.push(harness.scrolling(
        "f2 files, wheel-scrolling a 20k-line file",
        point(px(900.), px(400.)),
        -60.,
    ));

    // d. Switching, all against the heavy state above.
    show(&mut harness, Workspace::Conversation);
    let tabs = [
        Workspace::Changes,
        Workspace::History,
        Workspace::Conversation,
    ];
    rows.push(harness.switching(
        "d1 tab switch Conversation>Changes>History>Conversation (heavy)",
        12,
        |h, round| show(h, tabs[round % 3]),
    ));
    show(&mut harness, Workspace::Changes);
    let big_index = harness
        .window
        .update(&mut harness.cx, |plait, _, _| {
            let snapshot = plait.state.repository.snapshot().ready().unwrap();
            snapshot
                .changes
                .iter()
                .position(|change| change.path.ends_with("big.txt"))
                .unwrap()
        })
        .unwrap();
    let files = [big_index, 5, 700, 1500, 9, big_index + 1];
    rows.push(harness.switching(
        "d2 select file, diff NOT cached (real git read, waits for it)",
        files.len(),
        |h, round| select(h, files[round]),
    ));
    rows.push(harness.switching(
        "d3 select file, diff cached (incl. real 10k-line capped diff)",
        18,
        |h, round| select(h, files[round % files.len()]),
    ));
    // Both files were read in d2 and stay in the recent-diff cache. A step reads
    // the following file ahead in the background; the harness waits for that
    // read when it settles the next action, so one sample includes it.
    rows.push(
        harness.switching("d4 step_file to next, cached", 20, |h, _| {
            select(h, big_index);
            h.update(|plait, window, cx| plait.step_file(true, window, cx));
        }),
    );

    // Thread switch between the long thread and a small one.
    show(&mut harness, Workspace::Conversation);
    let small = long_thread("thr_0c44a9d1", &heavy, 40);
    let long = long_thread(THREAD, &heavy, 1500);
    let mut install = |h: &mut Harness, round: usize| {
        if round.is_multiple_of(2) {
            install_thread(h, "thr_0c44a9d1", small.clone());
        } else {
            install_thread(h, THREAD, long.clone());
        }
    };
    rows.push(harness.switching(
        "d5 thread switch 1500-msg <-> 40-msg (incl. markdown build on UI thread)",
        8,
        &mut install,
    ));
    let (mut firsts, mut renders) = (Vec::new(), Vec::new());
    for round in 0..8 {
        install(&mut harness, round);
        let (frame, render) = harness.frame();
        firsts.push(frame);
        renders.push(render);
    }
    rows.push(row(
        "d6 thread switch, first frame after install only",
        firsts,
        renders,
    ));

    rows.push(harness.switching(
        "d7 rapid A>B>A selection, reject delayed reads (no host I/O)",
        30,
        |h, round| {
            h.update(|plait, _, _| {
                let stale = plait.host.read_stamp();
                plait.host.select(Some(
                    if round.is_multiple_of(2) {
                        THREAD
                    } else {
                        "thr_0c44a9d1"
                    }
                    .into(),
                ));
                plait.state.repository.invalidate_view();
                assert!(!plait.host.accepts_read(&stale));
            });
            install(h, round);
        },
    ));
    // The heavy workspace stays mounted while Settings is open.
    scroll_conversation(&mut harness, 0.5);
    rows.push(harness.steady("modal baseline, heavy conversation"));
    harness.update(|plait, window, cx| plait.show_settings(true, window, cx));
    rows.push(harness.steady("modal open, heavy conversation behind Settings"));
    harness.update(|plait, window, cx| plait.show_settings(false, window, cx));
    rows.push(harness.steady("modal closed, same heavy conversation"));

    drop(harness);
    let _ = std::fs::remove_dir_all(&scratch);
    rows
}
