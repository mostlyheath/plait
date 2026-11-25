//! Window-local review panel state and sheet sizing.
use crate::review::recorded::Target;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelKind {
    #[default]
    Changes,
    Files,
    History,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Scope {
    #[default]
    ThisTurn,
    Uncommitted,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Panel {
    pub open: bool,
    pub kind: PanelKind,
    pub fill: bool,
    pub scope: Scope,
    pub recorded: Option<Target>,
    /// Find in Changes, kept with the tab while the window is open; never saved.
    pub diff_find: crate::tabs::bookmark::FindBookmark,
}

impl Panel {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    pub fn open_on(&mut self, kind: PanelKind) {
        self.kind = kind;
        self.open = true;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn set_fill(&mut self, fill: bool) {
        self.fill = fill;
        self.open = true;
    }

    pub fn select_scope(&mut self, scope: Scope) {
        self.scope = scope;
    }

    pub fn fills(&self, sheet: f32) -> bool {
        self.fill || sheet < 1020.
    }

    pub fn width(&self, sheet: f32, preferred: Option<f32>) -> f32 {
        if self.fills(sheet) {
            return sheet;
        }
        preferred.unwrap_or(sheet / 2.).clamp(360., sheet - 420.)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_the_sheet_and_keeps_the_conversation_readable() {
        let mut panel = Panel::default();
        assert_eq!(panel.width(1200., None), 600.);
        assert_eq!(panel.width(1200., Some(200.)), 360.);
        assert_eq!(panel.width(1200., Some(999.)), 780.);
        assert_eq!(panel.width(1019., Some(450.)), 1019.);
        assert_eq!(panel.width(1020., None), 510.);
        panel.set_fill(true);
        assert_eq!(panel.width(1200., Some(450.)), 1200.);
        panel.set_fill(false);
        assert_eq!(panel.width(1200., Some(450.)), 450.);
    }

    #[test]
    fn reopening_preserves_kind_and_scope() {
        let mut panel = Panel::default();
        assert!(!panel.open);
        panel.open_on(PanelKind::Files);
        panel.select_scope(Scope::Uncommitted);
        panel.close();
        panel.toggle();
        assert!(panel.open);
        assert_eq!(panel.kind, PanelKind::Files);
        assert_eq!(panel.scope, Scope::Uncommitted);
    }
}
