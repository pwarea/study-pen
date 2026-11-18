# Study Pen 0.2.2 — portable user guide

Study Pen is a small native Windows screen annotation app. This package supports Windows 10 and Windows 11 on x86-64 computers. macOS, Linux and native ARM64 builds are not included.

## Start

Extract the complete ZIP into a writable folder, then run `study-pen.exe`. No installation or administrator access is required. Keep `LICENSE` and `THIRD_PARTY_NOTICES.txt` with the executable when redistributing the package.

Use the combined **Mouse/Pen** button or **Ctrl + Alt + Shift + F1** to switch between drawing above other apps and clicking, scrolling or typing in them. Your ink stays visible. Drag the six-dot grip to move the toolbar. Hover over an icon to show its name.

The pen pointer uses a native fountain nib with its drawing tip aligned to the stroke. Very fast pointer movement briefly enlarges it with a smooth animation to help you locate it, then returns it to its normal size.

## Toolbar layout

Drag the Study Pen handle toward a monitor's left or right side. On the right, its end can move about 32 logical pixels past the edge before the toolbar slides out and back in vertically. The distance scales with the display. If you release before reaching that threshold, the toolbar returns fully on screen in horizontal layout. Pull the top handle away from the edge to move the vertical toolbar continuously with the pointer. It stays vertical until the entire horizontal toolbar would fit inside the usable work area, then returns to horizontal. The top and bottom edges do not request vertical layout.

Antialiased icons stay upright in both layouts and scale with the display. The toolbar scales uniformly to fit the monitor's usable work area, keeping every control accessible on smaller or heavily scaled displays. Its last orientation and position are saved and restored at the next launch.

Open **Settings → Toolbar** to choose **Liquid Glass** or the translucent blue-gradient **Liquid Air** theme. Turn on **Show tool names** for compact labels; hovering always shows the full tool name. Drag items within **On toolbar** to reorder them, from **Available tools** to add them, or back to **Available tools** to remove them. The last controls stay in the order **Settings → Hide toolbar → Quit Study Pen**. Settings and Quit are required; Hide toolbar is optional. **Reset toolbar** restores the default tool list and order; it leaves your theme, labels, colors and shortcuts unchanged.

Mouse and pen share one switch by default. Enable **Separate Mouse and Pen** in **Settings → Toolbar** if you prefer individual buttons. This display preference keeps their configured positions, so switching back restores your separate-button order.

## Shapes and fill

Add **Shapes** and **Fill shapes** in **Settings → Toolbar**. Click Shapes to choose **Rectangle**, **Ellipse**, **Line**, **Arrow** or **Triangle**, then drag on the screen to draw with the selected color and pen width. The shape menu also supports normal Windows keyboard navigation.

Choose Fill shapes and click inside a rectangle, ellipse or triangle to fill it with the primary color. This fills Study Pen shape objects; it does not fill areas of the underlying app or freehand outlines. The regular eraser removes parts of shapes, the object eraser removes the whole object, and Undo restores drawing, filling and erasing.

## Ruler

Add **Ruler** in **Settings → Toolbar**, then click it to show or hide the ruler. Drag its body to move it. Select the pen and begin a stroke near either long edge to draw a straight line along that edge.

The visible ruler also blocks new pen ink from crossing its body. Edge guidance leaves room for the full brush width, keeping thick strokes outside the ruler. A freehand stroke that crosses the ruler pauses at its edge and resumes on the other side; Undo removes all visible parts of that single held stroke together. Existing ink is unchanged.

Drag **Rotate** to rotate around the ruler's center, or click it to advance by 15° clockwise. Small angles snap to horizontal; hold **Shift** while rotating for precise angles without that snap. The mouse wheel rotates by 5° per notch. Click the degree readout to reset the angle to exactly **0.0°**; dragging the readout moves the ruler instead. Click its close button to hide it. Its theme follows the toolbar; its angle lasts only for the current app session.

## Shortcuts

| Action | Default shortcut |
| --- | --- |
| Toggle pen / desktop interaction | Ctrl + Alt + Shift + F1 |
| Clear all ink | Ctrl + Alt + Shift + F2 |
| Object eraser | Ctrl + Alt + Shift + F3 |
| Undo | Ctrl + Alt + Shift + F4 |
| Normal eraser / hold to resize | Ctrl + Alt + Shift + F5 |
| Change color | Ctrl + Alt + Shift + F6 |
| Hide / show toolbar | Ctrl + Alt + Shift + F7 |
| Exit Study Pen | Ctrl + Alt + Shift + F8 |
| Radial menu | Unassigned — choose in Settings |

Change shortcuts in Settings → Shortcuts. A shortcut already used by Windows or another program must be changed before Study Pen can register it. Startup registration failures are displayed beside the affected action, whose shortcut stays inactive until reassigned. The tool's button remains available.

