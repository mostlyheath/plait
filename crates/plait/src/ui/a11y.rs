//! Debug/test-only accessibility node collector for headless scene checks.
//!
//! Pinned GPUI's test platform (`TestPlatform` / `TestWindow`) never activates
//! `Window::a11y`: `a11y_active_flag` starts `false` (`window.rs:1757`), is set
//! only by the `activation` closure passed to `PlatformWindow::a11y_init`
//! (`window.rs:1776-1793`), and `TestWindow` inherits the no-op default
//! (`platform.rs:1484`). Outside GPUI, a custom headless `PlatformWindow`
//! cannot be implemented without adding `raw_window_handle`, `futures`, and
//! `uuid` dependencies.
//!
//! Instead, test debug builds wrap accessible elements in [`Audited`], which
//! delegates layout, prepaint, and paint unchanged and records the exact
//! `GlobalElementId` path, AccessKit `NodeId` hash, role, label, and value
//! during `Element::prepaint`. Because recording runs in `prepaint`, reused
//! `.cached()` views skip it just as they skip GPUI's real AccessKit tree.
//! Non-test and release builds compile `Audited<E>` away to `E` at zero cost.

use gpui::*;

#[cfg(all(test, debug_assertions))]
mod collector {
    use gpui::*;
    use std::{
        cell::RefCell,
        collections::HashMap,
        hash::{Hash, Hasher},
    };

