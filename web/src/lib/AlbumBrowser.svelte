<script lang="ts">
  import { untrack } from "svelte";
  import { fade } from "svelte/transition";
  import {
    Remote,
    type AlbumDetail,
    type AlbumEntry,
  } from "./connection.svelte";
  import TrackRow from "./TrackRow.svelte";

  let {
    remote,
    inDetail = $bindable(false),
    detailName = $bindable(""),
  }: {
    remote: Remote;
    inDetail?: boolean;
    detailName?: string;
  } = $props();

  let albums = $state<AlbumEntry[] | null>(null);
  let detail = $state<AlbumDetail | null>(null);
  let viewing = $state(false);
  let currentId = $state<number | null>(null);
  let pendingName = $state("");
  let loading = $state(false);
  let error = $state(false);
  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  let listEl = $state<HTMLElement | null>(null);
  let detailGen = 0;

  $effect(() => {
    remote.libraryRev;
    untrack(() => {
      loadAlbums();
      if (viewing && currentId !== null) loadDetail(currentId);
    });
  });
  $effect(() => {
    inDetail = viewing;
  });
  $effect(() => {
    detailName = detail !== null ? detail.title : pendingName;
  });

  async function loadAlbums() {
    error = false;
    try {
      const r = await fetch("/api/albums");
      if (!r.ok) throw new Error();
      albums = await r.json();
    } catch {
      if (!viewing) error = true;
    }
  }

  async function loadDetail(id: number) {
    const gen = ++detailGen;
    loading = true;
    error = false;
    try {
      const r = await fetch(`/api/album?id=${id}`);
      if (!r.ok) throw new Error();
      const fetched: AlbumDetail = await r.json();
      if (gen !== detailGen) return;
      detail = fetched;
    } catch {
      if (gen === detailGen) error = true;
    } finally {
      if (gen === detailGen) loading = false;
    }
  }

  export function openAlbum(album: AlbumEntry) {
    detail = null;
    currentId = album.id;
    pendingName = album.title;
    viewing = true;
    loadDetail(album.id);
    listEl?.scrollTo(0, 0);
  }

  /// Open by id, for search results that only carry the id.
  export function openById(id: number) {
    const known = albums?.find((album) => album.id === id);
    openAlbum(known ?? { id, title: "", artist: null, year: null, cover_id: null, track_count: 0 });
  }

  export function goBack() {
    detailGen++;
    detail = null;
    viewing = false;
    loading = false;
    error = false;
    listEl?.scrollTo(0, 0);
    if (albums === null || albums.length === 0) loadAlbums();
  }

  function retry() {
    if (viewing && currentId !== null) loadDetail(currentId);
    else loadAlbums();
  }

  function playAlbum() {
    if (detail === null) return;
    remote.playAlbum(detail.id);
  }

  function queueAlbum() {
    if (detail === null) return;
    remote.queueAlbum(detail.id);
    showToast(`Queued: ${detail.title || "album"}`);
  }

  function playTrack(trackId: number) {
    if (detail === null) return;
    remote.playAlbumTrack(detail.id, trackId);
  }

  function queueTrack(trackId: number, title: string) {
    if (detail === null) return;
    remote.queueAlbumTrack(detail.id, trackId);
    showToast(`Queued: ${title}`);
  }

  function showToast(message: string) {
    toast = message;
    if (toastTimer !== null) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => {
      toast = null;
      toastTimer = null;
    }, 1800);
  }

  function subtitle(album: { artist: string | null; year: number | null }): string {
    return [album.artist, album.year].filter(Boolean).join(" · ");
  }
</script>

