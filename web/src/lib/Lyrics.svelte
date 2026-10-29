<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import { Remote, type LyricLine, type TrackLyrics } from "./connection.svelte";
  import { swipe } from "./swipe";

  let { remote, onclose }: { remote: Remote; onclose: () => void } = $props();

  let lines = $state<LyricLine[]>([]);
  let synced = $state(false);
  let loading = $state(true);
  /// The desktop was asked for lyrics and came back with nothing.
  let missing = $state(false);
  let fetching = $state(false);
  /// Whether the view keeps the sung line in the middle. Tapping or scrolling
  /// turns it off for a few seconds so the page does not fight the finger.
  let follow = $state(true);
  let body = $state<HTMLDivElement | null>(null);

  const trackId = $derived(remote.currentTrackId);

  // Non-reactive bookkeeping: which request is current, whether a fetch the
  // user started is still in flight, and the last track we loaded.
  let seq = 0;
  let pending = false;
  let loadedFor: number | null | undefined = undefined;
  let followTimer: ReturnType<typeof setTimeout> | null = null;
  let fetchTimer: ReturnType<typeof setTimeout> | null = null;

  $effect(() => {
    const id = trackId;
    // A lyrics fetch reports back as a library bump, so any bump can mean this
    // track just gained lyrics.
    remote.libraryRev;
    const mine = ++seq;
    // A bump is also how a search answers, so only a new track cancels one.
    if (loadedFor !== id) {
      loadedFor = id;
      fetching = false;
      pending = false;
    }
    lines = [];
    synced = false;
    missing = false;
    loading = id !== null;
    // Fresh data for the sheet: follow the song again, even if the user had
    // just scrolled or tapped (a fetch comes back as one of these bumps).
    follow = true;
    if (followTimer !== null) {
      clearTimeout(followTimer);
      followTimer = null;
    }
    if (id === null) {
      fetching = false;
      pending = false;
      return;
    }
    fetch(`/api/lyrics?track_id=${id}`)
      .then((r) => r.json())
      .then((data: TrackLyrics) => {
        if (mine !== seq) return;
        lines = data.lines ?? [];
        synced = data.synced ?? false;
        loading = false;
        if (pending) {
          pending = false;
          fetching = false;
          missing = lines.length === 0;
        }
      })
      .catch(() => {
        if (mine !== seq) return;
        loading = false;
        if (pending) {
          pending = false;
          fetching = false;
          missing = true;
        }
      });
  });

  /// The line being sung: the last one whose time has passed.
  const active = $derived.by(() => {
    if (!synced) return -1;
    const now = remote.positionMs;
    for (let i = lines.length - 1; i >= 0; i -= 1) {
      const time = lines[i].time_ms;
      if (time !== null && time <= now) return i;
    }
    return -1;
  });

  $effect(() => {
    const index = active;
    if (!follow || index < 0) return;
    const el = body?.querySelector<HTMLElement>(`[data-line="${index}"]`);
    el?.scrollIntoView({ block: "center", behavior: "smooth" });
  });

  function pauseFollow() {
    follow = false;
    if (followTimer !== null) clearTimeout(followTimer);
    followTimer = setTimeout(() => {
      followTimer = null;
      follow = true;
    }, 4000);
  }

  function seekToLine(line: LyricLine) {
    if (line.time_ms === null) return;
    remote.seekTo(line.time_ms);
    // Catch up with the new position straight away instead of waiting out the
    // pause the tap just started.
    if (followTimer !== null) clearTimeout(followTimer);
    followTimer = null;
    follow = true;
  }

  function fetchOnline() {
    const id = trackId;
    if (id === null || fetching) return;
    fetching = true;
    missing = false;
    pending = true;
    remote.fetchLyrics(id);
    if (fetchTimer !== null) clearTimeout(fetchTimer);
    fetchTimer = setTimeout(() => {
      fetchTimer = null;
      if (!pending) return;
      // No bump came back, so nothing was stored.
      pending = false;
      fetching = false;
      missing = true;
    }, 15000);
  }

  const padded = $derived(synced && lines.length > 0);
