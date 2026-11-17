# Shortcut compatibility

Study Pen uses function-key shortcuts with three modifiers for its eight existing actions. The optional radial-menu shortcut starts unassigned. These defaults reduce collisions with common Windows commands and character entry. Other utilities and application settings can still conflict; registration must be checked on the computer where Study Pen runs.

| Action | Default shortcut |
| --- | --- |
| Toggle pen / desktop interaction | Ctrl + Alt + Shift + F1 |
| Clear all ink | Ctrl + Alt + Shift + F2 |
| Object eraser | Ctrl + Alt + Shift + F3 |
| Undo | Ctrl + Alt + Shift + F4 |
| Normal eraser / hold to resize | Ctrl + Alt + Shift + F5 |
| Swap primary and secondary colors | Ctrl + Alt + Shift + F6 |
| Hide / show toolbar | Ctrl + Alt + Shift + F7 |
| Exit Study Pen | Ctrl + Alt + Shift + F8 |
| Radial menu | Unassigned |

The tradeoff is a longer chord; some laptop keyboards require Fn as well. Choose another supported combination in Settings → Shortcuts if it fits your setup better.

Assign **Radial menu** explicitly to enable its global shortcut. Hold it to show the menu at the center of the monitor containing the pointer, while the toolbar is visible or hidden. The pointer moves to the neutral center. Briefly hover to choose one action or color, then release the shortcut's main key to close. Opening Colors recenters the pointer for choosing from its wheel. Adding this optional action preserves the existing shortcut assignments. See the [radial-menu guide](RADIAL_MENU.md).

## Why the earlier defaults changed

**Ctrl+Alt+D** switches Windows Magnifier to its docked view. Study Pen's earlier pen toggle overlapped that accessibility command. [Microsoft Magnifier shortcuts](https://support.microsoft.com/en-gb/accessibility/windows/magnifier/magnifier-keyboard-shortcuts-and-touch-gestures).

AltGr behaves as left Ctrl plus right Alt on Windows. This makes Ctrl+Alt letter shortcuts interfere with ordinary character entry on international layouts. [Microsoft PowerToys keyboard notes](https://learn.microsoft.com/en-gb/windows/powertoys/keyboard-manager).

The official Turkish Q layout maps AltGr+Q to **@** and AltGr+E to **€**. Consequently, the earlier Ctrl+Alt+Q exit and Ctrl+Alt+E eraser shortcuts could react to typing those characters. [Microsoft's Turkish Q keyboard](https://learn.microsoft.com/en-us/globalization/keyboards/kbdtuq), [official layout data](https://raw.githubusercontent.com/MicrosoftDocs/globalization/main/globalization/keyboards/kbdtuq.html).

Microsoft Word also uses Ctrl+Alt+D for an endnote, Ctrl+Alt+C for copying formatting, Ctrl+Alt+O for Outline view and Ctrl+Alt+Z for previous edit locations. A global shortcut may intercept those commands while Word is active. [Microsoft Word shortcuts](https://support.microsoft.com/en-us/accessibility/word/keyboard-shortcuts-in-word).

## Registration and saved assignments

Study Pen registers shortcuts with Windows using `RegisterHotKey` and checks each result. An unavailable shortcut is marked for its action in Settings and remains inactive until reassigned; toolbar buttons still work. A successful registration confirms that Windows accepted that global shortcut. It does not prove that another app has no local command using the same keys.

Windows-key combinations are reserved for the operating system, and F12 is reserved for debuggers. Study Pen avoids them in defaults and rejects unsupported or unsafe assignments. Auto-repeat is disabled with `MOD_NOREPEAT`, so holding a shortcut does not repeatedly trigger its action. The eraser has separate hold tracking for resizing. [Microsoft RegisterHotKey reference](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey).

Valid custom assignments are preserved when settings are upgraded. Unsafe legacy assignments are repaired, while colors, sizes and other preferences are retained. Recheck Shortcuts after upgrading, especially if another utility reserves a replacement binding.

## Local Alt menu handling

Study Pen's floating windows do not have a traditional menu bar. They consume unused `WM_SYSCHAR` and `SC_KEYMENU` menu requests locally. This adjustment applies to Study Pen's windows; Windows and other applications' sound settings are unchanged. Standard system commands are otherwise preserved. [WM_SYSCHAR](https://learn.microsoft.com/en-us/windows/win32/menurc/wm-syschar), [WM_SYSCOMMAND](https://learn.microsoft.com/en-us/windows/win32/menurc/wm-syscommand).