The earlier Ctrl+Alt letter defaults could overlap Magnifier, Word and AltGr typing, including @ and € on Turkish Q. The function-key defaults reduce these collisions. Other software can still reserve the same combination. Valid custom shortcuts are kept; unsafe legacy bindings are replaced. Some laptop keyboards also require Fn to reach F1–F8.

Tap the normal eraser shortcut to use the last size. Hold it for a moment and move the mouse up to enlarge it, or down to shrink it. Release the key before erasing with the mouse. The size is remembered. The object eraser deletes an entire touched stroke; the normal eraser removes touched portions. Undo also restores erasing and clearing.

## Colors

Open the color panel from the toolbar. Choose the primary or secondary color, select a quick swatch or use the full color picker. You can type HEX or RGB values. The change-color shortcut swaps the two remembered colors.

## Radial menu

First assign **Radial menu** in Settings → Shortcuts; it has no default key. The global shortcut works while the toolbar is hidden. Hold it to open a circular menu at the center of the monitor containing the pointer. The pointer moves to its neutral center. Move toward a sector to highlight it, then release the shortcut's main key to apply the choice. Move between sectors while holding to reconsider.

| Sector | Action |
| --- | --- |
| Left | Toggle mouse / pen interaction |
| Right | Clear all ink |
| Top | Select the object eraser |
| Bottom | Open the color wheel |

Hold over Colors briefly to open the color wheel and recenter the pointer. It offers 12 preset colors plus the primary and secondary color slots, with the current primary color highlighted. Move over a swatch and release to set the primary color. The secondary color stays as saved. After moving out toward a swatch, return to the center and hold briefly to go back to the tool menu. The center shows **Back** when this is available.

Each hold applies at most one tool action or color on release. Release in the center, between sectors or beyond the wheel's outer tolerance to cancel, or press Esc while holding. The shortcut's main key closes the menu even if its modifier keys were released first. The pointer stays at its current position afterward.

## Hide, restore and exit

Use **Ctrl + Alt + Shift + F7** or the tray menu to hide or show the toolbar. Ink and drawing mode remain as selected; use the pen toggle to return to desktop interaction. The tray icon remains available while the app is running. Use **Ctrl + Alt + Shift + F8**, the tray Exit command or Settings → Exit to close it completely. Closing the settings panel only closes that panel.

## Find or remove Study Pen

The first launch adds a **Study Pen** desktop icon and a Start menu shortcut. If you move the folder, move the settings file with it and update the shortcuts to point to the new executable location.

Open **Settings → General → Remove Study Pen…** to remove the current executable, its settings and recovery files, and its matching shortcuts.

## Carry or reset preferences

`study-pen.settings.json` is created beside `study-pen.exe` when that folder is writable. Carry both files to preserve colors, sizes, shortcuts and preferences. A protected or read-only folder uses `%LOCALAPPDATA%\StudyPen\portable\` instead, with a separate JSON file for that app location. These fallback preferences remain on that computer; to carry them, copy the active JSON file beside the executable as `study-pen.settings.json` in a writable folder.

Study Pen restores the newest valid preferences belonging to that copy and saves them beside the executable again once possible. The older `%LOCALAPPDATA%\StudyPen\settings.json` file is imported only when the copy has no valid settings of its own. A `.bak` file supports recovery, and unreadable originals are preserved as `.corrupt-N` files.

Close Study Pen before manually editing preferences. A full reset requires removing the copy's settings and `.bak` files from both locations; remove legacy settings and their backup too if you do not want them imported again. Release ZIPs do not contain another user's preferences. Ink is temporary and disappears when the app exits.

## Updates

Release builds check GitHub at startup by default. With **Automatic updates at startup** enabled, a startup window shows the progress of checking and downloading. Study Pen installs a newer portable package automatically when it comes from the build's release repository and passes verification. The updated app restarts with your saved settings.

Choose **Skip for now** during the startup check or download to open the installed version. Study Pen also opens that version if the feed is unavailable or the package can't be verified. To turn off startup updates, clear **Automatic updates at startup** under **Settings → General**.

To check manually, choose **Settings → General → Check for updates**, even when startup updates are disabled. If a newer verified release is available, choose **Install update**. The progress window allows cancellation while preparing the update; installation itself cannot be canceled once replacement begins. Study Pen then closes and restarts the updated copy with its saved preferences. Unverified or unsupported packages use a browser download instead.

If a release only offers a download, close Study Pen before replacing the executable and keep the settings file next to it.

## Verify the download

The release includes `SHA256SUMS-0.2.2.txt` for its ZIP archives. Compare your ZIP's SHA-256 using this PowerShell command.

```powershell
Get-FileHash .\study-pen-windows-x86_64-portable.zip -Algorithm SHA256
```

The package also contains `SHA256SUMS.txt` for the extracted executable. Checksums detect differences from the listed files; this package does not include a code-signing certificate.
