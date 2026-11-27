//! Branch graph fixtures are confined to scenes and tests.
use super::super::worktrees_view::Again;
use super::*;
use plait_client::{
    git::{
        log::{Commit, GraphCommit},
        worktree::inventory::Preservation,
    },
    host::worktree::Checkout,
    tabs::TabTarget,
    worktree_graph,
    worktree_size::Size,
};
fn oid(n: usize) -> String {
    (0..5)
        .map(|part| {
            format!(
                "{:08x}",
                (n as u32).wrapping_mul(0x9e3779b9).rotate_left(part * 5) ^ 0xa3c59ac3
            )
        })
        .collect()
}
fn commit(n: usize, parent: usize, subject: String) -> GraphCommit {
    GraphCommit {
        commit: Commit {
            id: oid(n),
            short: oid(n)[..7].into(),
            author: "Sample author".into(),
            time: super::super::commits::now() - n as i64 * 60,
            subject,
        },
        parents: if parent == 0 {
            vec![]
        } else {
            vec![oid(parent)]
        },
    }
}
pub(super) fn install(h: &mut Harness, count: usize) {
    h.update(|p,_,cx|{
        p.park_workspace(cx);
        p.notice=None;
        p.settings_open=false;
        let root=p.host.creation.checkout.clone();
        let view = p.tabs.model.open(TabTarget::Worktrees {checkout:p.tab_checkout(root.clone())},Some(p.host.connection.epoch)).unwrap();
        p.tabs.model.open(TabTarget::Draft { id:"scene-draft".into(), checkout:p.tab_checkout(root.clone()) },Some(p.host.connection.epoch)).unwrap();
        p.tabs.model.activate(view).unwrap();
        p.host.connection.threads_mut().retain(|t| !t.id.starts_with("worktree-scene-"));
        let mut rows=Vec::new();let mut commits=Vec::new();
        for i in 0..count {
            let path=if i==0 {root.clone()}else{root.with_extension("worktrees").join(format!("sample-{i}"))};
            let title=match i {0=>"Main checkout".into(),1=>"Retry requests after rate limits".into(),2=>"Read the local project settings".into(),3=>"Keep attachments across restarts".into(),_=>format!("Worktree {i}")};
            let id=format!("worktree-scene-{i}");
            if i > 0 { p.host.connection.threads_mut().push(ThreadSummary{id:id.clone(),checkout:path.clone(),title:Some(title),status:if i==1{ThreadStatus::Running}else if i==2{ThreadStatus::AwaitingApproval}else{ThreadStatus::Idle},..Default::default()}); }
            rows.push(Checkout { build_directories: Vec::new(), build_output: None,path,main:i==0,branch:Some(if i==0{"main".into()}else{format!("plait/topic-{i}")}),head:Some(oid(if i==0{10}else{100+i})),base:(i>0).then(||oid(9-i.min(6))),thread_id:(i>0).then_some(id),identity:Some(plait_client::git::worktree::Identity{dev:"1".into(),ino:(i+1).to_string()}),missing:false,removed:false,preservation:Preservation { build_output: None,changed:Some(if i==2{3}else{0}),unpushed:Some(if i==1{2}else{0}),exclusive:Some(0),can_keep_local:i>0 && i!=2,refusal:if i==1{Some("This worktree has unpushed commits. Push them or explicitly confirm preserving only the local branch.".into())}else if i==2{Some("This worktree has uncommitted or ignored files. Preserve or remove them before removing the worktree.".into())}else if i==0{Some("The main checkout cannot be removed here.".into())}else{None}},creation_id:(i>0).then(||format!("creation-{i}")),last_used:Some(super::super::commits::now() as u64*1000-i as u64*300000)});
            if i>0 {commits.push(commit(100+i,9-i.min(6),(if i==1{"Retry transient failures"}else if i==2{"Validate project overrides"}else{"Preserve draft attachments"}).to_string()));}
        }
        for n in (1..=10).rev(){commits.push(commit(n,n-1,match n {10=>"Keep thread tabs independent".into(),9=>"Read host-owned worktrees".into(),8=>"Preserve the checkout identity".into(),_=>format!("Project change {n}")}));}
        p.worktrees.graph=worktree_graph::layout(&rows,commits,false);
        p.worktrees.sizes=(0..count).map(|i|Some(Ok(Size{source:((i+1)*12_000_000) as u64,build:if i==1{3_800_000_000}else if i==3{1_400_000_000}else{0},incomplete:false,entries:300}))).collect();
        p.worktrees.scroll=UniformListScrollHandle::new();
        p.worktrees.rows=rows;p.worktrees.loading=false;p.worktrees.error=None;p.worktrees.selected=1.min(count.saturating_sub(1));p.worktrees.prepare();p.worktree_labels();p.refresh_tab_rows();cx.notify();
    });
}
pub(super) fn walk(repo: &Path, out: Option<&Path>, suffix: &str, width: f32, height: f32) {
    let mut h = Harness::new(repo, out, width, height);
    h.connect(repo, false);
    for light in [false, true] {
        h.update(|p, _, cx| {
            p.themes.forced = Some(light);
            p.apply_theme(cx);
        });
        let theme = if light { "light" } else { "dark" };
        install(&mut h, 4);
        h.capture(&format!("worktrees-graph-{theme}-{suffix}"));
        // A remote host: its own graph and sizes; Finder and Terminal are off.
        h.update(|p, _, cx| {
            p.host_local = false;
            cx.notify();
        });
        h.capture(&format!("worktrees-remote-{theme}-{suffix}"));
        h.update(|p, _, _| p.host_local = true);
        install(&mut h, 4);
        h.update(|p, _, _| p.worktrees.selected = 2);
        h.pointer(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(
                px(if width > 1000. { 1290. } else { 480. }),
                px(if width > 1000. { 143. } else { 416. }),
            ),
            pressed_button: None,
            modifiers: Modifiers::default(),
        }));
        h.capture(&format!("worktrees-refusal-hover-{theme}-{suffix}"));
        h.pointer(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(10.), px(10.)),
            pressed_button: None,
            modifiers: Modifiers::default(),
        }));
        h.update(|p, _, _| p.worktrees.selected = 1);
        h.pointer(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(600.), px(182.)),
            pressed_button: None,
            modifiers: Modifiers::default(),
        }));
        h.capture(&format!("worktrees-lane-hover-{theme}-{suffix}"));
        h.pointer(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(10.), px(10.)),
            pressed_button: None,
            modifiers: Modifiers::default(),
        }));
        h.update(|p, _, _| {
            p.worktrees.sizes.iter_mut().for_each(|s| *s = None);
            p.worktrees.account();
        });
        h.capture(&format!("worktrees-measuring-{theme}-{suffix}"));
        install(&mut h, 21);
        h.capture(&format!("worktrees-twenty-{theme}-{suffix}"));
        h.update(|p, _, _| {
            p.worktrees
                .scroll
                .scroll_to_item(p.worktrees.items.len() - 1, ScrollStrategy::Bottom)
        });
        h.capture(&format!("worktrees-twenty-forks-{theme}-{suffix}"));
        install(&mut h, 4);
        h.update(|p, _, _| {
            p.worktrees.rows[3].head = Some(oid(8));
            p.worktrees.rows[1].base = Some(oid(999));
            let commits = p
                .worktrees
                .graph
                .rows
                .iter()
                .map(|r| r.commit.clone())
                .collect();
            p.worktrees.graph = worktree_graph::layout(&p.worktrees.rows, commits, false);
            p.worktrees.prepare();
            p.worktree_labels();
        });
        h.capture(&format!("worktrees-merged-detached-{theme}-{suffix}"));
        install(&mut h, 4);
        h.update(|p,_,_| {
            p.worktrees.rows[1].missing=true;
            p.worktrees.rows[1].preservation.can_keep_local=false;
            p.worktrees.rows[1].preservation.refusal=Some("The worktree folder is missing. Nothing was removed. If you moved it, move it back; if its disk is not connected, connect it.".into());
            p.worktrees.sizes[1]=Some(Err("The checkout folder is missing.".into()));
            p.worktrees.rows[2].missing=true;p.worktrees.rows[2].removed=true;
            p.worktrees.rows[2].preservation.refusal=Some("This worktree was removed. Its branch, thread history and private build cache remain.".into());
            p.worktrees.rows[3].creation_id=None;p.worktrees.rows[3].thread_id=None;
            p.worktrees.rows[3].preservation.can_keep_local=false;
            p.worktrees.rows[3].preservation.refusal=Some("Plait did not create this worktree. Remove it with your Git tools.".into());
            p.worktrees.prepare();p.worktree_labels();
        });
        h.capture(&format!("worktrees-missing-foreign-{theme}-{suffix}"));
        install(&mut h, 1);
        h.capture(&format!("worktrees-empty-{theme}-{suffix}"));
        h.update(|p, _, _| {
            p.worktrees.park();
            p.worktrees.error = Some(plait_client::host::worktree::OLDER_HOST.into());
        });
        h.capture(&format!("worktrees-old-host-{theme}-{suffix}"));
        h.update(|p, _, _| {
            p.worktrees.error =
                Some("Offline · Connect the host to see this project's worktrees.".into())
        });
        h.capture(&format!("worktrees-offline-{theme}-{suffix}"));
    }
}
#[core::prelude::v1::test]
#[ignore = "render worktree scenes without a window"]
fn scenes() {
    if !isolated("worktrees_view::scenes") {
        return;
    }
    let scratch = std::env::temp_dir().join("plait-worktree-scenes");
    let _ = std::fs::remove_dir_all(&scratch);
    let repo = fixture_repository(&scratch);
    let out = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    for (suffix, w, h) in [("wide", 1380., 860.), ("narrow", 900., 600.)] {
        walk(&repo, out.as_deref(), suffix, w, h);
    }
}
// The accessibility collector exists only in debug builds; the frame
// benchmark compiles these tests in release.
#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn graph_accessibility_is_unique_stable_and_has_no_thread_view_bar() {
    if !isolated("worktrees_view::graph_accessibility_is_unique_stable_and_has_no_thread_view_bar")
    {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-a11y-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    install(&mut h, 4);
    let mut previous = None;
    for i in 0..2 {
        let frame = h.draw_a11y();
        frame.assert_no_duplicates(&format!("worktrees frame {i}"));
        assert!(frame.has_node(Role::Button, |node| node.label.as_deref()
            == Some("Refresh")));
        assert!(!frame.has_node(Role::Tab, |node| node.label.as_deref()
            == Some("Conversation")));
        assert!(frame.has_node(Role::Button, |node| node.label.as_deref()
            == Some(
                "Retry requests after rate limits  Working · plait/topic-1 · Clean · 2 not pushed"
            )));
        let nodes: Vec<_> = frame
            .nodes
            .into_iter()
            .filter(|node| node.path.contains("worktrees-"))
            .collect();
        if let Some(previous) = previous.as_ref() {
            assert_eq!(&nodes, previous);
        }
        previous = Some(nodes);
    }
    h.pointer(PlatformInput::MouseMove(MouseMoveEvent {
        position: point(px(600.), px(182.)),
        pressed_button: None,
        modifiers: Modifiers::default(),
    }));
    h.update(|p, _, _| assert_eq!(p.worktrees.hovered, Some(2)));
    h.draw_a11y().assert_no_duplicates("hovered worktree");
    h.update(|p, _, cx| {
        let owner = p.worktrees.rows[1].thread_id.clone().unwrap();
        p.host
            .connection
            .threads_mut()
            .iter_mut()
            .find(|t| t.id == owner)
            .unwrap()
            .title = None;
        p.worktree_labels();
        assert_eq!(p.worktrees.titles[1], "Untitled thread");
        assert!(p.worktrees.openable[1]);
        p.host.connection.threads_mut().retain(|t| t.id != owner);
        p.worktree_labels();
        assert_eq!(p.worktrees.titles[1], "Thread unavailable");
        assert!(!p.worktrees.openable[1]);
        let generation = p.worktrees.generation;
        let stamp = p
            .tabs
            .model
            .read_stamp(p.tabs.model.active().unwrap())
            .unwrap();
        p.tabs.model.next_tab().unwrap();
        p.park_workspace(cx);
        assert!(!p.worktrees_visible());
        assert!(!p.tabs.model.accepts_read(stamp));
        assert!(!p.accepts_worktrees(stamp, generation));
        p.tabs.model.close(stamp.lifetime.tab, false).unwrap();
        assert!(!p.accepts_worktrees(stamp, generation));
        p.refresh_worktrees_view(true, cx);
        assert_eq!(p.worktrees.generation, generation + 1);
        assert!(p.worktrees.items.is_empty());
    });
}
/// Handlers belong to the last drawn frame. A size reply that re-sorts the
/// lanes before the next frame must not turn Remove into another worktree's.
#[core::prelude::v1::test]
fn remove_asks_for_the_worktree_that_was_drawn() {
    if !isolated("worktrees_view::remove_asks_for_the_worktree_that_was_drawn") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-drawn-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    install(&mut h, 4);
    h.update(|p, window, cx| {
        // No run is working here: Remove waits for one.
        for t in p.host.connection.threads_mut() {
            t.status = ThreadStatus::Idle;
        }
        p.host
            .connection
            .threads_mut()
            .iter_mut()
            .filter(|t| t.id == "worktree-scene-1")
            .for_each(|t| t.status = ThreadStatus::Idle);
        let row = &mut p.worktrees.rows[1];
        row.preservation.refusal = None;
        row.preservation.unpushed = Some(0);
        p.worktrees.selected = 1;
        // What the drawn Remove button holds.
        let drawn = p.worktrees.rows[1].path.clone();
        let identity = p.worktrees.rows[1].identity.clone();
        // A measurement reply re-sorts by size before the next frame.
        p.worktrees.sizes[3] = Some(Ok(Size {
            source: 9_000_000_000,
            build: 0,
            incomplete: false,
            entries: 1,
        }));
        p.sort_worktree_lanes(true, cx);
        assert_ne!(p.worktrees.rows[1].path, drawn);
        p.ask_remove_view_worktree(&drawn, identity.as_ref(), window, cx);
        let asked = p.worktrees.removal.asking.take().map(|a| a.path);
        assert_eq!(
            asked,
            Some(drawn.clone()),
            "Remove asked about another worktree"
        );
        // A row that is no longer the one shown, or a replaced folder, asks nothing.
        let other = (1..4).find(|&i| i != p.worktrees.selected).unwrap();
        let (path, id) = (
            p.worktrees.rows[other].path.clone(),
            p.worktrees.rows[other].identity.clone(),
        );
        p.worktrees.rows[other].preservation.refusal = None;
        p.ask_remove_view_worktree(&path, id.as_ref(), window, cx);
        assert!(p.worktrees.removal.asking.is_none());
        let replaced = plait_client::git::worktree::Identity {
            dev: "1".into(),
            ino: "999".into(),
        };
        p.ask_remove_view_worktree(&drawn, Some(&replaced), window, cx);
        assert!(p.worktrees.removal.asking.is_none());
    });
    drop(h);
    let _ = std::fs::remove_dir_all(scratch);
}
/// A host helper runs to the end even when its result is retired, so
/// refreshes that arrive while one reads wait for it and run once.
#[core::prelude::v1::test]
fn refreshes_during_a_read_run_once_after_it() {
    if !isolated("worktrees_view::refreshes_during_a_read_run_once_after_it") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-once-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    install(&mut h, 4);
    h.update(|p, _, cx| {
        assert!(p.host.connection.connected && p.worktrees_visible());
        let generation = p.worktrees.generation;
        for force in [false, false, true, false] {
            p.refresh_worktrees_view(force, cx);
        }
        assert_eq!(
            p.worktrees.generation,
            generation + 1,
            "inventories overlapped"
        );
        assert_eq!(p.worktrees.again, Some(Again::Full { force: true }));
    });
    h.update(|p, _, _| {
        assert!(!p.worktrees.reading);
        assert_eq!(p.worktrees.again, None);
    });
    drop(h);
    let _ = std::fs::remove_dir_all(scratch);
}
/// A summary costs no Git: the title and state word come from it. Only a
/// run's end reads again, and then only that checkout's row.
#[core::prelude::v1::test]
fn a_summary_reads_nothing_and_a_run_end_reads_one_row() {
    if !isolated("worktrees_view::a_summary_reads_nothing_and_a_run_end_reads_one_row") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-ended-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    install(&mut h, 4);
    h.update(|p, _, cx| {
        let generation = p.worktrees.generation;
        let summary = |p: &Plait, i: usize| {
            p.host
                .connection
                .threads()
                .iter()
                .find(|t| t.id == format!("worktree-scene-{i}"))
                .unwrap()
                .clone()
        };
        let paths: Vec<_> = p.worktrees.rows.iter().map(|r| r.path.clone()).collect();
        let sample = |i: usize| paths[i].clone();
        let one = sample(1);
        let apply = |p: &mut Plait, live: Vec<ThreadSummary>, cx: &mut Context<Plait>| {
            let (relabel, ended) = p.worktree_summaries(&live);
            plait_client::navigation::merge_threads(p.host.connection.threads_mut(), live).unwrap();
            p.worktrees_after_summaries(relabel, ended, cx);
        };
        // Mid-run: a new title and still working.
        let mut running = summary(p, 1);
        assert_eq!(running.status, ThreadStatus::Running);
        running.title = Some("Retry after the limit resets".into());
        running.updated_at = running.updated_at.map(|t| t + 1);
        apply(p, vec![running.clone()], cx);
        assert_eq!(
            p.worktrees.generation, generation,
            "a summary started a read"
        );
        assert!(!p.worktrees.reading && p.worktrees.again.is_none());
        let at = p.worktrees.rows.iter().position(|r| r.path == one).unwrap();
        assert_eq!(p.worktrees.titles[at], "Retry after the limit resets");
        assert_eq!(p.worktrees.words[at], "Working");
        // An idle thread changing is not a run ending either.
        let mut idle = summary(p, 3);
        idle.title = Some("Keep every attachment".into());
        apply(p, vec![idle], cx);
        assert_eq!(p.worktrees.generation, generation);
        // The run ends: one row read for that checkout.
        running.status = ThreadStatus::Idle;
        apply(p, vec![running.clone()], cx);
        assert_eq!(p.worktrees.generation, generation + 1);
        assert!(p.worktrees.reading);
        // Ends arriving during that read wait for it; two checkouts become one full read.
        let mut waiting = summary(p, 2);
        assert!(waiting.status.busy());
        waiting.status = ThreadStatus::Idle;
        apply(p, vec![waiting], cx);
        assert_eq!(p.worktrees.generation, generation + 1);
        assert_eq!(
            p.worktrees.again,
            Some(Again::Only(sample(2))),
            "queued the one checkout"
        );
        p.reread_worktree(sample(2), cx);
        assert_eq!(p.worktrees.again, Some(Again::Only(sample(2))));
        p.reread_worktree(sample(3), cx);
        assert_eq!(p.worktrees.again, Some(Again::Full { force: false }));
    });
    drop(h);
    let _ = std::fs::remove_dir_all(scratch);
}
/// Remove waits for a run in that worktree: its facts were read before the
/// run started, and the host would refuse it then.
#[core::prelude::v1::test]
fn remove_waits_for_a_run_in_that_worktree() {
    if !isolated("worktrees_view::remove_waits_for_a_run_in_that_worktree") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-busy-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    install(&mut h, 4);
    h.update(|p, window, cx| {
        let at = p
            .worktrees
            .rows
            .iter()
            .position(|r| r.thread_id.as_deref() == Some("worktree-scene-1"))
            .unwrap();
        p.worktrees.rows[at].preservation = Preservation {
            build_output: None,
            changed: Some(0),
            unpushed: Some(0),
            exclusive: Some(0),
            refusal: None,
            can_keep_local: true,
        };
        p.worktrees.selected = at;
        let (path, id) = (
            p.worktrees.rows[at].path.clone(),
            p.worktrees.rows[at].identity.clone(),
        );
        for status in [ThreadStatus::Running, ThreadStatus::AwaitingApproval] {
            p.host
                .connection
                .threads_mut()
                .iter_mut()
                .find(|t| t.id == "worktree-scene-1")
                .unwrap()
                .status = status;
            assert!(p.run_in_worktree(&p.worktrees.rows[at]));
            p.ask_remove_view_worktree(&path, id.as_ref(), window, cx);
            assert!(
                p.worktrees.removal.asking.is_none(),
                "Remove asked during a run"
            );
        }
        // Another thread working in the same folder counts too.
        p.host
            .connection
            .threads_mut()
            .iter_mut()
            .find(|t| t.id == "worktree-scene-1")
            .unwrap()
            .status = ThreadStatus::Idle;
        p.host.connection.threads_mut().push(ThreadSummary {
            id: "worktree-scene-guest".into(),
            checkout: path.clone(),
            status: ThreadStatus::Running,
            ..Default::default()
        });
        assert!(p.run_in_worktree(&p.worktrees.rows[at]));
        p.host
            .connection
            .threads_mut()
            .retain(|t| t.id != "worktree-scene-guest");
        assert!(!p.run_in_worktree(&p.worktrees.rows[at]));
        p.ask_remove_view_worktree(&path, id.as_ref(), window, cx);
        assert_eq!(
            p.worktrees.removal.asking.take().map(|a| a.path),
            Some(path)
        );
    });
    drop(h);
    let _ = std::fs::remove_dir_all(scratch);
}
/// A worktree removed here leaves its thread marked missing in the sidebar
/// and composer, as the sidebar's own Remove does.
#[core::prelude::v1::test]
fn removal_here_marks_the_folder_missing_for_the_sidebar() {
    if !isolated("worktrees_view::removal_here_marks_the_folder_missing_for_the_sidebar") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-gone-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    install(&mut h, 4);
    h.update(|p, _, cx| {
        // Lists change only for a host verified on this connection.
        p.verify_identity_for_test(plait_client::tabs::HostKey {
            id: "fixture-host".into(),
            store: "/store".into(),
        });
        let machine = p.machines.active.clone();
        let row = p.worktrees.rows[3].clone();
        p.navigation.available.insert(row.path.clone(), true);
        assert!(
            p.worktrees
                .removal
                .ask(&row, p.tabs.host.clone(), "removal".into())
        );
        let attempt = p
            .worktrees
            .removal
            .answer(Some(plait_client::worktree_removal::Choice::RemoveIfPushed))
            .unwrap();
        let removed = plait_client::host::worktree::Operation {
            request_id: "creation-3".into(),
            thread_id: "12345678-1234-4abc-8abc-123456789abc".into(),
            state: plait_client::host::worktree::Stage::Removed,
            worktree: None,
            error: None,
        };
        p.settle_worktree_removal(&attempt, &machine, Ok(removed), cx);
        assert_eq!(p.navigation.available.get(&row.path), Some(&false));
        assert!(p.worktrees.removal.pending.is_none());
        // A refusal changes nothing about the folder.
        let other = p.worktrees.rows[1].path.clone();
        p.navigation.available.insert(other.clone(), true);
        p.worktrees.rows[1].preservation.refusal = None;
        p.worktrees.rows[1].preservation.unpushed = Some(0);
        let row = p.worktrees.rows[1].clone();
        assert!(
            p.worktrees
                .removal
                .ask(&row, p.tabs.host.clone(), "refused".into())
        );
        let attempt = p
            .worktrees
            .removal
            .answer(Some(plait_client::worktree_removal::Choice::RemoveIfPushed))
            .unwrap();
        let refused = plait_client::host::worktree::Operation {
            request_id: "creation-1".into(),
            thread_id: "12345678-1234-4abc-8abc-123456789abc".into(),
            state: plait_client::host::worktree::Stage::Ready,
            worktree: None,
            error: Some(plait_client::host::HostError {
                retry_after_seconds: None,
                code: "busy".into(),
                message: "New work appeared.".into(),
                outcome: None,
            }),
        };
        p.settle_worktree_removal(&attempt, &machine, Ok(refused), cx);
        assert_eq!(p.navigation.available.get(&other), Some(&true));

        // A reply that lands after a machine switch settles the removal but
        // leaves this machine's lists alone: the same path here is another
        // folder (#132).
        let removed = plait_client::host::worktree::Operation {
            request_id: "creation-1".into(),
            thread_id: "12345678-1234-4abc-8abc-123456789abc".into(),
            state: plait_client::host::worktree::Stage::Removed,
            worktree: None,
            error: None,
        };
        let row = p.worktrees.rows[1].clone();
        let ask = |p: &mut Plait, id: &str| {
            assert!(
                p.worktrees
                    .removal
                    .ask(&row, p.tabs.host.clone(), id.into())
            );
            p.worktrees
                .removal
                .answer(Some(plait_client::worktree_removal::Choice::RemoveIfPushed))
                .unwrap()
        };
        let attempt = ask(p, "switched");
        let elsewhere = plait_client::machines::MachineId::new("workstation").unwrap();
        p.settle_worktree_removal(&attempt, &elsewhere, Ok(removed.clone()), cx);
        assert!(p.worktrees.removal.pending.is_none());
        assert_eq!(p.navigation.available.get(&row.path), Some(&true));
        // Its result shows only in the asking machine's Worktrees tab.
        let root = p.host.creation.checkout.clone();
        p.worktrees.removal_project = Some((machine.clone(), root));
        assert!(p.removal_here());
        p.machines.active = elsewhere;
        assert!(!p.removal_here());
        p.machines.active = machine.clone();
        // After a reconnect, before the host is checked again, a reply on the
        // same machine changes no list either.
        let attempt = ask(p, "reconnected");
        p.host.connection.epoch += 1;
        assert!(!p.host_verified_here());
        p.settle_worktree_removal(&attempt, &machine, Ok(removed), cx);
        assert!(p.worktrees.removal.pending.is_none());
        assert_eq!(p.navigation.available.get(&row.path), Some(&true));
    });
    drop(h);
    let _ = std::fs::remove_dir_all(scratch);
}
/// One window holds one removal state. A removal whose reply was lost in one
/// project stays with that project's Worktrees tab.
// The accessibility collector exists only in debug builds; the frame
// benchmark compiles these tests in release.
#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn an_unconfirmed_removal_stays_with_its_project() {
    if !isolated("worktrees_view::an_unconfirmed_removal_stays_with_its_project") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-scope-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let other = scratch.join("other-project");
    std::fs::create_dir_all(&other).unwrap();
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    install(&mut h, 4);
    let shown = |frame: &crate::ui::a11y::A11yFrame| {
        frame
            .nodes
            .iter()
            .any(|n| n.path.ends_with("worktrees-result"))
    };
    let (mut here, mut there) = (None, None);
    h.update(|p, window, cx| {
        p.worktrees.selected = 3;
        let row = p.worktrees.rows[3].clone();
        p.ask_remove_view_worktree(&row.path, row.identity.as_ref(), window, cx);
        let attempt = p
            .worktrees
            .removal
            .answer(Some(plait_client::worktree_removal::Choice::RemoveIfPushed))
            .unwrap();
        assert!(
            p.worktrees
                .removal
                .finish(&attempt, Err("Connection lost.".into()))
        );
        here = p.tabs.model.active();
        let checkout = p.tab_checkout(other.clone());
        let id = p
            .tabs
            .model
            .open(
                TabTarget::Worktrees { checkout },
                Some(p.host.connection.epoch),
            )
            .unwrap();
        p.tabs.model.activate(id).unwrap();
        there = Some(id);
        assert!(p.worktrees_visible() && !p.removal_here());
        // Refresh here reads this project; it does not resend the other's removal.
        p.refresh_worktrees_view(true, cx);
        assert!(!p.worktree_busy);
        assert!(p.worktrees.removal.pending.is_some());
    });
    assert!(!shown(&h.draw_a11y()), "another project's removal is shown");
    h.update(|p, _, _| {
        assert_ne!(here, there);
        p.tabs.model.activate(here.unwrap()).unwrap();
        assert!(p.removal_here());
    });
    assert!(shown(&h.draw_a11y()));
    drop(h);
    let _ = std::fs::remove_dir_all(scratch);
}
/// Reopen follows restore: a Worktrees tab returns only for a project the
/// sidebar still verifies.
#[core::prelude::v1::test]
fn reopen_returns_worktrees_only_for_a_present_project() {
    if !isolated("worktrees_view::reopen_returns_worktrees_only_for_a_present_project") {
        return;
    }
    use crate::ui::{tab_lifecycle::Intent, tabs::TabNavigation};
    let scratch =
        std::env::temp_dir().join(format!("plait-worktree-reopen-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    install(&mut h, 4);
    let count = |p: &Plait| {
        p.tabs
            .model
            .tabs()
            .iter()
            .filter(|t| t.session().target().is_worktrees())
            .count()
    };
    for present in [true, false] {
        h.update(|p, window, cx| {
            use std::os::unix::fs::MetadataExt;
            let open: Vec<_> = p
                .tabs
                .model
                .tabs()
                .iter()
                .filter(|t| t.session().target().is_worktrees())
                .map(|t| t.id())
                .collect();
            for id in open {
                p.tabs.model.close(id, false).unwrap();
            }
            let root = p.host.creation.checkout.clone();
            let meta = std::fs::metadata(&root).unwrap();
            let identity = plait_client::tabs::DirectoryIdentity {
                device: meta.dev(),
                inode: meta.ino(),
            };
            p.tabs.identity.checkouts.insert(root.clone(), identity);
            let checkout = p.tab_checkout(root.clone());
            assert_eq!(checkout.directory, Some(identity));
            let id = p
                .tabs
                .model
                .open(
                    TabTarget::Worktrees { checkout },
                    Some(p.host.connection.epoch),
                )
                .unwrap();
            p.tabs.model.close(id, false).unwrap();
            if !present {
                // The project left the sidebar after its tab was closed.
                p.tabs.identity.checkouts.remove(&root);
            }
            assert_eq!(count(p), 0);
            p.request_tab(Intent::Navigate(TabNavigation::Reopen), window, cx);
        });
        h.update(|p, _, _| assert_eq!(count(p), usize::from(present), "present {present}"));
    }
    drop(h);
    let _ = std::fs::remove_dir_all(scratch);
}
/// Assistive technology keeps its place by node id: every control in the view
/// is named, and each keeps its id when a size reply re-sorts the lanes.
// The accessibility collector exists only in debug builds; the frame
// benchmark compiles these tests in release.
#[cfg(debug_assertions)]
#[core::prelude::v1::test]
fn graph_controls_are_named_and_keep_their_ids_when_lanes_resort() {
    if !isolated("worktrees_view::graph_controls_are_named_and_keep_their_ids_when_lanes_resort") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-worktree-ids-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    install(&mut h, 4);
    let buttons = |frame: &crate::ui::a11y::A11yFrame| {
        let mut found: Vec<_> = frame
            .nodes
            .iter()
            .filter(|n| n.path.contains("worktrees-view") && n.role == Role::Button)
            .map(|n| (n.path.clone(), n.node_id, n.label.clone()))
            .collect();
        found.sort();
        found
    };
    let before = buttons(&h.draw_a11y());
    assert!(before.len() > 10, "{before:?}");
    for (path, _, label) in &before {
        assert!(
            label.as_deref().is_some_and(|l| !l.trim().is_empty()),
            "unnamed {path}"
        );
    }
    h.update(|p, _, cx| {
        p.worktrees.sizes[3] = Some(Ok(Size {
            source: 9_000_000_000,
            build: 0,
            incomplete: false,
            entries: 1,
        }));
        p.sort_worktree_lanes(true, cx);
    });
    let frame = h.draw_a11y();
    frame.assert_no_duplicates("worktrees re-sorted");
    let after: Vec<_> = buttons(&frame)
        .into_iter()
        .map(|(path, id, _)| (path, id))
        .collect();
    for (path, id, _) in &before {
        assert!(
            after.contains(&(path.clone(), *id)),
            "{path} changed its id"
        );
    }
    drop(h);
    let _ = std::fs::remove_dir_all(scratch);
}

struct RemovalPrompt(FocusHandle);
impl Focusable for RemovalPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.0.clone()
    }
}
impl EventEmitter<PromptResponse> for RemovalPrompt {}
impl Render for RemovalPrompt {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[core::prelude::v1::test]
fn removal_prompt_freezes_choices_and_identity_across_resort_and_refresh() {
    if !isolated(
        "worktrees_view::removal_prompt_freezes_choices_and_identity_across_resort_and_refresh",
    ) {
        return;
    }
    use plait_client::{host::worktree::Request, worktree_removal::Choice};
    use std::{cell::RefCell, rc::Rc};
    let scratch =
        std::env::temp_dir().join(format!("plait-worktree-prompt-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    let captured = Rc::new(RefCell::new(None));
    h.cx.update(|cx| {
        let captured = captured.clone();
        cx.set_prompt_builder(move |_, title, detail, buttons, handle, window, cx| {
            let view = cx.new(|cx| RemovalPrompt(cx.focus_handle()));
            *captured.borrow_mut() = Some((
                view.clone(),
                title.to_owned(),
                detail.unwrap().to_owned(),
                buttons
                    .iter()
                    .map(|b| b.label().to_string())
                    .collect::<Vec<_>>(),
            ));
            handle.with_view(view, window, cx)
        });
    });
    for (published, choice) in [
        (true, Some(Choice::RemoveIfPushed)),
        (false, Some(Choice::KeepLocal)),
        (false, None),
    ] {
        install(&mut h, 4);
        let mut original = None;
        h.update(|p, window, cx| {
            p.worktrees.removal = Default::default();
            p.worktree_busy = false;
            p.host
                .connection
                .threads_mut()
                .iter_mut()
                .filter(|t| t.id == "worktree-scene-1")
                .for_each(|t| t.status = ThreadStatus::Idle);
            let row = &mut p.worktrees.rows[1];
            if published {
                row.preservation.unpushed = Some(0);
                row.preservation.refusal = None;
            }
            let row = row.clone();
            p.ask_remove_view_worktree(&row.path, row.identity.as_ref(), window, cx);
            original = p.worktrees.removal.asking.clone();
            // Another inventory and size reply change both ordering and identity.
            p.worktrees.rows[1].identity.as_mut().unwrap().ino = "999".into();
            p.worktrees.rows[1].creation_id = Some("replacement".into());
            p.worktrees.rows[1].preservation.can_keep_local = false;
            p.worktrees.sizes[3] = Some(Ok(Size {
                source: 9_000_000_000,
                build: 0,
                incomplete: false,
                entries: 1,
            }));
            p.sort_worktree_lanes(true, cx);
            // Keep this test from sending a real host mutation after confirmation.
            p.worktree_busy = true;
        });
        let (prompt, title, detail, buttons) = captured.borrow_mut().take().unwrap();
        assert_eq!(title, "Remove this worktree?");
        let expected = if published {
            vec!["Remove if pushed", "Cancel"]
        } else {
            vec!["Keep local commits only", "Cancel"]
        };
        assert_eq!(buttons, expected);
        if !published {
            assert!(
                detail.contains("2 commits are not pushed; they stay on branch plait/topic-1.")
            );
        }
        let label = choice.map_or("Cancel", Choice::label);
        let index = buttons.iter().position(|s| s == label).unwrap();
        prompt.update(&mut h.cx, |_, cx| cx.emit(PromptResponse(index)));
        h.settle();
        h.update(|p, _, _| {
            assert!(p.worktrees.removal.asking.is_none());
            if let Some(choice) = choice {
                let mut expected = original.take().unwrap();
                if let Request::RemoveWorktree { allow_unpushed, .. } = &mut expected.request {
                    *allow_unpushed = choice == Choice::KeepLocal;
                }
                assert_eq!(p.worktrees.removal.pending.as_ref(), Some(&expected));
            } else {
                assert!(p.worktrees.removal.pending.is_none());
            }
        });
    }
    drop(h);
    let _ = std::fs::remove_dir_all(scratch);
}
/// A remote host's checkout paths name another machine's disk, so its sizes
/// are measured by that host, never on this Mac.
#[core::prelude::v1::test]
fn a_remote_host_measures_its_own_worktrees() {
    if !isolated("worktrees_view::a_remote_host_measures_its_own_worktrees") {
        return;
    }
    let scratch =
        std::env::temp_dir().join(format!("plait-worktree-remote-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut h = Harness::new(&repo, None, 1380., 860.);
    h.connect(&repo, false);
    install(&mut h, 4);
    h.update(|p, _, cx| {
        // The scene leaves an earlier read cancelled; this is a fresh one.
        p.worktrees.cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Point the remote host at an unavailable socket; this Mac must not measure the fixture.
        p.host_local = false;
        p.host_client = plait_client::host::Client::local_or_unavailable();
        p.worktrees.rows[0].path = repo.clone();
        p.worktrees.sizes[1] = Some(Err("The checkout folder is missing.".into()));
        for i in [0, 2, 3] {
            p.worktrees.sizes[i] = None;
        }
        let stamp = p
            .tabs
            .model
            .active()
            .and_then(|id| p.tabs.model.read_stamp(id))
            .unwrap();
        let generation = p.worktrees.generation;
        p.measure_worktrees(stamp, generation, cx);
    });
    h.settle();
    h.update(|p, _, _| {
        for i in [0, 2, 3] {
            assert!(
                matches!(&p.worktrees.sizes[i], Some(Err(_))),
                "row {i} was measured on this Mac: {:?}",
                p.worktrees.sizes[i]
            );
        }
        assert_eq!(
            p.worktrees.sizes[1],
            Some(Err("The checkout folder is missing.".into())),
            "an earlier result was replaced"
        );
        assert_eq!(p.worktrees.total, "Size unavailable");
    });
    drop(h);
    let _ = std::fs::remove_dir_all(scratch);
}
