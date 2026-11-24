//! The Permissions chip: a harness's own permission modes, as the host lists
//! them, in a menu. It stands alone so any row can place it: the caller says
//! which harness, which mode is current, and what to do with a choice.
//!
//! Pi threads use Plait's own approvals instead; their run settings are in
//! `thread_controls`.

use super::{
    Plait,
    commands::Command,
    palette::{Item, Options, Palette},
    theme::*,
    thread_controls::TextTooltip,
};
use gpui::{prelude::*, *};
use plait_client::host::harness::HarnessId;
use std::rc::Rc;

/// What to do with the mode id chosen from the menu.
pub(super) type OnPermissions = Rc<dyn Fn(&mut Plait, String, &mut Window, &mut Context<Plait>)>;

/// One Permissions chip and the menu it opens.
pub(super) struct Permissions {
    pub(super) harness: HarnessId,
    /// The mode id the chip shows and the menu marks current.
    pub(super) current: String,
    /// Why it cannot change now. The chip is disabled and says so.
    pub(super) blocked: Option<&'static str>,
    /// The chip's hover text.
    pub(super) tip: SharedString,
    /// Under the menu: when a choice applies, or what the harness is really using.
    pub(super) note: SharedString,
    pub(super) on_select: OnPermissions,
}

impl Plait {
    /// The chip, or `None` when the harness lists no modes of its own.
    pub(super) fn permissions_chip(
        &self,
        id: &'static str,
        permissions: Permissions,
        cx: &mut Context<Self>,
    ) -> Option<Control> {
        let row = self.harness_row(permissions.harness)?;
        if row.modes.is_empty() {
            return None;
        }
        let label = row.mode(&permissions.current).map_or_else(
            || SharedString::from(permissions.current.clone()),
            |mode| mode.label.clone(),
        );
        let tip = permissions.tip.clone();
        let blocked = permissions.blocked;
        let permissions = Rc::new(permissions);
        Some(
            menu_chip(id, Some("Permissions"), None, label, tip, blocked)
                .relative()
                .child(self.anchor_probe(id))
                .on_activate(cx, move |this, window, cx| {
                    this.anchor_next(id);
                    this.open_permissions(&permissions, window, cx)
                }),
        )
    }

    /// The harness's modes as a menu, the current one marked.
    pub(super) fn open_permissions(
        &mut self,
        permissions: &Permissions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if permissions.blocked.is_some() {
            return;
        }
        let Some(row) = self.harness_row(permissions.harness) else {
            return;
        };
        let section = SharedString::from(format!("{} permissions", row.name));
        let items = row
            .modes
            .iter()
            .map(|mode| {
                let item = Item::new(
                    format!("permissions-{}", mode.id),
                    mode.label.clone(),
                    Command::Permissions {
                        mode: mode.id.clone(),
                        select: permissions.on_select.clone(),
                    },
                )
                .section(section.clone())
                .detail(mode.detail.clone());
                if mode.id == permissions.current {
                    item.badge("Current", colors().blue)
                } else {
                    item
                }
            })
            .collect();
        let options = Options {
            name: "Permissions".into(),
            placeholder: format!("Choose what {} may do…", row.name).into(),
            empty: "This harness lists no permissions of its own.".into(),
            initial: Some(format!("permissions-{}", permissions.current).into()),
            mark: None,
            aside: None,
            note: Some(permissions.note.clone()),
        };
        let palette = cx.new(|cx| Palette::new(options, items, window, cx));
        self.present(palette, None, window, cx);
    }
}

/// A chip that opens a menu: an optional caption or leading glyph, the current
/// value, and a chevron. Disabled with `blocked`, which is then its description.
pub(super) fn menu_chip(
    id: impl Into<ElementId>,
    caption: Option<&'static str>,
    lead: Option<Svg>,
    value: SharedString,
    tip: SharedString,
    blocked: Option<&'static str>,
) -> Control {
    let name = match caption {
        Some(caption) => format!("{caption}: {value}"),
        None => value.to_string(),
    };
    let hover = blocked.map_or_else(|| tip.clone(), SharedString::from);
    button(id, "")
        .rounded(px(ITEM_CORNER))
        .min_w_0()
        .flex_shrink_1()
        .gap_1()
        .aria_label(name)
        .aria_description(hover.clone())
        .tooltip(move |_, cx| cx.new(|_| TextTooltip(hover.to_string())).into())
        .disabled(blocked.is_some())
        .when_some(caption, |el, caption| {
            el.child(
                div()
                    .flex_shrink_0()
                    .text_color(rgb(colors().subtle))
                    .child(caption),
            )
        })
        .when_some(lead, |el, lead| el.child(lead))
        .child(div().min_w_0().truncate().child(value))
        .child(icon("down").size(px(12.)))
}

#[cfg(test)]
mod tests {
    use super::{ITEM_CORNER, menu_chip};
    use gpui::{Styled, div, px};

    #[test]
    fn menu_chip_uses_the_chip_corner_token() {
        let mut chip = menu_chip("mode", None, None, "Default".into(), "".into(), None);
        let mut expected = div().rounded(px(ITEM_CORNER));
        assert_eq!(chip.style().corner_radii, expected.style().corner_radii);
    }
}
