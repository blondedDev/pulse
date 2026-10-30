<script lang="ts">
  import { untrack } from "svelte";
  import { fade } from "svelte/transition";
  import { Remote, type PlaylistTrack } from "./connection.svelte";
  import TrackRow from "./TrackRow.svelte";

  /// What was played recently, newest first. The desktop player stamps each
  /// track as it starts, so this needs no state of its own on the phone: the
  /// list is refetched whenever the library revision moves, which is exactly
  /// when a new track has been recorded.
  let { remote }: { remote: Remote } = $props();

  let tracks = $state<PlaylistTrack[] | null>(null);
  let loading = $state(false);
  let error = $state(false);
  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  let gen = 0;

  $effect(() => {
    remote.libraryRev;
    untrack(() => load());
  });

  async function load() {
    const mine = ++gen;
    loading = true;
    error = false;
    try {
      const r = await fetch("/api/recent");
      if (!r.ok) throw new Error();
      const fetched: PlaylistTrack[] = await r.json();
      if (mine !== gen) return;
      tracks = fetched;
    } catch {
      if (mine === gen) error = true;
    } finally {
      if (mine === gen) loading = false;
    }
  }

  function playTrack(track: PlaylistTrack) {
    remote.playRecentTrack(track.id);
  }

  function queueTrack(track: PlaylistTrack) {
    remote.queueAllTrack(track.id);
    showToast(`Queued: ${track.title}`);
  }

  function showToast(message: string) {
    toast = message;
    if (toastTimer !== null) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      toast = null;
      toastTimer = null;
    }, 1800);
  }
</script>

<div class="relative flex min-h-0 flex-1 flex-col">
  <div class="min-h-0 flex-1 overflow-y-auto px-2 pb-[max(1rem,env(safe-area-inset-bottom))]">
    {#if error}
      <div class="flex flex-col items-center gap-3 px-3 py-10">
        <p class="text-center text-sm text-neutral-500">Failed to load. Check the connection.</p>
        <button
          class="rounded-full bg-white/10 px-4 py-1.5 text-sm text-neutral-200 transition active:scale-95 hover:bg-white/20"
          onclick={load}
        >
          Retry
        </button>
      </div>
    {:else if loading && tracks === null}
      <p class="px-3 py-10 text-center text-sm text-neutral-500">Loading…</p>
    {:else if tracks !== null}
      {#if tracks.length === 0}
        <p class="px-3 py-10 text-center text-sm text-neutral-500">Nothing played yet</p>
      {:else}
        {#each tracks as track (track.id)}
          <TrackRow {remote} {track} onplay={() => playTrack(track)} onqueue={() => queueTrack(track)} />
        {/each}
      {/if}
    {/if}
  </div>

  {#if toast}
    <div
      class="pointer-events-none absolute inset-x-0 bottom-3 z-50 flex justify-center px-3"
      transition:fade={{ duration: 150 }}
    >
      <span class="max-w-full truncate rounded-full bg-emerald-400 px-4 py-2 text-sm font-medium text-neutral-950 shadow-lg">
        {toast}
      </span>
    </div>
  {/if}
</div>
