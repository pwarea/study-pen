//! Persisted toolbar choices shared by the renderer and customization editor.

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarButton {
    Cursor,
    Pen,
    Eraser,
    ObjectEraser,
    Undo,
    Clear,
    Color,
    Settings,
    Hide,
    Quit,
    Shapes,
    Ruler,
    Fill,
}

impl ToolbarButton {
    pub const ALL: [Self; 13] = [
        Self::Cursor,
        Self::Pen,
        Self::Eraser,
        Self::ObjectEraser,
        Self::Undo,
        Self::Clear,
        Self::Color,
        Self::Settings,
        Self::Hide,
        Self::Quit,
        Self::Shapes,
        Self::Ruler,
        Self::Fill,
    ];

    pub const DEFAULT: [Self; 10] = [
        Self::Cursor,
        Self::Pen,
        Self::Eraser,
        Self::ObjectEraser,
        Self::Undo,
        Self::Clear,
        Self::Color,
        Self::Settings,
        Self::Hide,
        Self::Quit,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Cursor => "Cursor",
            Self::Pen => "Pen",
            Self::Eraser => "Eraser",
            Self::ObjectEraser => "Object eraser",
            Self::Undo => "Undo",
            Self::Clear => "Clear screen",
            Self::Color => "Color",
            Self::Settings => "Settings",
            Self::Hide => "Hide toolbar",
            Self::Quit => "Quit Study Pen",
            Self::Shapes => "Shapes",
            Self::Ruler => "Ruler",
            Self::Fill => "Fill shapes",
        }
    }

    /// Required controls remain available, but their positions are customizable.
    pub const fn is_required(self) -> bool {
        matches!(self, Self::Settings | Self::Quit)
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarTheme {
    #[default]
    LiquidGlass,
    LiquidAir,
}

impl ToolbarTheme {
    pub const ALL: [Self; 2] = [Self::LiquidGlass, Self::LiquidAir];

    pub const fn label(self) -> &'static str {
        match self {
            Self::LiquidGlass => "Liquid Glass",
            Self::LiquidAir => "Liquid Air",
        }
    }
}

pub fn default_buttons() -> Vec<ToolbarButton> {
    ToolbarButton::DEFAULT.to_vec()
}

/// Retains the chosen order and appends any missing required controls.
pub fn sanitize_buttons(buttons: &mut Vec<ToolbarButton>) {
    if buttons.is_empty() {
        *buttons = default_buttons();
        return;
    }
    let mut seen = 0_u16;
    buttons.retain(|button| {
        let bit = 1 << *button as u32;
        let duplicate = seen & bit != 0;
        seen |= bit;
        !duplicate
    });
    for required in [ToolbarButton::Settings, ToolbarButton::Quit] {
        if !buttons.contains(&required) {
            buttons.push(required);
        }
    }
}

/// Adds an available button at the end, keeping existing button positions.
pub fn add_button(buttons: &mut Vec<ToolbarButton>, button: ToolbarButton) -> bool {
    if buttons.contains(&button) {
        return false;
    }
    buttons.push(button);
    true
}

pub fn remove_button(buttons: &mut Vec<ToolbarButton>, button: ToolbarButton) -> bool {
    if button.is_required() {
        return false;
    }
    let Some(index) = buttons.iter().position(|candidate| *candidate == button) else {
        return false;
    };
    buttons.remove(index);
    true
}

/// Moves a button to its final zero-based position, rejecting stale indices.
pub fn move_button(buttons: &mut Vec<ToolbarButton>, from: usize, to: usize) -> bool {
    if from >= buttons.len() || to >= buttons.len() || from == to {
        return false;
    }
    let button = buttons.remove(from);
    buttons.insert(to, button);
    true
}

pub fn reset_buttons(buttons: &mut Vec<ToolbarButton>) -> bool {
    if buttons.as_slice() == ToolbarButton::DEFAULT {
        return false;
    }
    *buttons = default_buttons();
    true
}

pub fn deserialize_theme<'de, D>(deserializer: D) -> Result<ToolbarTheme, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

