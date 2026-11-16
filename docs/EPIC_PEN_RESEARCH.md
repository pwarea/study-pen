# Epic Pen research and Study Pen scope

## Reference behavior

Epic Pen's central interaction is a desktop overlay: drawing captures pointer input, and selecting its cursor mode restores interaction with other apps while leaving annotations visible. Its movable toolbar and configurable shortcuts provide access to tools without opening a separate canvas. The guide also describes whole-stroke erasing, clearing, multiple displays and Windows 10/11 support. See the official [user guide](https://epicpen.com/userguide).

The official [feature overview](https://epicpen.com/features) lists a pen, highlighter, screenshot capture, preset colors and shortcuts. Its additional tools include custom colors, text, geometric shapes, boards, disappearing ink and cursor emphasis. Supported input hardware can provide pressure and multi-touch.

These public descriptions informed the interaction model. Study Pen's source, interface, icons and assets are original; no proprietary implementation or visual assets were copied.

## Study Pen's current scope

| Area | Study Pen design |
| --- | --- |
| Desktop interaction | One shortcut switches between drawing and desktop interaction; existing ink stays visible. |
| Tools | Pen, partial-stroke eraser, whole-object eraser, undo and clear. |
| Eraser sizing | Tapping selects the remembered size. Holding the shortcut and moving vertically changes the diameter with a dead zone, smooth scaling and bounds. |
| Colors | Presets, a full color picker, HEX/RGB input, persistent primary/secondary colors and a shortcut to exchange them. |
| Shortcuts | Separate configurable global shortcuts for each requested action, persisted locally. |
| Interface | A compact original toolbar and settings panel with a restrained visual style. Right-edge docking allows a short horizontal overhang before sliding out and back in vertically. Pulling away moves the vertical toolbar with the pointer; horizontal layout returns once the whole horizontal toolbar fits in the work area. Controls stay upright. An optional global shortcut opens a radial menu while the toolbar is hidden. |
| Radial menu | Four sectors for mouse/pen, clear, object eraser and colors; a color wheel offers 12 presets and the saved primary/secondary colors, selected on hover. The shortcut starts unassigned. |
| Runtime | Rust, native Windows controls and a retained bitmap with tile-based dirty updates; canvas storage is allocated only when needed. |
| Updates | Public release builds check GitHub on startup by default and offer verified updates for the portable executable. |

Highlighters, screenshots, text, shapes, arrows, boards, fading ink, cursor emphasis, pressure handling and multi-touch are outside the current scope.

The source uses the MIT license. Dependency licenses remain applicable and their notices accompany portable packages. Researching a product's public behavior does not transfer rights to its source code, branding or assets.
