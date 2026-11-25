//! Checkout context beside the thread title or below the message box.
use super::*;
use unicode_segmentation::UnicodeSegmentation;

const GAP: f32 = 8.;
const CHIP_HEIGHT: f32 = 22.;
const LABEL_SIZE: f32 = 11.;
const TITLE_SIZE: f32 = 14.;
const TITLE_MIN: f32 = 160.;
const DRAG_MIN: f32 = 16.;
const PROJECT_CHROME: f32 = 28.; // 12-point icon, 4-point gap and two 6-point insets.
const BRANCH_CHROME: f32 = PROJECT_CHROME + 2.; // Button border.

impl Plait {
    pub(super) fn checkout_context(
        &self,
        title: Option<String>,
        cx: &mut Context<Self>,
    ) -> ContainerQuery {
        let project = self.active_project().name.replace(['\r', '\n'], " ");
        let project_chip = project_chip(
            &project,
            self.visible_project_checkout().display().to_string(),
        );
        let branch = self.checkout_branch().map(|(name, worktree)| {
            (
                name.replace(['\r', '\n'], " "),
                self.branch_chip(name, worktree, cx),
            )
        });
        let in_header = title.is_some();
        // The slot is laid out after the state and action buttons take their widths.
        container_query(move |size, window, _| {
            let reserve = title.as_ref().map_or(0., |title| {
                text_width(title, TITLE_SIZE, FontWeight::SEMIBOLD, window).min(TITLE_MIN)
                    + GAP
                    + DRAG_MIN
            });
            let room = f32::from(size.width) - reserve;
            let project_width = chip_width(&project, PROJECT_CHROME, 160., window);
            let branch_width = branch
                .as_ref()
                .map(|(name, _)| chip_width(name, BRANCH_CHROME, 190., window));
            let (project_width, branch_width) = fit_chips(room, project_width, branch_width);
            row()
                .size_full()
                .gap(px(GAP))
                .when(in_header, |el| el.pr(px(DRAG_MIN)))
                .children(title.map(title_label))
                .when_some(project_width, |el, width| {
                    el.child(project_chip.w(px(width)))
                })
                .when_some(branch.zip(branch_width), |el, ((_, chip), width)| {
                    el.child(chip.w(px(width)))
                })
        })
        .h(px(CHIP_HEIGHT))
    }

    fn checkout_branch(&self) -> Option<(&str, bool)> {
        let checkout = self.visible_project_checkout();
        let worktree = self.lists().worktree(checkout);
        self.state
            .repository
            .snapshot()
            .ready()
            .filter(|snapshot| snapshot.root == *checkout && !snapshot.plain)
            .map(|snapshot| snapshot.branch.as_str())
            .or_else(|| worktree.map(|tree| tree.branch.as_str()))
            .filter(|branch| !branch.is_empty() && checkout == self.host.creation.checkout)
            .map(|branch| (branch, worktree.is_some()))
    }

    fn branch_chip(&self, branch: &str, worktree: bool, cx: &mut Context<Self>) -> Control {
        let tip = format!(
            "{} · {}",
            branch,
            self.branch_blocker().unwrap_or("Switch Branch")
        );
        button("checkout-branch", "")
            .map(|el| self.covered_tab_control(el))
            .gap(px(4.))
            .h(px(CHIP_HEIGHT))
            .px(px(6.))
            .rounded(px(6.))
            .bg(rgba(colors().hover))
            .text_size(px(LABEL_SIZE))
            .font_weight(FontWeight::NORMAL)
            .aria_label(format!("Switch Branch: {branch}"))
            .aria_keyshortcuts(self.shortcut_aria("git.switch_branch"))
            .child(icon(if worktree { "worktree" } else { "branch" }).size(px(12.)))
            .child(chip_label("branch-name", branch.replace(['\r', '\n'], " ")))
            .tooltip(move |_, cx| cx.new(|_| thread_controls::TextTooltip(tip.clone())).into())
            .on_activate(cx, |this, window, cx| this.open_branches(false, window, cx))
    }
}

fn project_chip(project: &str, path: String) -> Stateful<Div> {
    row()
        .id("checkout-project")
        .flex_shrink_0()
        .gap(px(4.))
        .h(px(CHIP_HEIGHT))
        .px(px(6.))
        .rounded(px(6.))
        .bg(rgba(colors().hover))
        .text_size(px(LABEL_SIZE))
        .text_color(rgb(colors().muted))
        .child(icon("folder").size(px(12.)))
        .child(chip_label("project-name", project.to_owned()))
        .tooltip(move |_, cx| {
            cx.new(|_| thread_controls::TextTooltip(path.clone()))
                .into()
        })
}

fn chip_label(id: &'static str, name: String) -> Div {
    div()
        .min_w_0()
        .truncate()
        .text_ellipsis_middle()
        .child(a11y_text(id, name))
}

pub(super) fn title_label(title: String) -> Stateful<Div> {
    div()
        .id("header-title")
        .min_w_0()
        .truncate()
        .text_size(px(TITLE_SIZE))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(colors().strong))
        .child(a11y_text("title", title.clone()))
        .tooltip(move |_, cx| {
            cx.new(|_| thread_controls::TextTooltip(title.clone()))
                .into()
        })
}

fn text_width(text: &str, size: f32, weight: FontWeight, window: &Window) -> f32 {
    let mut run = window.text_style().to_run(text.len());
    run.font.weight = weight;
    let line = window
        .text_system()
        .shape_line(text.to_owned().into(), px(size), &[run], None);
    f32::from(line.width).ceil()
}

struct ChipWidth {
    preferred: f32,
    minimum: f32,
}

fn chip_width(name: &str, chrome: f32, cap: f32, window: &Window) -> ChipWidth {
    let measure = |label: &str| text_width(label, LABEL_SIZE, FontWeight::NORMAL, window) + chrome;
    let preferred = measure(name).min(cap);
    // GPUI's middle ellipsis gives the prefix about twice the space of the suffix.
    let mut letters = name.grapheme_indices(true);
    let minimum = match (letters.nth(5), letters.nth_back(3)) {
        (Some((i, sixth)), Some((j, _))) if i + sixth.len() < j => {
            measure(&format!("{}…{}", &name[..i + sixth.len()], &name[j..])).min(preferred)
        }
        _ => preferred,
    };
    ChipWidth { preferred, minimum }
}

fn fit_chips(
    room: f32,
    project: ChipWidth,
    branch: Option<ChipWidth>,
) -> (Option<f32>, Option<f32>) {
    if room < project.minimum {
        return (None, None);
    }
    if let Some(branch) = branch
        && room >= project.minimum + GAP + branch.minimum
    {
        let branch = branch.preferred.min(room - GAP - project.minimum);
        return (
            Some(project.preferred.min(room - GAP - branch)),
            Some(branch),
        );
    }
    (Some(project.preferred.min(room)), None)
}
