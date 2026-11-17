# Study Pen 0.1.1 — portable user guide

Study Pen is a small native Windows screen annotation app. This package supports Windows 10 and Windows 11 on x86-64 computers. macOS, Linux and native ARM64 builds are not included.

## Start

Extract the complete ZIP into a writable folder, then run `study-pen.exe`. No installation or administrator access is required. Keep `LICENSE` and `THIRD_PARTY_NOTICES.txt` with the executable when redistributing the package.

Use the pen button or **Ctrl + Alt + Shift + F1** to draw above other apps. Press the same shortcut again to click, scroll and type in those apps while keeping your ink visible. Drag the six-dot grip to move the toolbar. Hover over an icon to show its name.

## Toolbar layout

Drag the Study Pen handle toward a monitor's left or right side. On the right, its end can move about 32 logical pixels past the edge before the toolbar slides out and back in vertically. The distance scales with the display. If you release before reaching that threshold, the toolbar returns fully on screen in horizontal layout. Pull the top handle away from the edge to move the vertical toolbar continuously with the pointer. It stays vertical until the entire horizontal toolbar would fit inside the usable work area, then returns to horizontal. The top and bottom edges do not request vertical layout.

Icons stay upright in both layouts. The toolbar scales uniformly to fit the monitor's usable work area, keeping every control accessible on smaller or heavily scaled displays. Its last orientation and position are saved and restored at the next launch.

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

First assign **Radial menu** in Settings → Shortcuts; it has no default key. The global shortcut works while the toolbar is hidden. Hold it to open a circular menu at the center of the monitor containing the pointer. The pointer moves to its neutral center. Move toward a sector and hover briefly to select:

| Sector | Action |
| --- | --- |
| Left | Toggle mouse / pen interaction |
| Right | Clear all ink |
| Top | Select the object eraser |
| Bottom | Open the color wheel |

Opening Colors recenters the pointer and shows 12 preset colors plus the primary and secondary color slots. The current color is highlighted. Hover briefly to set the primary color without clicking; the secondary color stays as saved. Each hold applies one tool action or color and keeps it highlighted until release; release and hold again for another selection. Releasing the shortcut's main key closes the menu, even if its modifier keys were released first. The pointer stays at its current position afterward.

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

Choose **Skip for now** during the check or download to open the installed version. Study Pen also opens that version if the feed is unavailable or the package can't be verified. To turn off startup updates, clear **Automatic updates at startup** under **Settings → General**. The first upgrade from 0.1.0 still requires choosing **Install update** in Settings or downloading the new ZIP yourself.

If a release only offers a download, close Study Pen before replacing the executable and keep the settings file next to it.

## Verify the download

The release includes `SHA256SUMS-0.1.1.txt` for its ZIP archives. Compare your ZIP's SHA-256 using this PowerShell command.

```powershell
Get-FileHash .\study-pen-windows-x86_64-portable.zip -Algorithm SHA256
```

The package also contains `SHA256SUMS.txt` for the extracted executable. Checksums detect differences from the listed files; this package does not include a code-signing certificate.
