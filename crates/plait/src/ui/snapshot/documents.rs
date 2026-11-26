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
fn rendered_document_draws_in_the_panel() {
    if !isolated("documents::rendered_document_draws_in_the_panel") {
        return;
    }
    let output = std::env::var_os("PLAIT_SNAPSHOT_DIR").map(PathBuf::from);
    walk(output.as_deref());
}

pub(super) fn walk(output: Option<&Path>) {
    use plait_client::file_view::{Stamp, classify};
    const EXAMPLE: &[u8] = br#"# Project guide

[Jump to Setup](#setup)

| Format | Extension |
|:---|---:|
| Markdown | .md |

## Setup

- [x] Install tools
- [ ] Run checks

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
    }
}
