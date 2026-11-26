Prebuilt for **x86_64 Linux**.

**The AppImage is the install.** It is portable: settings, the library database
and artwork live in a `data/` folder beside the image, not in `~/.config` or
`~/.local/share`. One folder —

```
~/Applications/Pulse/
├── Pulse.AppImage
└── data/
```

— and deleting it removes Pulse and everything it wrote.

```sh
mkdir -p ~/Applications/Pulse
chmod +x Pulse-x86_64.AppImage
mv Pulse-x86_64.AppImage ~/Applications/Pulse/Pulse.AppImage
~/Applications/Pulse/Pulse.AppImage
```

The in-app updater replaces that file in place, so it needs to live somewhere
your user owns. Upgrading from 5.8 copies the old library in on first run and
leaves `~/.local/share/pulse-music` untouched.

Or the tarball:

```sh
tar -xzf pulse-x86_64-linux.tar.gz
install -m 755 pulse ~/.local/bin/pulse
```

Requires PipeWire with `pipewire-alsa`, plus `alsa-lib`, `libxkbcommon` and
`libxcb` at runtime. The AppImage also needs the host's Vulkan driver and
`libwayland-client`, which are not bundled because they have to match the GPU.
