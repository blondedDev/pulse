# Pulse as a package

`PKGBUILD` here installs Pulse the way it is published: the AppImage goes into
the user's own folder,

```
~/Applications/Pulse.AppImage
```

and the package owns the `pulse` launcher, the desktop entry and the icon. The
AppImage is chowned to uid/gid 1000 (this is a personal package for machines
with the same user), which is what lets Pulse's own updater replace it in place.

## Updates

Both paths write the same file:

- **In-app**: the runtime exports `APPIMAGE` as the path of that file, and the
  updater renames the verified download over it.
- **Package**: `paru -Syu pulse-appimage` installs a newer AppImage. Because the
  path is listed in `backup`, pacman will not clobber a copy the app already
  updated; if the packaged file is newer it leaves a `.pacnew` beside it.

Reinstalling the package is therefore safe after a self-update.

## Install / rebuild

```sh
makepkg -f
paru -U pulse-appimage-<version>-<rel>-x86_64.pkg.tar.zst   # or pacman -U
```

If `Pulse.AppImage` was put in place by hand before installing, pacman refuses
to overwrite an unowned file; add
`--overwrite '/home/blonded/Applications/Pulse.AppImage'`.

For a new release: change `pkgver`, run `updpkgsums` here to refresh the AppImage
and icon hashes, then rebuild. Bump `pkgrel` when only the packaging changes.
