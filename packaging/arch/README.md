# Pulse as a package

`PKGBUILD` here wraps the published AppImage in a pacman package: the file goes
to `/opt/pulse/Pulse.AppImage`, and the package owns a `pulse-appimage` launcher,
a desktop entry and the icon.

## Two update paths, one file each

The copy in `/opt` is a **seed**. It is root-owned and never modified after
install, so pacman's file record stays accurate. On first launch the launcher
copies the seed to

```
${XDG_DATA_HOME:-~/.local/share}/pulse-appimage/Pulse.AppImage
```

and runs that copy. The AppImage exports its path as `APPIMAGE`, so Pulse's own
updater finds the writable file and replaces it in place. That is the normal way
to update.

`paru -Syu pulse-appimage` (or reinstalling a package) replaces the seed; its
size/mtime id changes, and the launcher refreshes the user copy on the next
launch. Both paths converge on the same running file, and neither one leaves the
package database describing a file that is no longer there.

If the data directory cannot be written, the launcher runs the seed directly —
Pulse still starts, and only the in-app update is refused.

## Install / update the package

```sh
makepkg -f
paru -U pulse-appimage-<version>-<rel>-x86_64.pkg.tar.zst   # or pacman -U
```

For a new release: change `pkgver`, run `updpkgsums` in this directory to refresh
the AppImage and icon hashes, then rebuild. Bump `pkgrel` when only the packaging
changes (like the launcher script).
