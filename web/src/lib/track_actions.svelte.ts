/// The one track the action sheet is about. A single sheet is mounted at the
/// root and every row opens it, so a long press inside the library overlay and
/// one in the queue sheet behave identically.
export type SheetTrack = {
  id: number;
  title: string;
  artist: string | null;
  coverId: number | null;
  albumId: number | null;
};

export const trackSheet = $state<{ track: SheetTrack | null }>({ track: null });

export function openTrackSheet(track: SheetTrack) {
  trackSheet.track = track;
}

export function closeTrackSheet() {
  trackSheet.track = null;
}
