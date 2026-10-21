<script lang="ts">
  import { untrack } from "svelte";
  import { fade } from "svelte/transition";
  import {
    Remote,
    type FolderDetail,
    type FolderEntry,
    type PlaylistTrack,
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

  /// Folders opened so far. Empty means the library roots are on screen.
  let stack = $state<FolderEntry[]>([]);
  let entries = $state<FolderEntry[] | null>(null);
  let detail = $state<FolderDetail | null>(null);
  let loading = $state(false);
  let error = $state(false);
  let toast = $state<string | null>(null);
  let toastTimer: ReturnType<typeof setTimeout> | null = null;
  let listEl = $state<HTMLElement | null>(null);
  let shown = $state(200);
  let gen = 0;

  const PAGE = 200;

  const current = $derived(stack.length > 0 ? stack[stack.length - 1] : null);

  $effect(() => {
    remote.libraryRev;
    untrack(() => {
      if (current !== null) reload(current);
      else loadRoots();
    });
  });
  $effect(() => {
    inDetail = stack.length > 0;
  });
  $effect(() => {
    detailName = current !== null ? current.name : "";
  });

  async function loadRoots() {
    const mine = ++gen;
    error = false;
    try {
      const r = await fetch("/api/folders");
      if (!r.ok) throw new Error();
      const roots: FolderEntry[] = await r.json();
      if (mine !== gen) return;
      entries = roots;
      detail = null;
    } catch {
      if (mine === gen) error = true;
    }
  }

  /// Open a folder: its subfolders first, and its tracks only if it has none.
  ///
  /// Deliberately lazy — a folder can hold a thousand tracks, and the phone
  /// should not download them to show a list of five subdirectories.
  async function reload(folder: FolderEntry) {
    const mine = ++gen;
    loading = true;
    error = false;
    detail = null;
    shown = PAGE;
    try {
      const r = await fetch(`/api/folders?parent=${encodeURIComponent(folder.path)}`);
      if (!r.ok) throw new Error();
      const children: FolderEntry[] = await r.json();
      if (mine !== gen) return;
      entries = children;
      if (children.length === 0) await loadTracks(folder.path, mine);
    } catch {
      if (mine === gen) error = true;
    } finally {
      if (mine === gen) loading = false;
    }
  }

  async function loadTracks(path: string, mine: number) {
    try {
      const r = await fetch(`/api/folder?path=${encodeURIComponent(path)}`);
      if (!r.ok) throw new Error();
      const fetched: FolderDetail = await r.json();
      if (mine !== gen) return;
      detail = fetched;
    } catch {
      if (mine === gen) error = true;
    }
  }

  function openFolder(folder: FolderEntry) {
    stack = [...stack, folder];
    entries = null;
    inDetail = true;
    detailName = folder.name;
    reload(folder);
    listEl?.scrollTo(0, 0);
  }

  export function goBack() {
    gen++;
    listEl?.scrollTo(0, 0);
    error = false;
    loading = false;
    shown = PAGE;
    stack = stack.slice(0, -1);
    const parent = stack.length > 0 ? stack[stack.length - 1] : null;
    if (parent !== null) {
      reload(parent);
    } else {
      detail = null;
      loadRoots();
    }
  }

  function showTracks() {
    if (current === null) return;
    const mine = ++gen;
    loading = true;
    entries = [];
    shown = PAGE;
    loadTracks(current.path, mine).finally(() => {
      if (mine === gen) loading = false;
    });
  }

  function retry() {
    if (current !== null) reload(current);
    else loadRoots();
  }

  function playTrack(track: PlaylistTrack) {
    if (current === null) return;
    remote.playFolderTrack(current.path, track.id);
  }

  function queueTrack(track: PlaylistTrack) {
    if (current === null) return;
    remote.queueFolderTrack(current.path, track.id);
    showToast(`Queued: ${track.title}`);
  }

  function queueAll() {
    if (current === null) return;
    remote.queueFolder(current.path);
    showToast(`Queued: ${current.name}`);
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
  <div bind:this={listEl} class="min-h-0 flex-1 overflow-y-auto px-2 pb-[max(1rem,env(safe-area-inset-bottom))]">
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
    {:else if loading && detail === null && (entries === null || entries.length === 0)}
      <!-- A leaf folder loads its tracks after learning it has no children;
           without this the list reads "empty" for that gap. -->
      <p class="px-3 py-10 text-center text-sm text-neutral-500">Loading…</p>
    {:else if detail !== null}
      <div class="flex items-center justify-end gap-2 px-3 py-2">
        <button
          class="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-full text-neutral-400 transition active:scale-90 hover:bg-white/10 hover:text-white"
          aria-label="Add this folder to the queue"
          onclick={queueAll}
        >
          <svg class="h-5 w-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <path d="M4 6h9M4 12h9M4 18h6" />
            <path d="M18 9v6M15 12h6" />
          </svg>
        </button>
      </div>
      {#if detail.tracks.length === 0}
        <p class="px-3 py-10 text-center text-sm text-neutral-500">No tracks in this folder</p>
      {:else}
        {#each detail.tracks.slice(0, shown) as track (track.id)}
          <TrackRow {remote} {track} onplay={() => playTrack(track)} onqueue={() => queueTrack(track)} />
        {/each}
        {#if detail.tracks.length > shown}
          <button
            class="w-full px-3 py-4 text-center text-sm text-neutral-400 transition hover:text-white"
            onclick={() => (shown += PAGE)}
          >
            Show more ({detail.tracks.length - shown} left)
          </button>
        {/if}
      {/if}
    {:else if entries !== null}
      {#if entries.length === 0}
        <p class="px-3 py-10 text-center text-sm text-neutral-500">
          {current === null ? "No folders" : "This folder is empty"}
        </p>
      {:else}
        {#each entries as folder (folder.path)}
          <button
            class="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left transition active:scale-[0.99] hover:bg-white/5"
            onclick={() => openFolder(folder)}
          >
            <svg class="h-5 w-5 flex-shrink-0 text-neutral-500" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
              <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
            </svg>
            <span class="min-w-0 flex-1 truncate text-sm text-neutral-200">{folder.name}</span>
            <span class="flex-shrink-0 text-xs text-neutral-500">{folder.track_count}</span>
          </button>
        {/each}
        {#if current !== null && current.track_count > 0}
          <button
            class="flex w-full items-center gap-3 rounded-xl px-3 py-2.5 text-left transition active:scale-[0.99] hover:bg-white/5"
            onclick={showTracks}
          >
            <svg class="h-5 w-5 flex-shrink-0 text-emerald-400" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
              <path d="M9 18V5l12-2v13" />
              <circle cx="6" cy="18" r="3" />
              <circle cx="18" cy="16" r="3" />
            </svg>
            <span class="min-w-0 flex-1 truncate text-sm text-neutral-200">All tracks in this folder</span>
            <span class="flex-shrink-0 text-xs text-neutral-500">{current.track_count}</span>
          </button>
        {/if}
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
