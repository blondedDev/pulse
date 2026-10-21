# audio_engine

The playback engine: one `audio-engine` thread that owns the decoder and feeds
`audio_output`, driven by `Command`s and reporting `EngineEvent`s.

## Files

- `engine.rs` — the run loop, commands, fades, seeking.
- `stream.rs` — `StreamingSource` (a decoder on its own thread for network
  sources) and the `Source` enum the loop plays from.
- `engine_manager.rs` — the GPUI-side handle.
- `types.rs` — `Command`, `EngineEvent`, `StreamTrack`.

## Streaming

A network read can block for seconds. The engine thread must never block on it,
or pause, seek and skip stop responding. So a stream is decoded on a
`stream-decoder` thread that runs up to 96 batches ahead into a bounded channel,
and the loop only polls that channel:

- `Poll::Pending` means "no audio yet". The loop then waits for commands in
  10 ms slices instead of spinning. After 250 ms without audio it emits
  `Buffering(true)`; the next batch emits `Buffering(false)`.
- A fade only advances while the output plays samples, so a pause or seek fade
  started just before a stall would never finish. After 400 ms of starvation the
  pending pause or seek is applied directly; a pause or seek requested while
  already buffering skips the fade.
- A seek is a message with an epoch. Batches decoded before it carry the old
  epoch and are dropped, so the engine never plays stale audio after a seek.
