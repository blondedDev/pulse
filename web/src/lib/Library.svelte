<script lang="ts">
  import { fade } from "svelte/transition";
  import { Remote } from "./connection.svelte";
  import ArtistBrowser from "./ArtistBrowser.svelte";
  import PlaylistBrowser from "./PlaylistBrowser.svelte";
  import FolderBrowser from "./FolderBrowser.svelte";
  import AllTracks from "./AllTracks.svelte";
  import AlbumBrowser from "./AlbumBrowser.svelte";
  import Search from "./Search.svelte";

  let {
    remote,
    onclose,
    target = null,
    ontarget = () => {},
  }: {
    remote: Remote;
    onclose: () => void;
    target?: { kind: "artist"; name: string } | { kind: "album"; id: number } | null;
    ontarget?: () => void;
  } = $props();

  let tab = $state<"artists" | "albums" | "folders" | "all" | "playlists">("artists");
  let searching = $state(false);
  let query = $state("");
  let searchEl = $state<HTMLInputElement | null>(null);

  let artistBrowser = $state<ArtistBrowser | null>(null);
  let aInDetail = $state(false);
  let aName = $state("");
  let aHasPartial = $state(false);
  let aFull = $state(false);

  let plBrowser = $state<PlaylistBrowser | null>(null);
  let plInDetail = $state(false);
  let plName = $state("");

  let folderBrowser = $state<FolderBrowser | null>(null);
  let fInDetail = $state(false);
  let fName = $state("");

  let albumBrowser = $state<AlbumBrowser | null>(null);
  let albInDetail = $state(false);
  let albName = $state("");

  const activeInDetail = $derived(
    tab === "artists"
      ? aInDetail
      : tab === "albums"
        ? albInDetail
        : tab === "folders"
          ? fInDetail
          : tab === "playlists"
            ? plInDetail
            : false,
  );

  const tabTitle = $derived(
    tab === "artists"
      ? "Artists"
      : tab === "albums"
        ? "Albums"
        : tab === "folders"
          ? "Folders"
          : tab === "all"
            ? "All tracks"
            : "Playlists",
  );

  const title = $derived(
    tab === "artists"
      ? aInDetail
        ? aName
        : "Artists"
      : tab === "albums"
        ? albInDetail
          ? albName
          : "Albums"
        : tab === "folders"
          ? fInDetail
            ? fName
            : "Folders"
          : tab === "playlists"
            ? plInDetail
              ? plName
              : "Playlists"
            : tabTitle,
  );

  // The track sheet asked for a specific artist or album. Wait a tick so the
  // browser components are mounted, then hand the request over.
  $effect(() => {
    const request = target;
    if (request === null) return;
    closeSearch();
    if (request.kind === "artist") {
      tab = "artists";
      queueMicrotask(() => artistBrowser?.openByName(request.name));
    } else {
      tab = "albums";
      queueMicrotask(() => albumBrowser?.openById(request.id));
    }
    ontarget();
  });

  function openSearch() {
    searching = true;
    query = "";
    // The field is rendered on the next tick; focus it so the keyboard comes up.
    queueMicrotask(() => searchEl?.focus());
  }

  function closeSearch() {
    searching = false;
    query = "";
  }

  function openArtistFromSearch(id: number) {
    closeSearch();
    tab = "artists";
    artistBrowser?.openById(id);
  }

  function openAlbumFromSearch(id: number) {
    closeSearch();
    tab = "albums";
    albumBrowser?.openById(id);
  }

  function goBack() {
    if (tab === "artists") artistBrowser?.goBack();
    else if (tab === "albums") albumBrowser?.goBack();
    else if (tab === "folders") folderBrowser?.goBack();
    else if (tab === "playlists") plBrowser?.goBack();
  }

  const tabs = [
    { id: "artists", label: "Artists" },
    { id: "albums", label: "Albums" },
    { id: "folders", label: "Folders" },
    { id: "all", label: "All tracks" },
    { id: "playlists", label: "Playlists" },
  ] as const;
</script>

<div
  class="absolute inset-0 z-40 flex flex-col bg-neutral-950 text-neutral-100"
  transition:fade={{ duration: 150 }}
