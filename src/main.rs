#![windows_subsystem = "windows"]

mod app;
mod docking;
mod glass;
mod hotkeys;
mod identity;
mod ink;
mod install_update;
mod overlay;
mod radial;
mod removal;
mod settings;
mod splash;
mod startup_update;
mod toolbar_drag;
mod toolbar_motion;
mod tray;
mod ui;
mod updates;

fn main() {
    if let Err(error) = app::run() {
        app::show_error(&error);
    }
}