- A read blocked on bytes that are not downloaded yet gives up as soon as a
  newer seek is queued (`MediaStream::give_up_waiting_when`, fed with "the
  control channel is not empty"). Without it, seeking into a missing part and
  back to a downloaded one would wait for the missing part first. The error of
  the abandoned read carries the old epoch and is dropped like its batches.
- Dropping the `StreamingSource` calls its interrupt, which wakes a reader
  blocked on the network so the decoder thread can exit.

Opening a stream (reading the headers) also happens off the engine thread, in
`pulse::playback_opener::PlaybackOpener::start`. It first sends `Command::Prepare { play,
track_duration }`, which stops the old track and emits
`EngineEvent::Preparing { duration }` so the UI switches to the new track (title,
cover, a disabled slider at 0 with the catalog duration) before any audio; `SetStreamTrack` arrives when the
decoder is ready. Play and pause that arrive in between are remembered
(`pending_play`) instead of being lost, since there is no track to act on yet.

`Buffering(true)` is not sent on `Prepare` itself. The moment playback was requested is kept
in `prepare_started`. While the track is still being prepared, the loop waits for commands with
`recv_timeout` up to `BUFFERING_AFTER` (250 ms) past that moment and raises buffering only on the
timeout. After a stream is installed, the first starvation continues the count from
`prepare_started` instead of restarting it. So the indicator appears 250 ms after the request no
matter which phase is slow, and an open that finishes sooner (a partly cached file, a fast LAN
server) never flashes it. This is the same rule as the stall indicator during playback.

APE and DSD decoders are tied to `std::fs::File`, so those formats are never
streamed: the app downloads them whole first (`audio_decoder::can_stream`).

## Pause keeps the shared stream running for a while

mpv (7 s), Cog (10 s) and Chromium's media mixer (10 s) all keep the output unit
running with silence on pause and stop it only after an idle timeout. We do the
same. `RELEASE_AFTER_PAUSE` is 1 s: it covers a quick pause/resume, and the
device is freed soon after for exclusive apps and PipeWire's suspend-on-idle.
(The value was 10 s on macOS, where detaching from a device another app was still
playing produced an audible click; Pulse 3.0 is Linux-only, so that case is
gone.)

- The shared output (`cpal_stream`) no longer stops the stream on `pause`. It
  sets `silenced`, so the callback emits zeros without draining the ring buffer.
  `resume` clears it and calls `Stream::play`. `release`
  (`Output::release_paused`) calls `Stream::pause` on a stream that isn't
  playing. There's no own "running" flag: cpal's `play`/`pause` are idempotent
  on CoreAudio, WASAPI and ALSA, and cpal may start a stream on its own at build
  (CoreAudio does; ALSA starts on its start threshold), so a flag of ours would
  be wrong for a fresh stream. The callback reads `silenced` with `Acquire`, so
  once it sees `false` it also sees the fade that `handle_play` set up before
  `resume`.
- The deadline is the engine's own state, not the stream's
  (`RELEASE_AFTER_PAUSE` lives in `engine.rs`). Every pause the engine makes on
  a user or track action goes through `pause_output`, which sets `release_at`;
  `handle_play`, the only way into `Playing`, clears it. While parked with a
  deadline the loop waits with `recv_deadline(release_at)`, without one with a
  plain `recv`, so an idle engine never wakes up. On the deadline it calls
  `Output::pause` and `Output::release_paused` on whatever stream is current.
  A device switch or exclusive toggle on the UI thread can replace the stream
  during the pause (`StreamSwap` re-applies the pause to the new one); since
  the engine doesn't care which stream it is, the new one is released too.
- The end of a track (`TrackEnded`, and a failed stream) also sets
  `release_at`, but doesn't pause: the ring buffer still holds the tail of the
  track, and the app may send the next track into the same running stream
  (`SetLocalTrack` → `install` → `Play` → `handle_play` clears the deadline;
  see "Gapless: keeping the tail"). At the end of the queue nothing comes, and
  the deadline pauses and stops the output once the tail has played (under
  200 ms, the deadline is 1 or 10 s). Before this the output stayed in
  `Playing` feeding zeros forever after the last track. This covers exclusive
  outputs too: `Output::pause` stops them.
- cpal may start a stream on its own at build, so `pause` on a stream that has
  never played (`Idle`) calls `Stream::pause` instead of doing nothing.
  `Output::new` pauses the startup stream right away, and a swap while paused
  (device switch, exclusive toggle, `Output::shutdown`'s fallback) ends in
  `StreamSwap` → `sync_play_state` → `pause`. A swap while playing calls
  `resume` on the new stream instead, so a format change mid-playback doesn't
  stop and restart it.
  - Linux: an ALSA PCM is still PREPARED right after build, where
    `snd_pcm_pause` fails (cpal ignores the error), and cpal's worker then starts
    it by writing. So the immediate `pause` may do nothing. The engine starts
    with `release_at` already set, and by that deadline the PCM is running and
    `pause` works, so the startup stream is released. A swap while paused on
    Linux isn't covered: the engine doesn't know about it.
- `handle_play` starts the fade-in before `resume`, so a pause without a fade
  (gain at unity) can't leak one full-gain callback before the ramp starts.

Within that window a pause and resume are cheap and silent. The stop at the end
can still produce the artifact if another app is playing at that moment, but
it's no longer tied to pressing pause. Exclusive outputs stop at once: hog mode has no
other clients.

## Gapless: keeping the tail

The engine counts a track as ended when it has written the last sample into the
ring buffer, not when it has been heard: up to the 128 ms buffer is still to
play. The app answers `TrackEnded` with `SetLocalTrack` + `Play { fade_in:
false }` right away. `SetLocalTrack` used to go through `reset_track`, whose
`output.clear()` threw that tail away, so every gapless switch cut the end of
the track.

`end_track` now sets `ended_naturally` (only if no fade was running, since a
pause fade left running into the next track would end frozen at zero).
`handle_command` takes the flag for every command, so only a `SetLocalTrack`
that comes straight after the end sees it. That one resets the track state
(`reset_track_state`) but leaves the ring buffer, the fade and `needs_flush`
alone, so the next track is written right behind the tail into the same running
stream. Any other command in between consumes the flag, and the next
`SetLocalTrack` then clears as usual (`Stop` and `Prepare` clear themselves). The
release deadline drops the flag too: by then the tail has played. The engine
can't tell the app's gapless `SetLocalTrack` from a user's click that lands in
the few milliseconds between `TrackEnded` and it; the click then hears the
tail, faded by its `Play { fade_in: true }`, before the chosen track.

A cue track ends at `track_end` inside one file, and the engine used to notice
only after writing a whole batch, so the start of the next track was already
buffered (then cleared). Keeping the buffer would play it twice, so
`trim_to_track_end` cuts each freshly decoded batch at `track_end`. When no
whole frame is left before `track_end` (`frames_until` is 0; float position
rounding can leave a fraction) it ends the track. A batch that is empty on its
own is not an end: symphonia's gapless Vorbis decoder yields an empty first
packet (`test_vorbis_can_yield_an_empty_batch`), and ending on it would skip
every Ogg track. The cue seam is only as exact as the seek: `install` seeks the
file coarsely (`SeekMode::Coarse`, up to a FLAC frame early) and takes
`track_start` as the position, so the cut and the next start can be off by
tens of ms. That predates this change.

Not gapless by design: a remote track that isn't cached (`Prepare` pauses while
it downloads).

### A different sample rate drains the tail first

Keeping the ring buffer only helps while the stream survives. `Output::write`
compares the batch's format against the live stream and rebuilds on any
mismatch, and a rebuild allocates a fresh ring buffer — so the tail that the
engine had just preserved died with the old stream, cutting up to 128 ms off
the end of the outgoing track. That is the gap between "keeps the tail" and
"actually gapless" when consecutive tracks differ in sample rate, bit depth or
channel count.

`Output::drain_pending_tail` closes it. Before a format-driven rebuild, `write`
waits for `pending_samples()` to reach zero, so the tail plays out of the old
stream and only then does the new one open. The transition costs exactly the
tail's duration instead of truncating it.

The wait is bounded by `TAIL_DRAIN_LIMIT` (500 ms, comfortably longer than one
128 ms buffer) and gives up early when the output is parked — a paused device
would never drain, and blocking the engine thread there would hang playback
rather than fix it. The decision is isolated in `drain_step` so it can be tested
without an audio device; `pending_samples` is threaded through the `AudioOutput`
and exclusive `Backend` traits for the same reason.

Device changes (a different default output, an exclusive toggle) still rebuild
without draining: those are deliberate interruptions rather than track seams, and
swapping promptly is the better trade.

## Following the system default device

This lives in `audio_output` (`default_watch.rs` + `Output::write`), but the
engine thread drives it. When no device is pinned (`selected_uid == None`), the
shared stream has to follow the system default output:

- Linux: a long-lived `pactl subscribe` child and an `audio-default-watch`
  thread that reads its output. On a server event the thread compares the
  current default sink with the last one and sets the flag if it changed. The
  child is killed when `Output` is dropped, and the thread then ends on EOF. We
  need this because `resolve_device` pins the stream to the default sink with
  `PIPEWIRE_NODE`, so PipeWire does not move it on its own.

`Output::write` takes the flag once per batch, an atomic swap with no syscalls.
If it is set, the output is shared and nothing is pinned, the shared stream is
rebuilt on the new default. Writes only happen while playing, so a change made
during a pause is picked up by the first batch after resume. Exclusive mode
never follows the default, because hog mode holds the device it grabbed.

When the header device picker is hidden (`show_device_picker = false`, the
default), `Output::set_follow_default(true)` also makes leaving exclusive mode
drop the pin that `set_exclusive(true)` placed. The output then goes back to the
system default instead of staying on the previously exclusive device.

## When no output stream exists

`Output::current` is `None` in two cases: during every deliberate stream swap
(format change, device switch, exclusive toggle, following the default,
disconnect recovery, shutdown), and when no device could be opened at all, for
example at startup with nothing plugged in.

- `Output::is_playing` returns the engine's intent (the last `resume`/`pause`),
  not the state of the current stream. Every swap holds a `StreamSwap` guard,
  and its `Drop` re-applies the intent to whatever stream the swap installed,
  so a Play or Pause pressed during a swap still lands on the new stream. Before this, a `resume` issued while no stream existed was lost: the next
  stream was installed paused, the engine sat in `Playing` feeding an idle
  stream, and its fade-driven pause waited forever for a `FadedOut` that an
  idle stream never sends. Play, Pause and device selection all stayed dead
  until a restart.
- With no stream, `write` tries to open one at the batch's format, at most
  every 2 s. `resume` doesn't open anything: the engine writes right after it,
  and opening at a guessed format would mean a second open once the real format
  arrives. On success the UI gets a `Recovered` event. Between retries `write`
  sleeps 20 ms and returns 0, so the engine loop does not spin.
- The idle reopen never clears a pinned device. If the pinned device exists but
  fails to open (busy, still initializing), it waits for the next retry and
  doesn't fall back. Only a device that can't be resolved at all falls back to
  the default, without dropping the pin.
- Every deliberate swap holds `Output::transition` for its whole duration. The
  idle reopen only `try_lock`s it, so a swap's `None` window is never mistaken
  for "no device". Otherwise the engine thread would open a second stream on
  the same device (EBUSY on single-subdevice cards) and show a false
  "Audio output opened" toast on every track change. Swaps don't nest, so the
  lock is taken only at the entry points.

## Linux without the PipeWire ALSA plugin

The shared path reaches PipeWire only through `pipewire-alsa`: ALSA's `default`
becomes a PipeWire stream, `PIPEWIRE_NODE` picks the sink, and native sample
rate opens the `pipewire:NODE=` PCM. A running PipeWire server does not mean the
plugin is installed. Raspberry Pi OS Trixie, bare Arch, Void (plugin present
but not linked into `/etc/alsa/conf.d`) and NixOS without
`services.pipewire.alsa.enable` all ship without it. Without the plugin ALSA's
`default` is just card 0: often an HDMI port with nothing attached, and device
selection is silently ignored. Playing straight to ALSA cards was tried early
on and failed (crackling, dropouts, EBUSY), so we don't fall back to it.

`audio_output::pipewire_alsa_missing()` means "PipeWire is running, but ALSA has
neither a `pipewire` nor a `pulse` PCM in its hints". The `pipewire-0` socket
alone doesn't prove PipeWire handles audio: on PulseAudio systems (Ubuntu 22.04
and older, Debian 11) PipeWire runs only for screen sharing, and ALSA's
`default` goes through the `pulse` plugin and plays fine. The result is cached per process, because
alsa-lib reads its config once anyway. When it is true, the app does not start:
`pipewire_alsa_gate.rs` opens a window with install commands for common distros
and a Quit button. Flatpak is exempt, because the sandbox has its own ALSA
config and the user can't fix the host from inside it.
`native_mode_available()` also requires the plugin.
