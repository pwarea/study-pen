# Study Pen

[![Downloads](https://img.shields.io/github/downloads/pwarea/study-pen/study-pen-windows-x86_64-portable.zip?displayAssetName=false&label=downloads)](https://github.com/pwarea/study-pen/releases)

Study Pen lets you draw over any ordinary Windows app. Mark up a PDF, point something out while sharing your screen, or work through a problem on the desktop. Switch back to your cursor and the ink stays visible.

![Study Pen toolbar design on light and dark backgrounds](docs/study-pen-toolbar.png)

## Download

**[Download Study Pen for Windows](https://github.com/pwarea/study-pen/releases/latest/download/study-pen-windows-x86_64-portable.zip)**

Extract the ZIP to a writable folder and run **study-pen.exe**. The first launch adds a **Study Pen** icon to your desktop and an entry in Start.

Study Pen runs on 64-bit Windows 10 and 11. It needs no installer or administrator access.

## Use Study Pen

- Drag the six-dot grip to move the glass toolbar. Pull it past a side edge to dock it vertically.
- Use the combined **Mouse/Pen** button to switch between drawing and desktop interaction. Enable **Separate Mouse and Pen** in **Settings → Toolbar** for individual buttons. Undo restores the last drawing, erasing, or clearing action.
- A native fountain nib marks the pen tip. Very fast pointer movement briefly enlarges it to help you locate it.
- Use the regular eraser for part of a line and the object eraser for the whole stroke. Hold the eraser shortcut and move the mouse up or down to change its size.
- Pick colors with the swatches or enter HEX or RGB values. Assign a shortcut to swap between your two saved colors.
- Open **Settings → Shortcuts** to customize keys and bind the optional radial menu. You can hide or restore the toolbar from the system tray.
- In **Settings → Toolbar**, choose **Liquid Glass** or the blue **Liquid Air** theme, show tool names, and drag tools to add, remove or reorder them. The last controls stay **Settings → Hide toolbar (optional) → Quit Study Pen**. **Reset toolbar** restores the default tools and order.
- Add **Shapes**, **Fill shapes** and **Ruler** from the toolbar editor. Draw rectangles, ellipses, lines, arrows or triangles; fill closed shapes with your selected color; or draw straight pen strokes along the ruler.
- Drag the ruler to move it. Drag **Rotate** to change its angle, hold **Shift** for precise angles, or click the degree readout to reset it to **0.0°**.

| Action | Default shortcut |
| --- | --- |
| Toggle drawing | Ctrl + Alt + Shift + F1 |
| Clear screen | Ctrl + Alt + Shift + F2 |
| Object eraser | Ctrl + Alt + Shift + F3 |
| Undo | Ctrl + Alt + Shift + F4 |
| Eraser | Ctrl + Alt + Shift + F5 |
| Swap colors | Ctrl + Alt + Shift + F6 |
| Show or hide toolbar | Ctrl + Alt + Shift + F7 |
| Exit | Ctrl + Alt + Shift + F8 |

Preferences travel with the app in `study-pen.settings.json`. To remove a copy and its shortcuts, open **Settings → General → Remove Study Pen…**. [Portable user guide](docs/PORTABLE_README.md)

## Updates

Release builds check GitHub on startup by default. Study Pen shows update progress before opening the toolbar and automatically installs a newer, verified portable package from its release repository. Choose **Skip for now** during checking or downloading to use your installed version. Switch off **Automatic updates at startup** under **Settings → General** to opt out.

To update while Study Pen is open, go to **Settings → General**, choose **Check for updates**, then **Install update** if a new version is available. The progress window lets you cancel before installation starts. Study Pen restarts itself when the update is ready.

If the update feed is unavailable or an update can't be verified, Study Pen opens the installed version. Releases that cannot be installed in the app offer a browser download instead. Existing settings are preserved during verified in-place updates.

## Build

On Windows with stable Rust and the MSVC build tools, run `cargo build --release --locked`. To make the portable ZIP, run `powershell -ExecutionPolicy Bypass -File scripts/build.ps1`. [Release guide](docs/GITHUB_RELEASE.md)

## License

Study Pen is available under the MIT License. The portable ZIP includes third-party license notices. [LICENSE](LICENSE)
