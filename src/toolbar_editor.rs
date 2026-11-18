//! Compact toolbar appearance controls and shared drag/drop geometry.

use crate::{
    toolbar_preferences::{self, ToolbarButton, ToolbarTheme},
    ui::{ACCENT, BG, BORDER, MUTED, Painter, SELECTED, SURFACE, TEXT},
};

const GRID_LEFT: i32 = 24;
const GRID_RIGHT: i32 = 436;
const COLUMNS: usize = 4;
const CELL_WIDTH: i32 = 94;
const CELL_HEIGHT: i32 = 32;
const COLUMN_STEP: i32 = 106;
const ROW_STEP: i32 = 38;
const TOOLBAR_TOP: i32 = 306;
const THEME_TOP: i32 = 148;
const THEME_BOTTOM: i32 = 212;
const LABEL_TOP: i32 = 224;
const LABEL_BOTTOM: i32 = 268;
const RESET_TOP: i32 = 550;
const RESET_BOTTOM: i32 = 578;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drag {
    pub button: ToolbarButton,
    pub start: (i32, i32),
    pub point: (i32, i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Theme(ToolbarTheme),
    Labels,
    SeparatePointerTools,
    Reset,
    Drag(ToolbarButton),
}

fn rows(count: usize) -> usize {
    count.div_ceil(COLUMNS).max(1)
}

fn available(buttons: &[ToolbarButton]) -> Vec<ToolbarButton> {
    ToolbarButton::ALL
        .into_iter()
        .filter(|button| !buttons.contains(button))
        .collect()
}

fn available_top(buttons: &[ToolbarButton]) -> i32 {
    TOOLBAR_TOP + rows(buttons.len()) as i32 * ROW_STEP + 38
}

fn cell_bounds(index: usize, top: i32) -> [i32; 4] {
    [
        GRID_LEFT + (index % COLUMNS) as i32 * COLUMN_STEP,
        top + (index / COLUMNS) as i32 * ROW_STEP,
        CELL_WIDTH,
        CELL_HEIGHT,
    ]
}

fn contains(bounds: [i32; 4], x: i32, y: i32) -> bool {
    (bounds[0]..bounds[0] + bounds[2]).contains(&x)
        && (bounds[1]..bounds[1] + bounds[3]).contains(&y)
}

fn grid_slot(x: i32, y: i32, top: i32, count: usize) -> Option<usize> {
    if !(GRID_LEFT..GRID_RIGHT).contains(&x)
        || !(top..top + rows(count) as i32 * ROW_STEP).contains(&y)
    {
        return None;
    }
    if y >= top + (rows(count) as i32 - 1) * ROW_STEP + CELL_HEIGHT {
        return Some(count);
    }
    // Drop gaps belong to their closest cell; clicks use exact cell bounds.
    let column = ((x - GRID_LEFT + (COLUMN_STEP - CELL_WIDTH) / 2) / COLUMN_STEP)
        .clamp(0, COLUMNS as i32 - 1) as usize;
    let row = ((y - top) / ROW_STEP) as usize;
    Some((row * COLUMNS + column).min(count))
}

pub fn click(x: i32, y: i32, buttons: &[ToolbarButton]) -> Option<Command> {
    if (THEME_TOP..THEME_BOTTOM).contains(&y) {
        if (24..224).contains(&x) {
            return Some(Command::Theme(ToolbarTheme::LiquidGlass));
        }
        if (236..436).contains(&x) {
            return Some(Command::Theme(ToolbarTheme::LiquidAir));
        }
    }
    if (24..436).contains(&x) && (LABEL_TOP..LABEL_BOTTOM).contains(&y) {
        return Some(Command::Labels);
    }
    if (24..288).contains(&x) && (RESET_TOP..RESET_BOTTOM).contains(&y) {
        return Some(Command::SeparatePointerTools);
    }
    if (300..436).contains(&x) && (RESET_TOP..RESET_BOTTOM).contains(&y) {
        return Some(Command::Reset);
    }
    for (index, &button) in buttons.iter().enumerate() {
        if contains(cell_bounds(index, TOOLBAR_TOP), x, y) {
            return (!button.is_required()).then_some(Command::Drag(button));
        }
    }
    for (index, button) in available(buttons).into_iter().enumerate() {
        if contains(cell_bounds(index, available_top(buttons)), x, y) {
            return Some(Command::Drag(button));
        }
    }
    None
}

/// Reorders an enabled tool, inserts an available tool, or removes a tool when
/// dropped into Available tools. The fixed tail cannot be reordered; Hide can
/// be removed and restored between Settings and Quit.
/// Outside drops and movements under four logical pixels are cancellation.
pub fn drop_button(buttons: &mut Vec<ToolbarButton>, drag: Drag, x: i32, y: i32) -> bool {
    let dx = i64::from(x) - i64::from(drag.start.0);
    let dy = i64::from(y) - i64::from(drag.start.1);
    if dx.abs() < 4 && dy.abs() < 4 {
        return false;
    }
    if drag.button.is_required() {
        return false;
    }
    if let Some(target) = grid_slot(x, y, TOOLBAR_TOP, buttons.len()) {
        if let Some(from) = buttons.iter().position(|button| *button == drag.button) {
            if drag.button.is_pinned() {
                return false;
            }
            return toolbar_preferences::move_button(buttons, from, target.min(buttons.len() - 1));
        }
        if !toolbar_preferences::add_button(buttons, drag.button) {
            return false;
        }
        if drag.button.is_pinned() {
            return true;
        }
        let from = buttons
            .iter()
            .position(|button| *button == drag.button)
            .unwrap();
        toolbar_preferences::move_button(buttons, from, target.min(from));
        return true;
    }
    let available = available(buttons);
    if grid_slot(x, y, available_top(buttons), available.len()).is_some() {
        return toolbar_preferences::remove_button(buttons, drag.button);
    }
    false
}

pub fn paint(
    p: &Painter<'_>,
    theme: ToolbarTheme,
    labels: bool,
    separate_pointer_tools: bool,
    buttons: &[ToolbarButton],
    drag: Option<Drag>,
) {
    p.label("Appearance", 24.0, 119.0, 412.0, 5, TEXT);
    for (index, choice) in ToolbarTheme::ALL.into_iter().enumerate() {
        let x = 24.0 + index as f32 * 212.0;
        let selected = theme == choice;
        p.rounded(
            x,
            THEME_TOP as f32,
            200.0,
            64.0,
            10.0,
            if selected { SELECTED } else { SURFACE },
            if selected { ACCENT } else { BORDER },
        );
        p.label(
            choice.label(),
            x + 13.0,
            154.0,
            166.0,
            5,
            if selected { ACCENT } else { TEXT },
        );
        p.label(
            match choice {
                ToolbarTheme::LiquidGlass => "Soft, frosted depth",
                ToolbarTheme::LiquidAir => "Clear, luminous edges",
            },
            x + 13.0,
            176.0,
            166.0,
            1,
            MUTED,
        );
        if selected {
            p.line(
                &[(x + 177.0, 160.0), (x + 180.0, 163.0), (x + 186.0, 156.0)],
                ACCENT,
                1.5,
            );
        }
    }

    p.rounded(24.0, LABEL_TOP as f32, 412.0, 44.0, 9.0, SURFACE, BORDER);
    p.label("Show tool names", 37.0, 226.0, 295.0, 0, TEXT);
    p.label("Keep a label under each icon", 37.0, 244.0, 320.0, 1, MUTED);
    p.rounded(
        384.0,
        236.0,
        36.0,
        20.0,
        10.0,
        if labels { SELECTED } else { BG },
        if labels { ACCENT } else { BORDER },
    );
    p.rounded(
        if labels { 402.0 } else { 386.0 },
        238.0,
        16.0,
        16.0,
        8.0,
        if labels { ACCENT } else { MUTED },
        if labels { ACCENT } else { MUTED },
    );

    p.label("On toolbar", 24.0, 277.0, 270.0, 5, TEXT);
    p.label("Drag to arrange", 306.0, 278.0, 130.0, 1, MUTED);
    paint_grid(p, buttons, TOOLBAR_TOP, true, buttons, drag);
    let available = available(buttons);
    let top = available_top(buttons);
    p.label("Available tools", 24.0, (top - 29) as f32, 270.0, 5, TEXT);
    p.label("Drag to add", 332.0, (top - 28) as f32, 104.0, 1, MUTED);
    if available.is_empty() {
        let removing = drag.is_some_and(|drag| {
            !drag.button.is_required() && grid_slot(drag.point.0, drag.point.1, top, 0).is_some()
        });
        p.rounded(
            24.0,
            top as f32,
            412.0,
            24.0,
            8.0,
            BG,
            if removing { ACCENT } else { BORDER },
        );
        p.center(
            if removing {
                "Release to remove from toolbar"
            } else {
                "All tools are on your toolbar"
            },
            24.0,
            top as f32,
            412.0,
            24.0,
            1,
            MUTED,
        );
    } else {
        paint_grid(p, &available, top, false, buttons, drag);
    }
    p.label(
        if separate_pointer_tools {
            "Drag tools to arrange. The last controls stay fixed."
        } else {
            "Mouse / Pen share their first slot. Last controls stay fixed."
        },
        24.0,
        520.0,
        412.0,
        1,
        MUTED,
    );
    p.rounded(24.0, RESET_TOP as f32, 264.0, 28.0, 7.0, SURFACE, BORDER);
    p.label("Separate Mouse and Pen", 37.0, 554.0, 184.0, 1, TEXT);
    p.rounded(
        238.0,
        556.0,
        32.0,
        16.0,
        8.0,
        if separate_pointer_tools { SELECTED } else { BG },
        if separate_pointer_tools {
            ACCENT
        } else {
            BORDER
        },
    );
    p.rounded(
        if separate_pointer_tools { 256.0 } else { 240.0 },
        558.0,
        12.0,
        12.0,
        6.0,
        if separate_pointer_tools {
            ACCENT
        } else {
            MUTED
        },
        if separate_pointer_tools {
            ACCENT
        } else {
            MUTED
        },
    );
    p.rounded(300.0, RESET_TOP as f32, 136.0, 28.0, 7.0, SURFACE, BORDER);
    p.center(
        "Reset toolbar",
        300.0,
        RESET_TOP as f32,
        136.0,
        28.0,
        1,
        TEXT,
    );

    if let Some(drag) = drag {
        let dx = i64::from(drag.point.0) - i64::from(drag.start.0);
        let dy = i64::from(drag.point.1) - i64::from(drag.start.1);
        if dx.abs() >= 4 || dy.abs() >= 4 {
            let x = drag.point.0.saturating_sub(53).clamp(16, 338) as f32;
            let y = drag.point.1.saturating_sub(16).clamp(116, 548) as f32;
            p.rounded(x + 2.0, y + 3.0, 106.0, 32.0, 8.0, BG, BG);
            p.rounded(x, y, 106.0, 32.0, 8.0, SELECTED, ACCENT);
            p.center(drag.button.label(), x + 4.0, y, 98.0, 32.0, 1, ACCENT);
        }
    }
}

fn paint_grid(
    p: &Painter<'_>,
    choices: &[ToolbarButton],
    top: i32,
    enabled: bool,
    toolbar: &[ToolbarButton],
    drag: Option<Drag>,
) {
    let target = drag.and_then(|drag| {
        let target = grid_slot(drag.point.0, drag.point.1, top, choices.len())?;
        if !enabled {
            return Some(target);
        }
        if drag.button.is_pinned() {
            return None;
        }
        let movable = choices
            .iter()
            .take_while(|button| !button.is_pinned())
            .count();
        Some(target.min(movable.saturating_sub(1)))
    });
    for (index, button) in choices.iter().enumerate() {
        let [x, y, width, height] = cell_bounds(index, top);
        let source = drag.is_some_and(|drag| drag.button == *button);
        let destination = drag.is_some() && target == Some(index);
        p.rounded(
            x as f32,
            y as f32,
            width as f32,
            height as f32,
            7.0,
            if source {
                BG
            } else if enabled {
                SURFACE
            } else {
                BG
            },
            if source || destination {
                ACCENT
            } else {
                BORDER
            },
        );
        let pinned = enabled && button.is_pinned();
        p.center(
            button.label(),
            (x + 4) as f32,
            (y + if pinned { 1 } else { 0 }) as f32,
            (width - 8) as f32,
            if pinned { 16.0 } else { height as f32 },
            1,
            if source {
                MUTED
            } else if enabled {
                TEXT
            } else {
                MUTED
            },
        );
        if pinned && toolbar.contains(button) {
            p.center(
                if button.is_required() {
                    "Fixed"
                } else {
                    "Drag to remove"
                },
                (x + 4) as f32,
                (y + 16) as f32,
                (width - 8) as f32,
                15.0,
                1,
                MUTED,
            );
        }
    }
    if enabled
        && drag.is_some()
        && target == Some(choices.len())
        && !choices.len().is_multiple_of(COLUMNS)
    {
        let [x, y, width, height] = cell_bounds(choices.len(), top);
        p.rounded(
            x as f32,
            y as f32,
            width as f32,
            height as f32,
            7.0,
            BG,
            ACCENT,
        );
        p.center(
            "Add here",
            x as f32,
            y as f32,
            width as f32,
            height as f32,
            1,
            ACCENT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn center(index: usize, top: i32) -> (i32, i32) {
        let [x, y, width, height] = cell_bounds(index, top);
        (x + width / 2, y + height / 2)
    }

    #[test]
    fn appearance_hit_targets_match_their_exact_bounds() {
        let buttons = toolbar_preferences::default_buttons();
        assert_eq!(
            click(24, 148, &buttons),
            Some(Command::Theme(ToolbarTheme::LiquidGlass))
        );
        assert_eq!(
            click(435, 211, &buttons),
            Some(Command::Theme(ToolbarTheme::LiquidAir))
        );
        assert_eq!(click(230, 170, &buttons), None);
        assert_eq!(click(25, 225, &buttons), Some(Command::Labels));
        assert_eq!(
            click(24, 550, &buttons),
            Some(Command::SeparatePointerTools)
        );
        assert_eq!(
            click(287, 577, &buttons),
            Some(Command::SeparatePointerTools)
        );
        assert_eq!(click(288, 577, &buttons), None);
        assert_eq!(click(299, 577, &buttons), None);
        assert_eq!(click(435, 577, &buttons), Some(Command::Reset));
        assert_eq!(click(436, 577, &buttons), None);
    }

    #[test]
    fn each_enabled_and_available_cell_keeps_its_button_identity() {
        let buttons = toolbar_preferences::default_buttons();
        for (index, &button) in buttons.iter().enumerate() {
            let (x, y) = center(index, TOOLBAR_TOP);
            assert_eq!(
                click(x, y, &buttons),
                (!button.is_required()).then_some(Command::Drag(button))
            );
        }
        for (index, button) in available(&buttons).into_iter().enumerate() {
            let (x, y) = center(index, available_top(&buttons));
            assert_eq!(click(x, y, &buttons), Some(Command::Drag(button)));
        }
        assert_eq!(
            click(GRID_LEFT + CELL_WIDTH, TOOLBAR_TOP + 4, &buttons),
            None
        );
    }

    #[test]
    fn dropping_reorders_adds_and_removes_without_duplicates() {
        let mut buttons = toolbar_preferences::default_buttons();
        let drag = Drag {
            button: ToolbarButton::Pen,
            start: center(1, TOOLBAR_TOP),
            point: center(4, TOOLBAR_TOP),
        };
        assert!(drop_button(&mut buttons, drag, drag.point.0, drag.point.1));
        assert_eq!(buttons[4], ToolbarButton::Pen);
        let start = center(0, available_top(&buttons));
        let target = center(0, TOOLBAR_TOP);
        assert!(drop_button(
            &mut buttons,
            Drag {
                button: ToolbarButton::Shapes,
                start,
                point: target
            },
            target.0,
            target.1
        ));
        assert_eq!(buttons[0], ToolbarButton::Shapes);
        assert_eq!(
            buttons
                .iter()
                .filter(|button| **button == ToolbarButton::Shapes)
                .count(),
            1
        );
        let target = center(0, available_top(&buttons));
        assert!(drop_button(
            &mut buttons,
            Drag {
                button: ToolbarButton::Shapes,
                start: center(0, TOOLBAR_TOP),
                point: target
            },
            target.0,
            target.1
        ));
        assert!(!buttons.contains(&ToolbarButton::Shapes));
    }

    #[test]
    fn fixed_controls_and_cancelled_drags_preserve_toolbar() {
        let mut buttons = toolbar_preferences::default_buttons();
        let before = buttons.clone();
        let target = center(0, available_top(&buttons));
        for (button, index) in [(ToolbarButton::Settings, 7), (ToolbarButton::Quit, 9)] {
            let drag = Drag {
                button,
                start: center(index, TOOLBAR_TOP),
                point: target,
            };
            assert!(!drop_button(&mut buttons, drag, target.0, target.1));
            assert!(!drop_button(&mut buttons, drag, -100, 500));
            assert!(!drop_button(
                &mut buttons,
                drag,
                drag.start.0 + 2,
                drag.start.1 + 2
            ));
        }
        assert_eq!(buttons, before);
    }

    #[test]
    fn fixed_controls_reject_drag_start_and_tools_cannot_enter_the_tail() {
        let mut buttons = toolbar_preferences::default_buttons();
        for button in [ToolbarButton::Quit, ToolbarButton::Settings] {
            let index = buttons
                .iter()
                .position(|candidate| *candidate == button)
                .unwrap();
            let target = center(0, TOOLBAR_TOP);
            let source = center(index, TOOLBAR_TOP);
            assert_eq!(click(source.0, source.1, &buttons), None);
            let drag = Drag {
                button,
                start: source,
                point: target,
            };
            assert!(!drop_button(&mut buttons, drag, target.0, target.1));
        }
        let target = center(buttons.len() - 1, TOOLBAR_TOP);
        let drag = Drag {
            button: ToolbarButton::Pen,
            start: center(1, TOOLBAR_TOP),
            point: target,
        };
        assert!(drop_button(&mut buttons, drag, target.0, target.1));
        assert_eq!(buttons[buttons.len() - 4], ToolbarButton::Pen);
        assert_eq!(
            &buttons[buttons.len() - 3..],
            &[
                ToolbarButton::Settings,
                ToolbarButton::Hide,
                ToolbarButton::Quit
            ]
        );
        let target = center(buttons.len() - 1, TOOLBAR_TOP);
        let drag = Drag {
            button: ToolbarButton::Ruler,
            start: center(1, available_top(&buttons)),
            point: target,
        };
        assert!(drop_button(&mut buttons, drag, target.0, target.1));
        assert_eq!(buttons[buttons.len() - 4], ToolbarButton::Ruler);
        assert_eq!(
            &buttons[buttons.len() - 3..],
            &[
                ToolbarButton::Settings,
                ToolbarButton::Hide,
                ToolbarButton::Quit
            ]
        );
    }

    #[test]
    fn hide_can_be_removed_and_restored_but_cannot_be_reordered() {
        let mut buttons = toolbar_preferences::default_buttons();
        let from = center(8, TOOLBAR_TOP);
        assert_eq!(
            click(from.0, from.1, &buttons),
            Some(Command::Drag(ToolbarButton::Hide))
        );
        let target = center(0, TOOLBAR_TOP);
        let mut drag = Drag {
            button: ToolbarButton::Hide,
            start: from,
            point: target,
        };
        assert!(!drop_button(&mut buttons, drag, target.0, target.1));
        drag.point = center(0, available_top(&buttons));
        assert!(drop_button(&mut buttons, drag, drag.point.0, drag.point.1));
        assert_eq!(
            &buttons[buttons.len() - 2..],
            &[ToolbarButton::Settings, ToolbarButton::Quit]
        );
        let start = center(0, available_top(&buttons));
        let target = center(0, TOOLBAR_TOP);
        let drag = Drag {
            button: ToolbarButton::Hide,
            start,
            point: target,
        };
        assert!(drop_button(&mut buttons, drag, target.0, target.1));
        assert_eq!(
            &buttons[buttons.len() - 3..],
            &[
                ToolbarButton::Settings,
                ToolbarButton::Hide,
                ToolbarButton::Quit
            ]
        );
    }

    #[test]
    fn every_valid_toolbar_size_fits_above_the_reset_and_status_rows() {
        for count in 1..=ToolbarButton::ALL.len() {
            let buttons = ToolbarButton::ALL[..count].to_vec();
            let available = available(&buttons);
            let toolbar_bottom = TOOLBAR_TOP + rows(buttons.len()) as i32 * ROW_STEP;
            let available_bottom =
                available_top(&buttons) + rows(available.len()) as i32 * ROW_STEP;
            assert!(toolbar_bottom < available_top(&buttons) - 24);
            assert!(
                available_bottom <= 536,
                "{count} buttons must fit above the reset row"
            );
            for index in 0..count {
                let (x, y) = center(index, TOOLBAR_TOP);
                assert!(x < GRID_RIGHT && y < RESET_TOP);
            }
        }
    }
}