    /// One accessibility node observed during `Element::prepaint`.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct A11yNode {
        pub node_id: u64,
        pub bounds: Bounds<Pixels>,
        pub path: String,
        pub role: Role,
        pub label: Option<String>,
        pub value: Option<String>,
        pub description: Option<String>,
    }

    impl A11yNode {
        pub(crate) fn summary(&self) -> String {
            format!(
                "#{id} role={role:?} path={path:?} label={label:?} value={value:?}",
                id = self.node_id,
                role = self.role,
                path = self.path,
                label = self.label,
                value = self.value,
            )
        }
    }

    /// Two elements that hashed to the same AccessKit `NodeId` in one frame.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct DuplicateNode {
        pub node_id: u64,
        pub first: A11yNode,
        pub duplicate: A11yNode,
    }

    /// Accessibility nodes collected during a single drawn frame.
    #[derive(Default, Debug)]
    pub(crate) struct A11yFrame {
        pub nodes: Vec<A11yNode>,
        seen: HashMap<u64, usize>,
        pub duplicates: Vec<DuplicateNode>,
    }

    impl A11yFrame {
        fn push(&mut self, node: A11yNode) {
            if let Some(&first_index) = self.seen.get(&node.node_id) {
                self.duplicates.push(DuplicateNode {
                    node_id: node.node_id,
                    first: self.nodes[first_index].clone(),
                    duplicate: node,
                });
                return;
            }
            let index = self.nodes.len();
            self.seen.insert(node.node_id, index);
            self.nodes.push(node);
        }

        pub(crate) fn assert_no_duplicates(&self, scene: &str) {
            if self.duplicates.is_empty() {
                return;
            }
            let mut details = String::new();
            for dup in &self.duplicates {
                details.push_str(&format!(
                    "\n  duplicate #{}: first=({}) second=({})",
                    dup.node_id,
                    dup.first.summary(),
                    dup.duplicate.summary(),
                ));
            }
            panic!(
                "scene {scene:?} produced {} duplicate accessibility node id(s):{details}",
                self.duplicates.len()
            );
        }

        pub(crate) fn has_node(&self, role: Role, predicate: impl Fn(&A11yNode) -> bool) -> bool {
            self.nodes
                .iter()
                .any(|node| node.role == role && predicate(node))
        }
    }

    thread_local! {
        static ACTIVE: RefCell<Option<A11yFrame>> = const { RefCell::new(None) };
    }

    /// Restores the thread-local collector even if drawing panics.
    struct Guard(Option<A11yFrame>);

    impl Drop for Guard {
        fn drop(&mut self) {
            ACTIVE.with(|cell| {
                *cell.borrow_mut() = self.0.take();
            });
        }
    }

    /// Runs `draw` with prepaint accessibility collection enabled for this thread.
    pub(crate) fn collect<R>(draw: impl FnOnce() -> R) -> (R, A11yFrame) {
        let previous = ACTIVE.with(|cell| cell.borrow_mut().replace(A11yFrame::default()));
        let mut guard = Guard(previous);
        let result = draw();
        let frame = ACTIVE.with(|cell| {
            let taken = cell.borrow_mut().take().unwrap_or_default();
            *cell.borrow_mut() = guard.0.take();
            taken
        });
        std::mem::forget(guard);
        (result, frame)
    }

    /// Extracts GPUI's `GlobalElementId::accesskit_node_id` hash.
    /// `GlobalElementId::hash` writes its precomputed `u64` path hash directly.
    fn accesskit_node_id(global_id: &GlobalElementId) -> u64 {
        struct Identity(u64);
        impl Hasher for Identity {
            fn finish(&self) -> u64 {
                self.0
            }
            fn write(&mut self, _: &[u8]) {
                unreachable!("GlobalElementId::hash writes a single u64");
            }
            fn write_u64(&mut self, value: u64) {
                self.0 = value;
            }
        }
        let mut hasher = Identity(0);
        global_id.hash(&mut hasher);
        hasher.finish()
    }

    fn record_element<E: Element>(
        global_id: &GlobalElementId,
        bounds: Bounds<Pixels>,
        element: &E,
    ) {
        ACTIVE.with(|cell| {
            let mut slot = cell.borrow_mut();
            let Some(frame) = slot.as_mut() else {
                return;
            };
            let Some(role) = element.a11y_role() else {
                return;
            };
            let mut node = gpui::accesskit::Node::new(role);
            element.write_a11y_info(&mut node);
            frame.push(A11yNode {
                node_id: accesskit_node_id(global_id),
                bounds,
                path: global_id.to_string(),
                role,
                label: node.label().map(str::to_owned),
                value: node.value().map(str::to_owned),
                description: node.description().map(str::to_owned),
            });
        });
    }

    /// Transparent element wrapper that mirrors GPUI's prepaint a11y recording.
    pub struct Audited<E>(pub(super) E);

    impl<E: Styled> Styled for Audited<E> {
        fn style(&mut self) -> &mut StyleRefinement {
            self.0.style()
        }
    }

    impl<E: InteractiveElement> InteractiveElement for Audited<E> {
        fn interactivity(&mut self) -> &mut Interactivity {
            self.0.interactivity()
        }
    }

    impl<E: Element + InteractiveElement> StatefulInteractiveElement for Audited<E> {}

    impl<E: ParentElement> ParentElement for Audited<E> {
        fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
            self.0.extend(elements);
        }
    }

    impl<E: Element> Element for Audited<E> {
        type RequestLayoutState = E::RequestLayoutState;
        type PrepaintState = E::PrepaintState;

        fn id(&self) -> Option<ElementId> {
            self.0.id()
        }

        fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
            self.0.source_location()
        }

        fn a11y_role(&self) -> Option<Role> {
            self.0.a11y_role()
        }

        fn write_a11y_info(&self, node: &mut gpui::accesskit::Node) {
            self.0.write_a11y_info(node);
        }

        fn a11y_synthetic_children(
            &mut self,
            prepaint: &mut Self::PrepaintState,
            builder: &mut A11ySubtreeBuilder,
        ) {
            self.0.a11y_synthetic_children(prepaint, builder);
        }

        fn request_layout(
            &mut self,
            id: Option<&GlobalElementId>,
            inspector_id: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, Self::RequestLayoutState) {
            self.0.request_layout(id, inspector_id, window, cx)
        }

        fn prepaint(
            &mut self,
            id: Option<&GlobalElementId>,
            inspector_id: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            request_layout: &mut Self::RequestLayoutState,
            window: &mut Window,
            cx: &mut App,
        ) -> Self::PrepaintState {
            if let Some(global_id) = id {
                record_element(global_id, bounds, &self.0);
            }
            self.0
                .prepaint(id, inspector_id, bounds, request_layout, window, cx)
        }

        fn paint(
            &mut self,
            id: Option<&GlobalElementId>,
            inspector_id: Option<&InspectorElementId>,
            bounds: Bounds<Pixels>,
            request_layout: &mut Self::RequestLayoutState,
            prepaint: &mut Self::PrepaintState,
            window: &mut Window,
            cx: &mut App,
        ) {
            self.0.paint(
                id,
                inspector_id,
                bounds,
                request_layout,
                prepaint,
                window,
                cx,
            );
        }
    }
}

#[cfg(all(test, debug_assertions))]
pub use collector::Audited;
#[cfg(all(test, debug_assertions))]
pub(crate) use collector::{A11yFrame, collect};

#[cfg(not(all(test, debug_assertions)))]
pub type Audited<E> = E;

/// Accessible static text element with an explicit `ElementId`.
#[inline]
pub fn text(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Audited<Text> {
    let element = Text::new(id.into(), text.into());
    #[cfg(all(test, debug_assertions))]
    {
        Audited(element)
    }
    #[cfg(not(all(test, debug_assertions)))]
    {
        element
    }
}

/// Wraps a role-bearing stateful element so debug tests observe it in `prepaint`.
pub trait AuditA11y: Sized {
    fn audited(self) -> Audited<Self>;
}

impl AuditA11y for Stateful<Div> {
    #[inline]
    fn audited(self) -> Audited<Self> {
        #[cfg(all(test, debug_assertions))]
        {
            Audited(self)
        }
        #[cfg(not(all(test, debug_assertions)))]
        {
            self
        }
    }
}

impl AuditA11y for Text {
    #[inline]
    fn audited(self) -> Audited<Self> {
        #[cfg(all(test, debug_assertions))]
        {
            Audited(self)
        }
        #[cfg(not(all(test, debug_assertions)))]
        {
            self
        }
    }
}
