<script lang="ts">
  import { untrack } from "svelte";
  import { longpress } from "./longpress";
  import { openTrackSheet } from "./track_actions.svelte";
  import {
    Remote,
    type AlbumEntry,
    type ArtistEntry,
    type PlaylistTrack,
  } from "./connection.svelte";

  let {
    remote,
    query,
    onartist,
    onalbum,
  }: {
    remote: Remote;
    query: string;
    onartist: (id: number) => void;
    onalbum: (id: number) => void;
  } = $props();

  let artists = $state<ArtistEntry[]>([]);
  let albums = $state<AlbumEntry[]>([]);
  let tracks = $state<PlaylistTrack[]>([]);
  let loaded = $state(false);
  let failed = $state(false);
  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;

  const ARTIST_LIMIT = 4;
  const ALBUM_LIMIT = 6;
  const TRACK_LIMIT = 20;

  $effect(() => {
    remote.libraryRev;
    untrack(() => load());
  });

  async function load() {
    failed = false;
    try {
      const [a, b, t] = await Promise.all([
        fetch("/api/artists"),
        fetch("/api/albums"),
        fetch("/api/tracks"),
      ]);
      if (!a.ok || !b.ok || !t.ok) throw new Error();
      artists = await a.json();
      albums = await b.json();
      tracks = await t.json();
      loaded = true;
    } catch {
      failed = true;
    }
  }

  /// Prefix matches first, then anywhere in the string, then alphabetical, so
  /// typing "jid" puts JID above "Majid Jordan".
  function rank<T>(items: T[], field: (item: T) => string, needle: string): T[] {
    const scored: { item: T; score: number; label: string }[] = [];
    for (const item of items) {
      const label = field(item).toLowerCase();
      const at = label.indexOf(needle);
      if (at === -1) continue;
      scored.push({ item, score: at === 0 ? 0 : 1, label });
    }
    // Numeric collation keeps "Track 2" before "Track 10".
    scored.sort(
      (x, y) =>
        x.score - y.score ||
        x.label.localeCompare(y.label, undefined, { numeric: true }),
    );
    return scored.map((entry) => entry.item);
  }

  const needle = $derived(query.trim().toLowerCase());

  const artistHits = $derived(
    needle === "" ? [] : rank(artists, (a) => a.name, needle),
  );
  const albumHits = $derived(
    needle === ""
      ? []
      : rank(
          albums,
          (a) => `${a.title} ${a.artist ?? ""}`,
          needle,
        ),
  );
  const trackHits = $derived(
    needle === ""
      ? []
      : rank(
          tracks,
          (t) => `${t.title} ${t.artist ?? ""}`,
          needle,
        ),
  );

  const empty = $derived(
    loaded &&
      needle !== "" &&
      artistHits.length === 0 &&
      albumHits.length === 0 &&
      trackHits.length === 0,
  );

  function playTrack(track: PlaylistTrack) {
    remote.playAllTrack(track.id);
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

{#snippet coverFallback()}
  <div class="flex h-full w-full items-center justify-center text-neutral-600">
    <svg class="h-1/2 w-1/2" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
      <path stroke-linecap="round" stroke-linejoin="round" d="M9 18V5l12-2v13" />
      <circle cx="6" cy="18" r="3" />
      <circle cx="18" cy="16" r="3" />
    </svg>
  </div>
{/snippet}

{#snippet sectionTitle(label: string, total: number, shown: number)}
  <div class="flex items-baseline justify-between px-3 pb-1 pt-4">
    <h3 class="text-xs font-semibold uppercase tracking-wider text-neutral-500">{label}</h3>
    {#if total > shown}
      <span class="text-xs text-neutral-600">{shown} of {total}</span>
    {/if}
  </div>
{/snippet}

<div class="relative flex min-h-0 flex-1 flex-col">
  <div class="min-h-0 flex-1 overflow-y-auto px-1 pb-[max(1rem,env(safe-area-inset-bottom))]">
    {#if failed}
      <p class="px-3 py-10 text-center text-sm text-neutral-500">
        Search is unavailable. Check the connection.
      </p>
    {:else if !loaded}
      <p class="px-3 py-10 text-center text-sm text-neutral-500">Loading library…</p>
    {:else if empty}
      <p class="px-3 py-10 text-center text-sm text-neutral-500">
        Nothing matches “{query.trim()}”
      </p>
    {:else}
      {#if artistHits.length > 0}
        {@render sectionTitle("Artists", artistHits.length, Math.min(artistHits.length, ARTIST_LIMIT))}
        {#each artistHits.slice(0, ARTIST_LIMIT) as artist (artist.id)}
          <button
            class="flex w-full items-center gap-3 rounded-xl px-3 py-2 text-left transition active:scale-[0.99] hover:bg-white/5"
            onclick={() => onartist(artist.id)}
          >
            <div class="h-11 w-11 flex-shrink-0 overflow-hidden rounded-full bg-neutral-800">
              {#if artist.cover_ids.length > 0}
                <img
                  src={remote.coverUrlFor(artist.cover_ids[0])}
                  alt=""
                  loading="lazy"
                  class="h-full w-full object-cover"
                />
              {:else}
                {@render coverFallback()}
              {/if}
            </div>
            <span class="min-w-0 flex-1">
              <span class="block truncate text-sm font-medium text-neutral-100">{artist.name}</span>
              <span class="block text-xs text-neutral-500">
                {artist.track_count} {artist.track_count === 1 ? "track" : "tracks"}
              </span>
            </span>
          </button>
        {/each}
      {/if}

      {#if albumHits.length > 0}
        {@render sectionTitle("Albums", albumHits.length, Math.min(albumHits.length, ALBUM_LIMIT))}
        <div class="grid grid-cols-2 gap-x-3 gap-y-3 px-2 py-1 sm:grid-cols-3">
          {#each albumHits.slice(0, ALBUM_LIMIT) as album (album.id)}
            <button
              class="flex flex-col gap-1.5 text-left transition active:scale-[0.98]"
              onclick={() => onalbum(album.id)}
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
                  {[album.artist, album.year].filter(Boolean).join(" · ") || "\u00a0"}
                </span>
              </span>
            </button>
          {/each}
        </div>
      {/if}

      {#if trackHits.length > 0}
        {@render sectionTitle("Tracks", trackHits.length, Math.min(trackHits.length, TRACK_LIMIT))}
        {#each trackHits.slice(0, TRACK_LIMIT) as track (track.id)}
          <div
            class="group flex items-center gap-2 rounded-xl pl-3 pr-2 transition hover:bg-white/5"
            use:longpress={() =>
              openTrackSheet({
                id: track.id,
                title: track.title,
                artist: track.artist,
                coverId: track.cover_id,
                albumId: null,
              })}
          >
            <button
              class="flex min-w-0 flex-1 items-center gap-3 py-2 text-left transition active:scale-[0.99]"
              onclick={() => playTrack(track)}
            >
              <div class="h-11 w-11 flex-shrink-0 overflow-hidden rounded-md bg-neutral-800">
                {#if track.cover_id !== null}
                  <img
                    src={remote.coverUrlFor(track.cover_id)}
                    alt=""
                    loading="lazy"
                    class="h-full w-full object-cover"
                  />
                {:else}
                  {@render coverFallback()}
                {/if}
              </div>
              <span class="min-w-0 flex-1">
                <span class="block truncate text-sm text-neutral-100">{track.title}</span>
                {#if track.artist}
                  <span class="block truncate text-xs text-neutral-500">{track.artist}</span>
                {/if}
              </span>
            </button>
            <button
              class="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-full text-neutral-400 transition active:scale-90 hover:bg-white/10 hover:text-white"
              aria-label={`Queue ${track.title}`}
              onclick={() => queueTrack(track)}
            >
              <svg class="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
                <path d="M4 6h9M4 12h9M4 18h6" />
                <path d="M18 9v6M15 12h6" />
              </svg>
            </button>
          </div>
        {/each}
      {/if}
    {/if}
  </div>

  {#if toast}
    <div class="pointer-events-none absolute inset-x-0 bottom-3 z-50 flex justify-center px-3">
      <span class="max-w-full truncate rounded-full bg-emerald-400 px-4 py-2 text-sm font-medium text-neutral-950 shadow-lg">
        {toast}
      </span>
    </div>
  {/if}
</div>
