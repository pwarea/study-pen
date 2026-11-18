#![windows_subsystem = "windows"]

mod app;
mod cursor_motion;
mod docking;
mod glass;
mod hotkeys;
mod identity;
mod ink;
mod install_update;
mod overlay;
mod panel_material;
mod panel_primitives;
mod pen_cursor;
mod radial;
mod removal;
mod ruler;
mod settings;
mod shape_picker;
mod splash;
mod startup_update;
mod toolbar_drag;
mod toolbar_editor;
mod toolbar_labels;
mod toolbar_motion;
mod toolbar_preferences;
mod tray;
mod ui;
mod updates;

fn main() {
    if let Err(error) = app::run() {
        app::show_error(&error);
    }
}
