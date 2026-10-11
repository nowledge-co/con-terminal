# Application Icons

Tail A6 is the default for new configurations and packaged applications. The
catalog retains the persisted `default` ID as Classic, so an explicit choice
does not silently acquire different artwork. Missing and unknown IDs use Tail A6.

## Preview and persistence

The existing process-wide preview stack owns the displayed choice. Save only
updates persisted state after configuration reaches disk; discard restores the
preceding live preview or saved choice. This applies across Settings windows.

macOS uses NSApplication for the running Dock/Cmd-Tab icon and NSWorkspace for
the saved Finder icon. It never modifies signed bundle resources. Saving Tail A6
clears a custom file icon to reveal the bundle's Tail A6 resource; saving Classic
sets Classic as a custom file icon.

Windows assigns small and large window icons with WM_SETICON. Native handles
remain owned by a bounded UI-thread cache (one pair per catalog entry), because
WM_SETICON does not copy them and other windows may still refer to an older
preview. Handles are destroyed at UI-thread teardown. The executable and pinned
shortcuts are not rewritten.

X11 writes straight-alpha ARGB CARDINAL data to `_NET_WM_ICON`. It does not change
WM_CLASS, application identity, or desktop files. Saved choices atomically update
only `$XDG_DATA_HOME/icons/hicolor/256x256/apps/co.nowledge.con.png` (normally
`~/.local/share/icons/...`); system packages remain untouched. This also gives
Wayland launchers the saved choice without changing window grouping, though
desktop caches control when it appears. Flatpak skips this host integration:
its sandbox's XDG data directory is not the host launcher theme.

Live Wayland window previews are intentionally excluded: the pinned GPUI backend has no toplevel-icon API,
and changing an app ID or rewriting a desktop file would break window grouping
or create an upgrade-sensitive desktop-file override. Live Wayland support needs
an upstream-supported protocol before it can share the picker behavior.

For Windows/X11, a Root creation observer applies the current choice to newly
opened terminal, Settings, and dialog windows. Existing-window refresh is deferred
until a picker event's active window is returned to App; nested updates would
otherwise miss that window. There is no per-frame polling or terminal/render path
work. Windows handles and X11 packed pixels are cached per catalog entry. Saved
launcher writes run on the background executor, serialize with the saved-state
lock, and publish only complete PNGs.

## Packaging and verification

macOS, Linux, Flatpak, Cargo bundle metadata, and the Windows resource generator
all use `assets/app-icons/con-raccoon-tail-A6.png`. macOS explicitly resizes its
1024px ICNS representation rather than assuming the source is 1024px. Windows
generates a seven-resolution ICO at build time from the PNG rather than carrying
a second independently maintained default asset.

Tests cover old explicit Classic IDs, new/legacy omitted defaults, the catalog's
decodable images, straight-alpha X11 channel order, ICO dimensions and alpha, and
preview ownership/rollback/save races. Portable CI runs the icon tests on both
Windows and Linux. Native desktop acceptance still requires testing the chosen
window manager/taskbar and its icon cache; a macOS test cannot prove these paths.

## References

- [Windows window icon ownership](https://learn.microsoft.com/en-us/windows/win32/menurc/about-icons)
- [X11 window icon data](https://specifications.freedesktop.org/wm/latest-single/)
- [Freedesktop launcher icon lookup](https://specifications.freedesktop.org/icon-theme/latest/)