>
  <header class="flex items-center gap-3 border-b border-white/10 px-4 py-3">
    {#if activeInDetail}
      <button
        class="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-full text-neutral-300 transition active:scale-90 hover:bg-white/10"
        aria-label="Back"
        onclick={goBack}
      >
        <svg class="h-6 w-6" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <path d="M15 18l-6-6 6-6" />
        </svg>
      </button>
    {/if}
    {#if searching}
      <div class="flex min-w-0 flex-1 items-center gap-2">
        <svg
          class="h-4 w-4 flex-shrink-0 text-neutral-500"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="2"
          stroke-linecap="round"
        >
          <circle cx="11" cy="11" r="7" />
          <path d="M20 20l-3.5-3.5" />
        </svg>
        <input
          bind:this={searchEl}
          bind:value={query}
          type="text"
          inputmode="search"
          autocomplete="off"
          autocorrect="off"
          autocapitalize="off"
          spellcheck="false"
          placeholder="Artists, albums, tracks"
          aria-label="Search library"
          class="min-w-0 flex-1 bg-transparent text-base text-neutral-100 placeholder:text-neutral-500 focus:outline-none"
        />
        {#if query !== ""}
          <button
            class="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-full text-neutral-400 transition active:scale-90 hover:bg-white/10 hover:text-white"
            aria-label="Clear search"
            onclick={() => {
              query = "";
              searchEl?.focus();
            }}
          >
            <svg class="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <path d="M6 6l12 12M18 6L6 18" />
            </svg>
          </button>
        {/if}
      </div>
    {:else}
      <h2 class="min-w-0 flex-1 truncate text-base font-semibold">{title}</h2>
      {#if !activeInDetail}
        <button
          class="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-full text-neutral-400 transition active:scale-90 hover:bg-white/10 hover:text-white"
          aria-label="Search"
          onclick={openSearch}
        >
          <svg class="h-5 w-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
            <circle cx="11" cy="11" r="7" />
            <path d="M20 20l-3.5-3.5" />
          </svg>
        </button>
      {/if}
    {/if}
    {#if tab === "artists" && aInDetail && aHasPartial}
      <button
        class={`flex h-10 flex-shrink-0 items-center gap-2 rounded-full px-4 text-xs font-medium transition active:scale-95 ${
          aFull ? "bg-emerald-400 text-neutral-950" : "bg-white/10 text-neutral-300"
        }`}
        onclick={() => artistBrowser?.toggleFull()}
      >
        <svg class="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="9" />
          <circle cx="12" cy="12" r="3" />
        </svg>
        Full albums
      </button>
    {/if}
    {#if searching}
      <button
        class="h-10 flex-shrink-0 rounded-full px-2 text-sm font-medium text-neutral-300 transition active:scale-95 hover:text-white"
        onclick={closeSearch}
      >
        Cancel
      </button>
    {:else}
      <button
        class="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-full text-neutral-400 transition active:scale-90 hover:bg-white/10 hover:text-white"
        aria-label="Close"
        onclick={onclose}
      >
        <svg class="h-5 w-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
          <path d="M6 6l12 12M18 6L6 18" />
        </svg>
      </button>
    {/if}
  </header>

  {#if searching}
    <Search
      {remote}
      {query}
      onartist={openArtistFromSearch}
      onalbum={openAlbumFromSearch}
    />
  {:else}
  {#if !activeInDetail}
    <div
      class="flex items-center gap-1 overflow-x-auto border-b border-white/10 px-3 py-2 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
    >
      {#each tabs as entry (entry.id)}
        <button
          class={`flex-shrink-0 rounded-full px-3 py-1.5 text-sm font-semibold tracking-wide transition ${
            tab === entry.id ? "bg-white/10 text-white" : "text-neutral-400 hover:text-neutral-200"
          }`}
          onclick={() => (tab = entry.id)}
        >
          {entry.label}
        </button>
      {/each}
    </div>
  {/if}

  <div class={`min-h-0 flex-1 flex-col ${tab === "artists" ? "flex" : "hidden"}`}>
    <ArtistBrowser
      bind:this={artistBrowser}
      {remote}
      bind:inDetail={aInDetail}
      bind:detailName={aName}
      bind:detailHasPartial={aHasPartial}
      bind:detailFull={aFull}
    />
  </div>
  <div class={`min-h-0 flex-1 flex-col ${tab === "albums" ? "flex" : "hidden"}`}>
    <AlbumBrowser
      bind:this={albumBrowser}
      {remote}
      bind:inDetail={albInDetail}
      bind:detailName={albName}
    />
  </div>
  <div class={`min-h-0 flex-1 flex-col ${tab === "folders" ? "flex" : "hidden"}`}>
    <FolderBrowser
      bind:this={folderBrowser}
      {remote}
      bind:inDetail={fInDetail}
      bind:detailName={fName}
    />
  </div>
  <div class={`min-h-0 flex-1 flex-col ${tab === "all" ? "flex" : "hidden"}`}>
    <AllTracks {remote} />
  </div>
  <div class={`min-h-0 flex-1 flex-col ${tab === "playlists" ? "flex" : "hidden"}`}>
    <PlaylistBrowser
      bind:this={plBrowser}
      {remote}
      bind:inDetail={plInDetail}
      bind:detailName={plName}
    />
  </div>
  {/if}
</div>
