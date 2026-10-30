<script lang="ts">
  import { Remote } from "./connection.svelte";

  /// The volume slider that lives in the dock, for people who would rather not
  /// have a popover land on top of the artwork. Which one shows is a desktop
  /// setting (`remote_volume_inline`), carried in the state payload.
  let { remote }: { remote: Remote } = $props();

  function onInput(event: Event) {
    const value = Number((event.currentTarget as HTMLInputElement).value);
    remote.previewVolume(value / 100);
  }

  function onEnd(event: Event) {
    const value = Number((event.currentTarget as HTMLInputElement).value);
    remote.endVolume(value / 100);
  }
</script>

<div class="flex min-w-0 flex-1 items-center gap-2 pl-1">
  <span class="flex-shrink-0 text-neutral-400" aria-hidden="true">
    <svg class="h-5 w-5" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M11 5L6 9H2v6h4l5 4V5z" fill="currentColor" stroke="none" />
      {#if remote.volume <= 0}
        <line x1="17" y1="9" x2="23" y2="15" />
        <line x1="23" y1="9" x2="17" y2="15" />
      {:else if remote.volume < 0.5}
        <path d="M15.5 8.5a5 5 0 0 1 0 7" />
      {:else}
        <path d="M15.5 8.5a5 5 0 0 1 0 7" />
        <path d="M18.5 5.5a9 9 0 0 1 0 13" />
      {/if}
    </svg>
  </span>
  <input
    type="range"
    class="seek min-w-0 flex-1"
    min="0"
    max="100"
    step="1"
    value={remote.volume * 100}
    style={`--p:${remote.volume * 100}%`}
    aria-label="Volume"
    disabled={remote.volumeLocked}
    oninput={onInput}
    onchange={onEnd}
    onpointerup={onEnd}
    onpointercancel={onEnd}
    onlostpointercapture={onEnd}
  />
</div>