pub fn deserialize_labels<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(false))
}

pub fn deserialize_buttons<'de, D>(deserializer: D) -> Result<Vec<ToolbarButton>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let serde_json::Value::Array(values) = value else {
        return Ok(default_buttons());
    };
    let mut buttons = values
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect();
    sanitize_buttons(&mut buttons);
    Ok(buttons)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn customization_moves_buttons_without_duplicates_and_keeps_required_controls() {
        use ToolbarButton::*;
        let mut buttons = vec![Settings, Pen, Color, Quit];
        assert!(add_button(&mut buttons, Ruler));
        assert!(!add_button(&mut buttons, Pen));
        assert!(move_button(&mut buttons, 4, 1));
        assert_eq!(buttons, [Settings, Ruler, Pen, Color, Quit]);
        assert!(move_button(&mut buttons, 0, 4));
        assert_eq!(buttons, [Ruler, Pen, Color, Quit, Settings]);
        assert!(move_button(&mut buttons, 3, 0));
        assert_eq!(buttons, [Quit, Ruler, Pen, Color, Settings]);
        assert!(remove_button(&mut buttons, Pen));
        assert!(!remove_button(&mut buttons, Pen));
        assert!(!remove_button(&mut buttons, Settings));
        assert!(!remove_button(&mut buttons, Quit));
        assert!(!move_button(&mut buttons, 4, 0));
        assert!(!move_button(&mut buttons, 0, 4));
        assert!(!move_button(&mut buttons, 0, 0));
        assert_eq!(buttons, [Quit, Ruler, Color, Settings]);
    }

    #[test]
    fn sanitization_preserves_custom_order_and_the_smallest_recoverable_toolbar() {
        use ToolbarButton::*;
        let mut buttons = vec![Fill, Pen, Fill, Ruler, Pen];
        sanitize_buttons(&mut buttons);
        assert_eq!(buttons, [Fill, Pen, Ruler, Settings, Quit]);
        for button in [Fill, Pen, Ruler] {
            assert!(remove_button(&mut buttons, button));
        }
        sanitize_buttons(&mut buttons);
        assert_eq!(buttons, [Settings, Quit]);
        let mut empty = Vec::new();
        sanitize_buttons(&mut empty);
        assert_eq!(empty, ToolbarButton::DEFAULT);
    }

    #[test]
    fn reset_restores_original_order_and_reports_when_nothing_changed() {
        let mut buttons = vec![ToolbarButton::Settings, ToolbarButton::Quit];
        assert!(reset_buttons(&mut buttons));
        assert_eq!(buttons, ToolbarButton::DEFAULT);
        assert!(!reset_buttons(&mut buttons));
    }

    #[test]
    fn persisted_names_are_independent_of_visible_labels() {
        assert_eq!(
            serde_json::to_string(&ToolbarButton::ObjectEraser).unwrap(),
            "\"object_eraser\""
        );
        assert_eq!(
            serde_json::to_string(&ToolbarTheme::LiquidAir).unwrap(),
            "\"liquid_air\""
        );
        assert_eq!(ToolbarButton::Quit.label(), "Quit Study Pen");
        assert_eq!(ToolbarTheme::LiquidGlass.label(), "Liquid Glass");
    }

    #[test]
    fn missing_quit_migrates_at_the_end_without_reordering_existing_buttons() {
        use ToolbarButton::*;
        let mut buttons = vec![Ruler, Settings, Fill, Pen];
        sanitize_buttons(&mut buttons);
        assert_eq!(buttons, [Ruler, Settings, Fill, Pen, Quit]);
        sanitize_buttons(&mut buttons);
        assert_eq!(buttons, [Ruler, Settings, Fill, Pen, Quit]);

        let mut already_present = vec![Quit, Fill, Settings, Ruler];
        sanitize_buttons(&mut already_present);
        assert_eq!(already_present, [Quit, Fill, Settings, Ruler]);
        for button in ToolbarButton::ALL {
            assert_eq!(button.is_required(), matches!(button, Settings | Quit));
        }
    }
}