{#snippet coverFallback()}
  <div class="flex h-full w-full items-center justify-center text-neutral-600">
    <svg class="h-1/3 w-1/3" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
      <path stroke-linecap="round" stroke-linejoin="round" d="M9 18V5l12-2v13" />
      <circle cx="6" cy="18" r="3" />
      <circle cx="18" cy="16" r="3" />
    </svg>
  </div>
{/snippet}

<div class="relative flex min-h-0 flex-1 flex-col">
  <div bind:this={listEl} class="min-h-0 flex-1 overflow-y-auto px-3 pb-[max(1rem,env(safe-area-inset-bottom))]">
    {#if error}
      <div class="flex flex-col items-center gap-3 px-3 py-10">
        <p class="text-center text-sm text-neutral-500">Failed to load. Check the connection.</p>
        <button
          class="rounded-full bg-white/10 px-4 py-1.5 text-sm text-neutral-200 transition active:scale-95 hover:bg-white/20"
          onclick={retry}
        >
          Retry
        </button>
      </div>
    {:else if !viewing}
      {#if albums === null}
        <p class="px-3 py-10 text-center text-sm text-neutral-500">Loading…</p>
      {:else if albums.length === 0}
        <p class="px-3 py-10 text-center text-sm text-neutral-500">No albums found</p>
      {:else}
        <div class="grid grid-cols-2 gap-x-3 gap-y-4 py-2 sm:grid-cols-3">
          {#each albums as album (album.id)}
            <button
              class="group flex flex-col gap-1.5 text-left transition active:scale-[0.98]"
              onclick={() => openAlbum(album)}
            >
              <div class="aspect-square w-full overflow-hidden rounded-xl bg-neutral-800">
                {#if album.cover_id !== null}
                  <img
                    src={remote.coverUrlFor(album.cover_id)}
                    alt=""
                    loading="lazy"
                    class="h-full w-full object-cover"
                  />
                {:else}
                  {@render coverFallback()}
                {/if}
              </div>
              <span class="min-w-0">
                <span class="block truncate text-sm font-medium text-neutral-100">
                  {album.title || "No metadata"}
                </span>
                <span class="block truncate text-xs text-neutral-500">
                  {subtitle(album) || "\u00a0"}
                </span>
              </span>
            </button>
          {/each}
        </div>
      {/if}
    {:else if loading && detail === null}
      <p class="px-3 py-10 text-center text-sm text-neutral-500">Loading…</p>
    {:else if detail !== null}
      <div class="flex items-center gap-3 py-3">
        <div class="h-20 w-20 flex-shrink-0 overflow-hidden rounded-lg bg-neutral-800">
          {#if detail.cover_id !== null}
            <img src={remote.coverUrlFor(detail.cover_id)} alt="" class="h-full w-full object-cover" />
          {:else}
            {@render coverFallback()}
          {/if}
        </div>
        <div class="min-w-0 flex-1">
          <p class="truncate text-sm font-semibold">{detail.title || "No metadata"}</p>
          <p class="truncate text-xs text-neutral-500">{subtitle(detail)}</p>
          <p class="mt-0.5 text-xs text-neutral-600">{detail.tracks.length} tracks</p>
        </div>
      </div>
      <div class="flex items-center gap-2 pb-2">
        <button
          class="flex h-10 flex-1 items-center justify-center gap-2 rounded-full bg-white text-sm font-semibold text-neutral-900 transition active:scale-[0.98] disabled:opacity-40"
          disabled={detail.tracks.length === 0}
          onclick={playAlbum}
        >
          <svg class="h-4 w-4" viewBox="0 0 24 24" fill="currentColor">
            <path d="M8 5v14l11-7z" />
          </svg>
          Play
        </button>
        <button
          class="flex h-10 flex-1 items-center justify-center gap-2 rounded-full bg-white/10 text-sm font-medium text-neutral-200 transition active:scale-[0.98] hover:bg-white/20 disabled:opacity-40"
          disabled={detail.tracks.length === 0}
          onclick={queueAlbum}
        >
          <svg class="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <path d="M4 6h9M4 12h9M4 18h6" />
            <path d="M18 9v6M15 12h6" />
          </svg>
          Queue
        </button>
      </div>
      {#if detail.tracks.length === 0}
        <p class="px-3 py-10 text-center text-sm text-neutral-500">No tracks in this album</p>
      {/if}
      {#each detail.tracks as track (track.id)}
        <TrackRow
          {remote}
          {track}
          onplay={() => playTrack(track.id)}
          onqueue={() => queueTrack(track.id, track.title)}
          albumId={detail.id}
        />
      {/each}
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
