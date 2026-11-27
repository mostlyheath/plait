use super::*;

#[core::prelude::v1::test]
fn rendered_document_file_and_navigation() {
    if !isolated("documents::rendered_document_file_and_navigation") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-doc-scene-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let mut source = String::from(
        "---\ntitle: Demo\n---\n# Read me\n\n*Example* with **bold**, ~~old~~ and `code`.\n\n> quote\n\n- [x] task\n\n| Heading | Value |\n|---|---|\n| One | Two |\n\n```rust\nfn main() {}\n```\n\n---\n\n![pixel](image.png) [jump](#end) [other](next.md#two) [absent](gone.md) [missing](next.md#absent)\n\nNote[^a]\n\n[^a]: Footnote\n\n<!-- literal -->\n\n",
    );
    source.push_str(&"Read **this** passage.\n\n".repeat(60));
    source.push_str("# End\n");
    std::fs::write(repo.join("demo.md"), source).unwrap();
    std::fs::write(repo.join("next.md"), "# One\n\n## Two").unwrap();
    std::fs::write(
        repo.join("image.png"),
        include_bytes!("../../../../../runtime/test/support/images/0.png"),
    )
    .unwrap();
    let host = test_host::TestHost::read_client(scratch.clone());
    let mut h = Harness::with_client(&repo, None, 900., 600., None, host);
    h.update(|p, _, cx| p.open_file("demo.md".into(), None, cx));
    h.update(|p, _, _| {
        let v = &mut p.workspace.file_viewer;
        assert!(v.rendered());
        assert!(v.doc.pictures.contains_key("image.png"));
        v.doc.outline = true;
    });
    h.update(|p, w, cx| w.focus(&p.focus, cx));
    h.draw();
    h.keystroke("escape");
    h.update(|p, _, cx| {
        assert!(!p.workspace.file_viewer.doc.outline);
        p.workspace.file_viewer.doc.outline = true;
        cx.notify();
    });
    h.keystroke("down");
    h.keystroke("enter");
    h.update(|p, w, cx| {
        assert!(!p.workspace.file_viewer.doc.outline);
        assert!(
            p.workspace
                .file_viewer
                .doc
                .list
                .as_ref()
                .unwrap()
                .logical_scroll_top()
                .item_ix
                > 50
        );
        p.follow_document("#read-me", cx);
        p.follow_document("#end", cx);
        assert!(
            p.workspace
                .file_viewer
                .doc
                .list
                .as_ref()
                .unwrap()
                .logical_scroll_top()
                .item_ix
                > 50
        );
        p.open_find(w, cx);
        let field = p.workspace.file_viewer.find.field.clone().unwrap();
        field.update(cx, |f, cx| f.set_text("read this", cx));
        p.check_document_links(cx);
    });
    h.update(|p, _, cx| {
        let v = &p.workspace.file_viewer;
        assert_eq!(v.find.matches.len(), 60);
        let results = v.doc.results.as_ref().unwrap();
        assert_eq!(results.len(), 2, "{results:?}");
        assert!(results.iter().any(|s| s.contains("gone.md")));
        assert!(
            results
                .iter()
                .any(|s| s.contains("next.md#absent") && s.contains("missing heading"))
        );
        p.follow_document("next.md#two", cx);
    });
    h.update(|p, _, _| {
        let v = &p.workspace.file_viewer;
        assert_eq!(v.path.as_deref(), Some(Path::new("next.md")));
        assert!(v.rendered());
    });
    h.draw();
    // Switching back reuses the projection, including for a megabyte of Markdown.
    let paragraph = "A paragraph with **bold text** and an [anchor](#title).\n\n";
    std::fs::write(
        repo.join("large.md"),
        format!(
            "# Title\n\n{}",
            paragraph.repeat(1_048_576 / paragraph.len() + 1)
        ),
    )
    .unwrap();
    h.update(|p, w, cx| {
        p.close_find(w, cx);
        p.open_file("large.md".into(), None, cx);
    });
    let mut cached = None;
    h.update(|p, _, cx| {
        cached = p.workspace.file_viewer.document().cloned();
        p.open_file("next.md".into(), None, cx);
    });
    h.update(|p, _, cx| {
        let start = Instant::now();
        p.open_file("large.md".into(), None, cx);
        let elapsed = start.elapsed();
        assert!(Arc::ptr_eq(
            cached.as_ref().unwrap(),
            p.workspace.file_viewer.document().unwrap()
        ));
        assert!(
            elapsed < Duration::from_millis(50),
            "cached open took {elapsed:?}"
        );
    });
    h.draw();
    drop(h);
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn editor_opens_the_document_in_its_checkout() {
    if !isolated("documents::editor_opens_the_document_in_its_checkout") {
        return;
    }
    let scratch = std::env::temp_dir().join(format!("plait-editor-{}", std::process::id()));
    let repo = fixture_repository(&scratch);
    let linked = scratch.join("linked checkout");
    git(
        &repo,
        &["worktree", "add", "--detach", linked.to_str().unwrap()],
    );
    let name = "src/Agent's guide.md";
    for checkout in [&repo, &linked] {
        let file = checkout.join(name);
        let source = format!(
            "# {}\n\n```\n{}```\n",
            checkout.display(),
            "a line\n".repeat(100)
        );
        std::fs::write(&file, source).unwrap();
        let host = test_host::TestHost::read_client(scratch.clone());
        let mut h = Harness::with_client(checkout, None, 900., 600., None, host);
        h.update(|p, _, cx| {
            p.preferences.value.open_files_with =
                plait_client::handoff::EditorChoice::Custom("ed {file} {project}".into());
            p.open_file(name.into(), None, cx);
        });
        let verify = |p: &mut Plait, cx: &mut Context<Plait>| {
            p.open_in_editor(cx);
            let launch = p.ways_out.started.pop().unwrap();
            assert_eq!(Path::new(&launch.args[0]), file);
            assert_eq!(Path::new(&launch.args[1]), checkout);
            assert!(file.is_file());
        };
        h.update(|p, _, cx| {
            assert!(p.workspace.file_viewer.rendered());
            verify(p, cx);
            p.workspace
                .file_viewer
                .doc
                .list
                .as_ref()
                .unwrap()
                .scroll_to(ListOffset {
                    item_ix: 40,
                    offset_in_item: px(1.),
                });
        });
        h.draw();
        h.update(|p, _, cx| {
            assert_eq!(p.way_out_target().unwrap().line, 43);
            p.remember_document_mode(Path::new(name), true);
            verify(p, cx);
        });
        h.update(|p, window, cx| {
            let changes = &p.state.repository.snapshot().ready().unwrap().changes;
            let index = changes
                .iter()
                .position(|change| change.path == Path::new(name))
                .unwrap();
            p.select_file(index, plait_client::model::DiffMode::Unstaged, window, cx);
        });
        h.update(|p, _, cx| verify(p, cx));
        let refused = |p: &mut Plait, _: &mut Window, cx: &mut Context<Plait>| {
            p.open_in_editor(cx);
            assert!(p.ways_out.started.is_empty());
            assert!(p.notice.as_ref().unwrap().contains("Cannot open"));
        };
        let saved = checkout.join("saved-src");
        std::fs::rename(checkout.join("src"), &saved).unwrap();
        std::os::unix::fs::symlink(&saved, checkout.join("src")).unwrap();
        h.update(refused);
        std::fs::remove_file(checkout.join("src")).unwrap();
        std::fs::rename(saved, checkout.join("src")).unwrap();
        std::fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(checkout.join("README.md"), &file).unwrap();
        h.update(refused);
        std::fs::remove_file(&file).unwrap();
        h.update(refused);
    }
    std::fs::remove_dir_all(scratch).unwrap();
}

#[core::prelude::v1::test]
fn rendered_document_draws_in_the_panel() {
    if !isolated("documents::rendered_document_draws_in_the_panel") {
        return;
    }
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    walk(output.as_deref());
}

#[core::prelude::v1::test]
fn find_reveals_the_end_of_one_long_code_row() {
    use plait_client::file_view::{Stamp, classify};
    if !isolated("documents::find_reveals_the_end_of_one_long_code_row") {
        return;
    }
    let mut h = header::fixture(None, 900., 600.);
    h.update(|p, _, cx| {
        p.tabs
            .model
            .panel_mut()
            .unwrap()
            .open_on(plait_client::panel::PanelKind::Files);
        let path = Path::new("long.md");
        let source = format!(
            "```\n{}needle\n{}next\n```",
            "x".repeat(4 * 1024 * 1024 - 64 * 1024),
            "\u{200b}".repeat(1365 * 12 + 1)
        );
        let stamp = Stamp {
            len: source.len() as u64,
            device: 1,
            inode: 1,
            modified: None,
        };
        let viewer = &mut p.workspace.file_viewer;
        viewer.path = Some(path.into());
        viewer.opened = Some(Arc::new(classify(stamp, Some(source.into_bytes()), path)));
        viewer.prepare_document();
        let doc = viewer.document().unwrap();
        assert_eq!(doc.rows.len(), 2);
        // Simulate twelve fragments already measured at zero width by the platform.
        let second = &doc.rows[1];
        let start = second.bytes.start;
        let matches = doc.find("needle", false);
        let widths = viewer.doc.code_widths.entry(1).or_default();
        for i in 0..12 {
            widths.borrow_mut().insert(start + i * 4095, px(0.));
        }
        viewer.find.matches = Arc::new(matches);
        p.step_find(true, cx);
    });
    for _ in 0..3 {
        #[cfg(debug_assertions)]
        {
            let nodes = h.draw_a11y().nodes;
            let second = nodes
                .iter()
                .filter(|node| node.path.contains("document-1-"))
                .count();
            assert!((1..=8).contains(&second));
            let fragments = nodes
                .iter()
                .filter(|node| node.path.contains("document-0-"))
                .count();
            assert!((1..=3).contains(&fragments), "{fragments} code fragments");
        }
        #[cfg(not(debug_assertions))]
        h.draw();
    }
    h.update(|p, _, _| {
        let scroll = &p.workspace.file_viewer.doc.code_scroll[&0];
        assert!(scroll.max_offset().x > scroll.bounds().size.width);
        assert!(
            (scroll.offset().x + scroll.max_offset().x).abs() <= px(1.),
            "offset {:?}, max {:?}, pending {:?}",
            scroll.offset(),
            scroll.max_offset(),
            p.workspace.file_viewer.doc.code_reveal.get()
        );
        assert_eq!(
            p.workspace.file_viewer.doc.code_scroll[&1].offset().x,
            px(0.)
        );
    });
}

pub(super) fn walk(output: Option<&Path>) {
    use plait_client::file_view::{Stamp, classify};
    const EXAMPLE: &[u8] = br#"# Project guide

Use this guide to set up the project, review the available formats, and check the steps before working on your first change. [Jump to Setup](#setup)

| Format | Extension |
|:---|---:|
| Markdown | .md |

## Setup

> Keep your local checkout up to date before you start working on a change. Check the project guide whenever the build tools change.

- [x] Install the tools used to build and test the project on your machine, then run the checks from the root of your checkout.
- [ ] Run checks

```sh
cargo test --locked -p plait ui::snapshot::documents::rendered_document_draws_in_the_panel -- --exact
```

![Preview](preview.png)"#;
    for (width, height, suffix) in [(900., 600., "narrow"), (1380., 860., "wide")] {
        let mut h = header::fixture(output, width, height);
        h.update(|p, _, cx| {
            p.themes.forced = Some(false);
            p.apply_theme(cx);
            p.tabs
                .model
                .panel_mut()
                .unwrap()
                .open_on(plait_client::panel::PanelKind::Files);
            let viewer = &mut p.workspace.file_viewer;
            let path = Path::new("guide.md");
            let bytes = EXAMPLE;
            viewer.path = Some(path.into());
            let stamp = Stamp {
                len: bytes.len() as u64,
                device: 1,
                inode: 1,
                modified: None,
            };
            viewer.opened = Some(Arc::new(classify(stamp, Some(bytes.to_vec()), path)));
            viewer.prepare_document();
            viewer.doc.pictures.insert(
                "preview.png".into(),
                super::super::images::document_image(include_bytes!(
                    "../../../../../runtime/test/support/images/0.png"
                ))
                .unwrap(),
            );
            viewer.doc.outline = true;
        });
        capture_both(&mut h, &format!("rendered-document-outline-{suffix}"));
        h.update(|p, _, cx| {
            let row = p
                .workspace
                .file_viewer
                .document()
                .unwrap()
                .heading("setup")
                .unwrap();
            p.choose_document_heading(row, cx);
            assert!(!p.workspace.file_viewer.doc.outline);
            p.follow_document("#project-guide", cx);
        });
        capture_both(&mut h, &format!("rendered-document-anchor-{suffix}"));
        if suffix == "wide" {
            let heights = |p: &Plait| {
                let list = p.workspace.file_viewer.doc.list.as_ref().unwrap();
                // Paragraph, quote, and task list item in the scene.
                [1, 5, 6].map(|i| list.bounds_for_item(i).unwrap().size.height)
            };
            let mut narrow = [px(0.); 3];
            h.update(|p, window, cx| {
                narrow = heights(p);
                p.fill_panel(window, cx);
            });
            capture_both(&mut h, "rendered-document-filled");
            h.update(|p, _, _| {
                for (filled, side) in heights(p).into_iter().zip(narrow) {
                    assert!(filled < side, "{filled:?} should be shorter than {side:?}");
                }
            });
        }
    }
}
