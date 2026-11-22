//! Controls that open a picker remember where they were drawn, so the picker
//! opens beside them instead of at the top of the window.
use super::Plait;
use gpui::*;

impl Plait {
    /// An invisible layer filling its parent that records the parent's bounds
    /// each frame. The parent must be `relative`.
    pub(super) fn anchor_probe(&self, id: &'static str) -> impl IntoElement {
        let anchors = self.anchors.clone();
        canvas(
            move |bounds, _, _| {
                anchors.borrow_mut().insert(id, bounds);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// The next picker opens beside control `id`, if it has been drawn.
    pub(super) fn anchor_next(&mut self, id: &'static str) {
        self.pending_anchor = self.anchors.borrow().get(id).copied();
    }
}
