<script lang="ts">
  import { fade, fly } from "svelte/transition";
  import { Remote } from "./connection.svelte";
  import { closeTrackSheet, trackSheet } from "./track_actions.svelte";

  let {
    remote,
    onartist,
    onalbum,
  }: {
    remote: Remote;
    onartist: (name: string) => void;
    onalbum: (id: number) => void;
  } = $props();

  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  const track = $derived(trackSheet.track);

  function showToast(message: string) {
    toast = message;
    if (toastTimer !== null) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      toast = null;
      toastTimer = null;
    }, 1600);
  }

  function play() {
    if (track === null) return;
    remote.playAllTrack(track.id);
    closeTrackSheet();
  }

  function playNext() {
    if (track === null) return;
    remote.playNextTrack(track.id);
    showToast(`Playing next: ${track.title}`);
    closeTrackSheet();
  }

  function queue() {
    if (track === null) return;
    remote.queueAllTrack(track.id);
    showToast(`Queued: ${track.title}`);
    closeTrackSheet();
  }
</script>

{#if track !== null}
  <button
    class="absolute inset-0 z-50 cursor-default bg-black/60"
    aria-label="Close actions"
    transition:fade={{ duration: 150 }}
    onclick={closeTrackSheet}
  ></button>
  <section
    class="absolute inset-x-0 bottom-0 z-50 flex flex-col rounded-t-3xl border-t border-white/10 bg-neutral-900 pb-[max(0.75rem,env(safe-area-inset-bottom))] shadow-2xl"
    transition:fly={{ y: 400, duration: 220 }}
  >
    <div class="flex items-center gap-3 px-5 pb-2 pt-4">
      <div class="h-12 w-12 flex-shrink-0 overflow-hidden rounded-lg bg-neutral-800">
        {#if track.coverId !== null}
          <img src={remote.coverUrlFor(track.coverId)} alt="" class="h-full w-full object-cover" />
        {:else}
          <div class="flex h-full w-full items-center justify-center text-neutral-600">
            <svg class="h-1/2 w-1/2" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
              <path stroke-linecap="round" stroke-linejoin="round" d="M9 18V5l12-2v13" />
              <circle cx="6" cy="18" r="3" />
              <circle cx="18" cy="16" r="3" />
            </svg>
          </div>
        {/if}
      </div>
      <div class="min-w-0 flex-1">
        <p class="truncate text-sm font-semibold">{track.title}</p>
        {#if track.artist}
          <p class="truncate text-xs text-neutral-400">{track.artist}</p>
        {/if}
      </div>
      <button
        class="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-full text-neutral-400 transition active:scale-90 hover:bg-white/10 hover:text-white"
        aria-label="Close"
        onclick={closeTrackSheet}
      >
        <svg class="h-5 w-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <path d="M6 6l12 12M18 6L6 18" />
        </svg>
      </button>
    </div>

    <div class="px-2 pt-1">
      <button
        class="flex w-full items-center gap-3 rounded-xl px-3 py-3 text-left transition active:scale-[0.99] hover:bg-white/5"
        onclick={play}
      >
        <svg class="h-5 w-5 flex-shrink-0 text-neutral-300" viewBox="0 0 24 24" fill="currentColor">
          <path d="M8 5v14l11-7z" />
        </svg>
        <span class="text-sm text-neutral-100">Play now</span>
      </button>
      <button
        class="flex w-full items-center gap-3 rounded-xl px-3 py-3 text-left transition active:scale-[0.99] hover:bg-white/5"
        onclick={playNext}
      >
        <svg class="h-5 w-5 flex-shrink-0 text-neutral-300" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M4 6h9M4 12h9M4 18h6" />
          <path d="M17 15V6M14 9l3-3 3 3" />
        </svg>
        <span class="text-sm text-neutral-100">Play next</span>
      </button>
      <button
        class="flex w-full items-center gap-3 rounded-xl px-3 py-3 text-left transition active:scale-[0.99] hover:bg-white/5"
        onclick={queue}
      >
        <svg class="h-5 w-5 flex-shrink-0 text-neutral-300" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <path d="M4 6h9M4 12h9M4 18h6" />
          <path d="M18 9v6M15 12h6" />
        </svg>
        <span class="text-sm text-neutral-100">Add to queue</span>
      </button>
      {#if track.artist}
        <button
          class="flex w-full items-center gap-3 rounded-xl px-3 py-3 text-left transition active:scale-[0.99] hover:bg-white/5"
          onclick={() => {
            const name = track?.artist;
            if (!name) return;
            closeTrackSheet();
            onartist(name);
          }}
        >
          <svg class="h-5 w-5 flex-shrink-0 text-neutral-300" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="12" cy="8" r="4" />
            <path d="M4 21a8 8 0 0 1 16 0" />
          </svg>
          <span class="text-sm text-neutral-100">Go to artist</span>
        </button>
      {/if}
      {#if track.albumId !== null}
        <button
          class="flex w-full items-center gap-3 rounded-xl px-3 py-3 text-left transition active:scale-[0.99] hover:bg-white/5"
          onclick={() => {
            const id = track?.albumId;
            if (id == null) return;
            closeTrackSheet();
            onalbum(id);
          }}
        >
          <svg class="h-5 w-5 flex-shrink-0 text-neutral-300" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="12" cy="12" r="9" />
            <circle cx="12" cy="12" r="2.5" />
          </svg>
          <span class="text-sm text-neutral-100">Go to album</span>
        </button>
      {/if}
    </div>
  </section>

  {#if toast}
    <div
      class="pointer-events-none absolute inset-x-0 z-50 flex justify-center px-4"
      style="bottom: calc(env(safe-area-inset-bottom) + 4.5rem)"
      transition:fade={{ duration: 150 }}
    >
      <span class="max-w-full truncate rounded-full bg-emerald-400 px-4 py-2 text-sm font-medium text-neutral-950 shadow-lg">
        {toast}
      </span>
    </div>
  {/if}
{/if}
