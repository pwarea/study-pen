# Platform support

Study Pen supports **Windows 10/11 on x86-64**. Its executable is a Windows PE binary. macOS and Linux builds are not implemented or tested.

Rust supports macOS and Linux targets, so native ports are feasible. That does not make the present Win32 implementation portable by changing a compiler target. The ink geometry and undo model can be shared; windowing, input, drawing presentation, tray integration, settings locations and HTTPS integration need platform backends. [Rust's platform support list](https://doc.rust-lang.org/stable/rustc/platform-support.html) distinguishes compiler support from application support.

## macOS

A native backend could use AppKit windows for the toolbar and a transparent ink layer. AppKit exposes [window levels](https://developer.apple.com/documentation/appkit/nswindow/level-swift.struct) and [mouse-event pass-through](https://developer.apple.com/documentation/appkit/nswindow/ignoresmouseevents). A macOS implementation would also need native rendering, menu-bar integration, Retina/multi-display handling and packaging.

Global keyboard behavior must follow macOS permissions and event APIs. Apple's [global event monitor documentation](https://developer.apple.com/documentation/appkit/nsevent/addglobalmonitorforevents(matching:handler:)) states that keyboard observation requires Accessibility trust and that monitors observe rather than suppress events. The appropriate shortcut and hold-to-resize implementation must be evaluated separately; this is not a promise that every Windows shortcut can be reproduced unchanged.

## Linux

Linux support needs an explicit display-system plan. An X11 backend and a Wayland backend have different window, input and shortcut mechanisms. Supporting one does not imply support for the other.

On Wayland, the [GlobalShortcuts portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html) provides shortcut activation and deactivation, with desktop-mediated binding. A desktop overlay also needs appropriate compositor protocols: [wlr layer-shell](https://raw.githubusercontent.com/swaywm/wlr-protocols/master/unstable/wlr-layer-shell-unstable-v1.xml) defines desktop layers and input regions, but a port must detect its availability rather than assume every compositor exposes it. Portal implementations are [desktop-backend dependent](https://docs.flatpak.org/en/latest/portal-api-reference.html).

The [InputCapture portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.InputCapture.html) is not a general replacement for Windows pointer polling: activation is controlled by the compositor and currently uses screen-edge pointer barriers. Therefore, preserving global hold-and-move eraser sizing while other apps have focus needs desktop-specific validation. A Wayland port may require compositor integration or adjusted behavior.

## Proposed direction

Keep the lightweight Rust ink model, then introduce separate Windows, macOS and Linux backends. Validate macOS and a defined Linux desktop/display stack with native prototypes before promising feature parity or publishing packages. No cross-platform implementation is included in this release.