</script>

<button
  class="absolute inset-0 z-40 cursor-default bg-black/60"
  aria-label="Close lyrics"
  transition:fade={{ duration: 150 }}
  onclick={onclose}
></button>
<section
  class="absolute inset-x-0 bottom-0 z-40 flex h-[88dvh] flex-col rounded-t-3xl border-t border-white/10 bg-neutral-900 shadow-2xl"
  transition:fly={{ y: 600, duration: 250 }}
>
  <div
    class="flex flex-shrink-0 items-center gap-3 px-5 pb-2 pt-4"
    use:swipe={{ ondown: onclose, threshold: 70 }}
  >
    <div class="min-w-0 flex-1">
      <h2 class="truncate text-sm font-semibold tracking-wide text-neutral-200">
        {remote.title ?? "Lyrics"}
      </h2>
      {#if remote.artist}
        <p class="truncate text-xs text-neutral-500">{remote.artist}</p>
      {/if}
    </div>
    {#if lines.length > 0 && !synced}
      <span class="flex-shrink-0 rounded-full bg-white/5 px-2 py-0.5 text-[11px] text-neutral-400">
        Not synced
      </span>
    {/if}
    <button
      class="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-full text-neutral-400 transition active:scale-90 hover:text-white"
      aria-label="Close"
      onclick={onclose}
    >
      <svg class="h-5 w-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
        <path d="M6 6l12 12M18 6L6 18" />
      </svg>
    </button>
  </div>

  <div
    bind:this={body}
    role="region"
    aria-label="Lyrics"
    class={`mx-auto min-h-0 w-full max-w-2xl flex-1 overflow-y-auto overscroll-contain px-6 ${
      padded
        ? "pt-[30vh] pb-[40vh]"
        : lines.length === 0
          ? "flex items-center justify-center"
          : "py-6"
    }`}
    onpointerdown={pauseFollow}
    onwheel={pauseFollow}
  >
    {#if lines.length > 0}
      {#if synced}
        {#each lines as line, i (i)}
          <button
            data-line={i}
            class={`block w-full py-1.5 text-left text-lg leading-snug transition-colors ${
              i === active
                ? "font-semibold text-white"
                : "text-neutral-500 hover:text-neutral-300"
            }`}
            onclick={() => seekToLine(line)}
          >
            {line.text || "\u00a0"}
          </button>
        {/each}
      {:else}
        {#each lines as line, i (i)}
          <p data-line={i} class="py-0.5 text-base leading-relaxed text-neutral-300">
            {line.text || "\u00a0"}
          </p>
        {/each}
      {/if}
    {:else if loading}
      <div class="flex justify-center py-10 text-neutral-500">
        <svg class="h-6 w-6 animate-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <path d="M12 3a9 9 0 1 0 9 9" />
        </svg>
      </div>
    {:else}
      <div class="flex flex-col items-center gap-4 px-4 py-12 text-center">
        <p class="text-sm text-neutral-400">
          {#if !remote.hasTrack}
            Nothing playing
          {:else if missing}
            Nothing found online
          {:else}
            No lyrics for this track
          {/if}
        </p>
        {#if remote.hasTrack}
          <button
            class="flex items-center gap-2 rounded-full bg-white/10 px-4 py-2 text-sm font-medium text-neutral-100 transition active:scale-95 hover:bg-white/15 disabled:opacity-60"
            disabled={fetching || trackId === null}
            onclick={fetchOnline}
          >
            {#if fetching}
              <svg class="h-4 w-4 animate-spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
                <path d="M12 3a9 9 0 1 0 9 9" />
              </svg>
              Searching
            {:else}
              <svg class="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="11" cy="11" r="7" />
                <path d="M20 20l-3.5-3.5" />
              </svg>
              {missing ? "Try again" : "Search online"}
            {/if}
          </button>
        {/if}
      </div>
    {/if}
  </div>
</section>
