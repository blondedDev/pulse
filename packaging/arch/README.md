# Pulse as a package

`PKGBUILD` here wraps the published AppImage in a pacman package: the file goes
to `/opt/pulse/Pulse.AppImage`, and the package owns a `pulse-appimage` launcher,
a desktop entry and the icon. The name is deliberately not `pulse`, so it does
not collide with a bare binary in `~/.local/bin`.

## Install / update

```sh
makepkg -f
paru -U pulse-appimage-<version>-1-x86_64.pkg.tar.zst   # or pacman -U
```

For a new release: change `pkgver`, run `updpkgsums` in this directory to refresh
the AppImage and icon hashes, then rebuild.

## One updater at a time

`/opt` is root-owned, so the in-app update checker cannot replace the AppImage
there; it reports that it could not write. That is intentional — the package is
the update path, and letting two things own the same file would leave the
package database describing a binary that is no longer on disk.
