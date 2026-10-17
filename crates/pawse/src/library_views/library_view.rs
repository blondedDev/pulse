use gpui::{
    AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement, MouseButton,
    NavigationDirection, ParentElement, Render, Styled, Subscription, Window, div,
};
use gpui_component::v_flex;

use crate::library_views::albums_view::{AlbumSelectedEvent, AlbumsView, OpenLibrarySettings};
use crate::library_views::artists_view::{ArtistSelectedEvent, ArtistsView};
use crate::library_views::folders_view::{FolderSelectedEvent, FoldersView};
use crate::library_views::genres_view::{GenreSelectedEvent, GenresView};
use crate::library_views::grouped_tracks_view::GroupedTracksView;
use crate::library_views::playlist_tracks_view::PlaylistTracksView;
use crate::library_views::playlists_view::{PlaylistSelectedEvent, PlaylistsView};
use crate::library_views::rewind_view::RewindView;
use crate::library_views::tracks_view::TracksView;
use crate::localization::tr;
use crate::now_playing::{NavigateToAlbumRequested, NavigateToArtistRequested};
use crate::playback_queue::QueueSource;
use crate::services::Services;
use crate::settings_store::SettingsStore;
use music_library::ArtistGrouping;

#[derive(Clone, Debug)]
pub enum LibraryViewEvent {
    StateChanged,
    OpenLibrarySettings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryRootTab {
    Albums,
    Artists,
    Genres,
    /// The whole catalog as one list. Its own tab rather than a row inside
    /// Playlists: it is not a playlist, and burying it there made it look like
    /// one.
    AllTracks,
    Playlists,
    /// What the listening history adds up to.
    Rewind,
    Folders,
}

impl LibraryRootTab {
    /// The reverse of [`Self::resolve_startup`]: which saved preference lands
    /// on this tab. Lets the startup-tab setting offer exactly the tabs that
    /// can be reached.
    pub fn from_startup(tab: crate::settings_store::StartupTab) -> Self {
        match tab {
            crate::settings_store::StartupTab::Albums => Self::Albums,
            crate::settings_store::StartupTab::Artists => Self::Artists,
            crate::settings_store::StartupTab::Genres => Self::Genres,
            crate::settings_store::StartupTab::AllTracks => Self::AllTracks,
            crate::settings_store::StartupTab::Folders => Self::Folders,
            crate::settings_store::StartupTab::Playlists => Self::Playlists,
        }
    }

    /// Whether this tab can actually be shown, given which features the user
    /// has enabled. Used to keep a saved startup tab from pointing at a hidden
    /// tab.
    pub fn is_available(self, settings: &crate::settings_store::SettingsStore) -> bool {
        match self {
            Self::Albums | Self::Artists | Self::AllTracks => true,
            Self::Genres => settings.genres_enabled(),
            Self::Folders => settings.folders_enabled(),
            Self::Playlists => settings.playlists_enabled(),
            Self::Rewind => settings.rewind_enabled(),
        }
    }

    /// Map a stored startup preference onto a tab that is actually reachable.
    /// Falls back to Albums, the one tab that is always present.
    pub fn resolve_startup(
        preferred: crate::settings_store::StartupTab,
        settings: &crate::settings_store::SettingsStore,
    ) -> Self {
        let tab = Self::from_startup(preferred);
        if tab.is_available(settings) {
            tab
        } else {
            Self::Albums
        }
    }
}

enum NavEntry {
    Root(LibraryRootTab),
    AlbumTracks {
        view: Entity<TracksView>,
        _sub: Subscription,
        target: RedoTarget,
    },
    ArtistTracks {
        view: Entity<GroupedTracksView>,
        _subs: [Subscription; 2],
        target: RedoTarget,
    },
    GenreTracks {
        view: Entity<GroupedTracksView>,
        _subs: [Subscription; 2],
        target: RedoTarget,
    },
    /// A folders list scoped to a folder the user opened. One of these is
    /// pushed per level of drill-down.
    FolderLevel {
        view: Entity<FoldersView>,
        _subs: [Subscription; 2],
        target: RedoTarget,
    },
    FolderTracks {
        view: Entity<GroupedTracksView>,
        _subs: [Subscription; 2],
        target: RedoTarget,
    },
    PlaylistTracks {
        view: Entity<PlaylistTracksView>,
        target: RedoTarget,
    },
}

/// How to reopen a frame that was popped off the navigation stack.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RedoTarget {
    Album(i64),
    Artist(i64),
    Genre(String),
    /// A folder path, one drill-down level.
    Folder(String),
    Playlist(i64),
}

impl NavEntry {
    /// What has to be reopened to get this frame back. `Root` is the bottom of
    /// the stack and is never popped, so it needs nothing.
    fn redo_target(&self) -> Option<RedoTarget> {
        match self {
            Self::Root(_) => None,
            Self::AlbumTracks { target, .. }
            | Self::ArtistTracks { target, .. }
            | Self::GenreTracks { target, .. }
            | Self::FolderLevel { target, .. }
            | Self::FolderTracks { target, .. }
            | Self::PlaylistTracks { target, .. } => Some(target.clone()),
        }
    }
}

/// The forward half of browser-style history.
///
/// Back pushes what it popped, forward pops it again. A frame cannot be stored
/// here (it holds subscriptions), so only the description of it is kept.
///
/// Opening something the user picked *abandons* the queue, like a browser —
/// but a frame opened while replaying a popped one must not, or pressing
/// forward twice would only ever work once.
#[derive(Default)]
struct RedoHistory {
    entries: Vec<RedoTarget>,
    /// Set between `take_next` and `end_replay`.
    replaying: bool,
}

impl RedoHistory {
    fn push(&mut self, target: RedoTarget) {
        self.entries.push(target);
    }

    /// Claim the next frame to reopen, and mark the rest of the queue as
    /// behind a replay until `end_replay`.
    fn take_next(&mut self) -> Option<RedoTarget> {
        let next = self.entries.pop()?;
        self.replaying = true;
        Some(next)
    }

    fn end_replay(&mut self) {
        self.replaying = false;
    }

    /// Called by every frame opener once it is on the stack.
    fn abandon(&mut self) {
        if !self.replaying {
            self.entries.clear();
        }
    }

    fn can_redo(&self) -> bool {
        !self.entries.is_empty()
    }
}

struct GenresRoot {
    view: Entity<GenresView>,
    _subs: [Subscription; 2],
}

struct FoldersRoot {
    view: Entity<FoldersView>,
    _subs: [Subscription; 2],
}

pub struct LibraryView {
    stack: Vec<NavEntry>,
    /// What `go_back` popped, so the forward mouse button can reopen it.
    redo: RedoHistory,
    albums_view: Entity<AlbumsView>,
    artists_view: Entity<ArtistsView>,
    genres: Option<GenresRoot>,
    folders: Option<FoldersRoot>,
    playlists_view: Entity<PlaylistsView>,
    /// Built the first time Rewind is opened: it runs a query over the whole
    /// listening history, which is wasted work for someone who never looks.
    rewind_view: Option<Entity<RewindView>>,
    /// The whole catalog, built on the first `select_tab(AllTracks)`. Loading
    /// every track into row data is not free, so it is not built at launch.
    all_tracks_view: Option<Entity<PlaylistTracksView>>,
    _album_subscription: Subscription,
    _artist_subscription: Subscription,
    _playlist_subscription: Subscription,
    _settings_subscriptions: [Subscription; 2],
    _settings_observer: Subscription,
}

impl LibraryView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let albums_view = cx.new(|cx| AlbumsView::new(window, cx));
        let artists_view = cx.new(|cx| ArtistsView::new(window, cx));
        let playlists_view = cx.new(|cx| PlaylistsView::new(window, cx));

        let album_subscription =
            cx.subscribe(&albums_view, |this, _, event: &AlbumSelectedEvent, cx| {
                this.show_album_tracks(event.album.clone(), cx);
            });

        let artist_subscription =
            cx.subscribe(&artists_view, |this, _, event: &ArtistSelectedEvent, cx| {
                let grouping = cx.global::<SettingsStore>().artists_grouping();
                this.show_artist_tracks(event.artist.clone(), grouping, cx);
            });

        let playlist_subscription = cx.subscribe_in(
            &playlists_view,
            window,
            |this, _, event: &PlaylistSelectedEvent, window, cx| {
                this.show_playlist_tracks(event.playlist.clone(), window, cx);
            },
        );

        let settings_subscriptions = [            cx.subscribe(&albums_view, |_, _, _: &OpenLibrarySettings, cx| {
                cx.emit(LibraryViewEvent::OpenLibrarySettings);
            }),
            cx.subscribe(&artists_view, |_, _, _: &OpenLibrarySettings, cx| {
                cx.emit(LibraryViewEvent::OpenLibrarySettings);
            }),
        ];

        let settings_observer = cx.observe_global::<SettingsStore>(|this, cx| {
            let store = cx.global::<SettingsStore>();
            let playlists = store.playlists_enabled();
            let rewind = store.rewind_enabled();
            if !rewind {
                this.rewind_view = None;
            }
            let genres = store.genres_enabled();
            let folders = store.folders_enabled();
            if !genres {
                this.genres = None;
            }
            if !folders {
                this.folders = None;
            }
            let before = this.stack.len();
            this.stack.retain(|entry| match entry {
                NavEntry::Root(LibraryRootTab::Playlists) => playlists,
                NavEntry::PlaylistTracks { .. } => playlists,
                NavEntry::Root(LibraryRootTab::Rewind) => rewind,
                NavEntry::Root(LibraryRootTab::Genres) => genres,
                NavEntry::GenreTracks { .. } => genres,
                NavEntry::Root(LibraryRootTab::Folders) => folders,
                NavEntry::FolderLevel { .. } | NavEntry::FolderTracks { .. } => folders,
                _ => true,
            });
            if this.stack.len() == before {
                return;
            }
            if !matches!(this.stack.first(), Some(NavEntry::Root(_))) {
                this.stack = vec![NavEntry::Root(LibraryRootTab::Albums)];
            }
            cx.emit(LibraryViewEvent::StateChanged);
            cx.notify();
        });

        Self {
            stack: vec![NavEntry::Root(LibraryRootTab::Albums)],
            redo: RedoHistory::default(),
            albums_view,
            artists_view,
            genres: None,
            folders: None,
            playlists_view,
            rewind_view: None,
            all_tracks_view: None,
            _album_subscription: album_subscription,
            _artist_subscription: artist_subscription,
            _playlist_subscription: playlist_subscription,
            _settings_subscriptions: settings_subscriptions,
            _settings_observer: settings_observer,
        }
    }

    pub fn is_drilled_in(&self) -> bool {
        self.stack.len() > 1
    }

    pub fn current_tab(&self) -> Option<LibraryRootTab> {
        if self.is_drilled_in() {
            return None;
        }
        match self.stack.first() {
            Some(NavEntry::Root(t)) => Some(*t),
            _ => None,
        }
    }

    pub fn select_tab(
        &mut self,
        tab: LibraryRootTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same_root =
            self.stack.len() == 1 && matches!(self.stack[0], NavEntry::Root(t) if t == tab);
        if same_root {
            return;
        }
        if tab == LibraryRootTab::Genres {
            self.ensure_genres_view(cx);
        }
        if tab == LibraryRootTab::Folders {
            self.ensure_folders_view(cx);
        }
        if tab == LibraryRootTab::AllTracks {
            self.ensure_all_tracks_view(window, cx);
        }
        if tab == LibraryRootTab::Rewind {
            // Refresh on open rather than on every play: this is a view people
            // glance at, not one they watch.
            let view = self.ensure_rewind_view(cx);
            view.update(cx, |view, cx| view.reload(cx));
        }
        self.stack = vec![NavEntry::Root(tab)];
        self.redo.abandon();
        cx.emit(LibraryViewEvent::StateChanged);
        cx.notify();
    }

    fn ensure_rewind_view(&mut self, cx: &mut Context<Self>) -> Entity<RewindView> {
        if self.rewind_view.is_none() {
            self.rewind_view = Some(cx.new(RewindView::new));
        }
        self.rewind_view.clone().expect("created just above")
    }

    pub fn apply_search(&mut self, query: &str, cx: &mut Context<Self>) {
        match self.stack.last() {
            Some(NavEntry::Root(LibraryRootTab::Albums)) => {
                self.albums_view.update(cx, |v, cx| v.set_filter(query, cx));
            }
            Some(NavEntry::Root(LibraryRootTab::Artists)) => {
                self.artists_view
                    .update(cx, |v, cx| v.set_filter(query, cx));
            }
            Some(NavEntry::Root(LibraryRootTab::Genres)) => {
                if let Some(genres) = &self.genres {
                    genres.view.update(cx, |v, cx| v.set_filter(query, cx));
                }
            }
            Some(NavEntry::Root(LibraryRootTab::Folders)) => {
                if let Some(folders) = &self.folders {
                    folders.view.update(cx, |v, cx| v.set_filter(query, cx));
                }
            }
            Some(NavEntry::Root(LibraryRootTab::Playlists)) => {
                self.playlists_view
                    .update(cx, |v, cx| v.set_filter(query, cx));
            }
            Some(NavEntry::Root(LibraryRootTab::Rewind)) => {}
            Some(NavEntry::Root(LibraryRootTab::AllTracks)) => {
                if let Some(all_tracks) = &self.all_tracks_view {
                    all_tracks.update(cx, |v, cx| v.set_filter(query, cx));
                }
            }
            Some(NavEntry::AlbumTracks { view, .. }) => {
                view.update(cx, |v, cx| v.set_filter(query, cx));
            }
            Some(NavEntry::FolderLevel { view, .. }) => {
                view.update(cx, |v, cx| v.set_filter(query, cx));
            }
            Some(
                NavEntry::ArtistTracks { view, .. }
                | NavEntry::GenreTracks { view, .. }
                | NavEntry::FolderTracks { view, .. },
            ) => {
                view.update(cx, |v, cx| v.set_filter(query, cx));
            }
            Some(NavEntry::PlaylistTracks { view, .. }) => {
                view.update(cx, |v, cx| v.set_filter(query, cx));
            }
            None => {}
        }
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.stack.len() > 1
            && let Some(entry) = self.stack.pop()
            && let Some(target) = entry.redo_target()
        {
            self.redo.push(target);
            cx.emit(LibraryViewEvent::StateChanged);
            cx.notify();
        }
    }

    /// Redo the last `go_back` — what the forward mouse button does. Nothing to
    /// redo once a tab or a folder is opened again, since that clears it.
    pub fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.redo.take_next() else {
            return;
        };
        match target {
            RedoTarget::Album(id) => self.navigate_to_album(id, cx),
            RedoTarget::Artist(id) => self.navigate_to_artist(id, None, cx),
            RedoTarget::Genre(name) => self.navigate_to_genre(&name, cx),
            RedoTarget::Folder(path) => self.open_folder_path(path, cx),
            RedoTarget::Playlist(id) => self.navigate_to_playlist(id, window, cx),
        }
        self.redo.end_replay();
        cx.emit(LibraryViewEvent::StateChanged);
        cx.notify();
    }

    pub fn can_go_forward(&self) -> bool {
        self.redo.can_redo()
    }

    pub(crate) fn navigate_to_genre(&mut self, genre_name: &str, cx: &mut Context<Self>) {
        if let Some(genre) = cx
            .global::<Services>()
            .library
            .genres()
            .into_iter()
            .find(|genre| genre.name == genre_name)
        {
            self.show_genre_tracks(genre, cx);
        }
    }

    /// Reopen a folder level by path. The summary is rebuilt from the parent
    /// listing when it is still there; a folder that has since been removed
    /// opens an empty level rather than nothing at all.
    fn open_folder_path(&mut self, path: String, cx: &mut Context<Self>) {
        let (parent, name) = music_library::split_folder_path(&path, std::path::MAIN_SEPARATOR);
        let name = name.to_string();
        let summary = cx
            .global::<Services>()
            .library
            .child_folders(parent)
            .into_iter()
            .find(|folder| folder.path == path)
            .unwrap_or(music_library::FolderSummary {
                name,
                path,
                track_count: 0,
                album_count: 0,
            });
        self.open_folder(summary, cx);
    }

    pub(crate) fn navigate_to_playlist(
        &mut self,
        playlist_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(playlist) = cx
            .global::<Services>()
            .library
            .playlists()
            .into_iter()
            .find(|playlist| playlist.id == playlist_id)
        {
            self.show_playlist_tracks(playlist, window, cx);
        }
    }

    pub fn navigate_to_album(&mut self, album_id: i64, cx: &mut Context<Self>) {
        let services = cx.global::<Services>();
        if let Some(album) = services
            .library
            .albums()
            .into_iter()
            .find(|a| a.id == album_id)
        {
            self.show_album_tracks(album, cx);
        }
    }

    pub fn navigate_to_artist(
        &mut self,
        artist_id: i64,
        from_track: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        let preferred = cx.global::<SettingsStore>().artists_grouping();
        let library = &cx.global::<Services>().library;
        let found = resolve_artist_page(
            artist_id,
            preferred,
            || {
                from_track.and_then(|track_id| {
                    library.listed_artist_for_credit(track_id, artist_id, preferred)
                })
            },
            |id, grouping| library.artist_summary(id, grouping),
        );
        if let Some((artist, grouping)) = found {
            self.show_artist_tracks(artist, grouping, cx);
        }
    }

    fn show_album_tracks(&mut self, album: music_library::AlbumSummary, cx: &mut Context<Self>) {
        let target = RedoTarget::Album(album.id);
        let view = cx.new(|cx| TracksView::new(&album, cx));
        let sub = cx.subscribe(&view, |this, _, event: &NavigateToArtistRequested, cx| {
            this.navigate_to_artist(event.artist_id, event.track_id, cx);
        });
        self.redo.abandon();
        self.stack.push(NavEntry::AlbumTracks {
            view,
            _sub: sub,
            target,
        });
        cx.emit(LibraryViewEvent::StateChanged);
        cx.notify();
    }

    fn show_artist_tracks(
        &mut self,
        artist: music_library::ArtistSummary,
        grouping: ArtistGrouping,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| GroupedTracksView::artist(&artist, grouping, cx));
        let subs = Self::grouped_tracks_subscriptions(&view, cx);
        self.redo.abandon();
        self.stack.push(NavEntry::ArtistTracks {
            view,
            _subs: subs,
            target: RedoTarget::Artist(artist.id),
        });
        cx.emit(LibraryViewEvent::StateChanged);
        cx.notify();
    }

    fn show_genre_tracks(&mut self, genre: music_library::GenreSummary, cx: &mut Context<Self>) {
        let sort = cx.global::<SettingsStore>().genres_sort();
        let view = cx.new(|cx| GroupedTracksView::genre(&genre, sort, cx));
        let subs = Self::grouped_tracks_subscriptions(&view, cx);
        self.redo.abandon();
        self.stack.push(NavEntry::GenreTracks {
            view,
            _subs: subs,
            target: RedoTarget::Genre(genre.name.clone()),
        });
        cx.emit(LibraryViewEvent::StateChanged);
        cx.notify();
    }

    /// Opens a folder one level deeper. If it has subdirectories we push
    /// another folder list so the user keeps drilling down; if it has none we
    /// go straight to its tracks, so a click never dead-ends on an empty list.
    fn open_folder(&mut self, folder: music_library::FolderSummary, cx: &mut Context<Self>) {
        let has_subfolders = !cx
            .global::<Services>()
            .library
            .child_folders(&folder.path)
            .is_empty();
        if has_subfolders {
            let path = folder.path.clone();
            let view = cx.new(|cx| FoldersView::for_folder(&path, cx));
            let subs = [
                cx.subscribe(&view, |this, _, event: &FolderSelectedEvent, cx| {
                    this.open_folder(event.folder.clone(), cx);
                }),
                cx.subscribe(&view, |_, _, _: &OpenLibrarySettings, cx| {
                    cx.emit(LibraryViewEvent::OpenLibrarySettings);
                }),
            ];
            self.redo.abandon();
            self.stack.push(NavEntry::FolderLevel {
                view,
                _subs: subs,
                target: RedoTarget::Folder(path.clone()),
            });
        } else {
            let view = cx.new(|cx| GroupedTracksView::folder(&folder, cx));
            let subs = Self::grouped_tracks_subscriptions(&view, cx);
            self.redo.abandon();
            self.stack.push(NavEntry::FolderTracks {
                view,
                _subs: subs,
                target: RedoTarget::Folder(folder.path.clone()),
            });
        }
        cx.emit(LibraryViewEvent::StateChanged);
        cx.notify();
    }

    fn grouped_tracks_subscriptions(
        view: &Entity<GroupedTracksView>,
        cx: &mut Context<Self>,
    ) -> [Subscription; 2] {
        [
            cx.subscribe(view, |this, _, event: &NavigateToAlbumRequested, cx| {
                this.navigate_to_album(event.album_id, cx);
            }),
            cx.subscribe(view, |this, _, event: &NavigateToArtistRequested, cx| {
                this.navigate_to_artist(event.artist_id, event.track_id, cx);
            }),
        ]
    }

    fn ensure_genres_view(&mut self, cx: &mut Context<Self>) {
        if self.genres.is_some() {
            return;
        }
        let view = cx.new(GenresView::new);
        let subs = [
            cx.subscribe(&view, |this, _, event: &GenreSelectedEvent, cx| {
                this.show_genre_tracks(event.genre.clone(), cx);
            }),
            cx.subscribe(&view, |_, _, _: &OpenLibrarySettings, cx| {
                cx.emit(LibraryViewEvent::OpenLibrarySettings);
            }),
        ];
        self.genres = Some(GenresRoot { view, _subs: subs });
    }

    fn ensure_folders_view(&mut self, cx: &mut Context<Self>) {
        if self.folders.is_some() {
            return;
        }
        let view = cx.new(FoldersView::new);
        let subs = [
            cx.subscribe(&view, |this, _, event: &FolderSelectedEvent, cx| {
                this.open_folder(event.folder.clone(), cx);
            }),
            cx.subscribe(&view, |_, _, _: &OpenLibrarySettings, cx| {
                cx.emit(LibraryViewEvent::OpenLibrarySettings);
            }),
        ];
        self.folders = Some(FoldersRoot { view, _subs: subs });
    }

    /// Build the all-tracks list the first time its tab is opened. It keeps
    /// itself current through the library event bus, so the only thing to do
    /// here is create it once — loading every track into row data is not
    /// something to pay for at launch.
    fn ensure_all_tracks_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.all_tracks_view.is_some() {
            return;
        }
        let view = cx.new(|cx| {
            PlaylistTracksView::new(tr().all_tracks.clone(), QueueSource::AllTracks, window, cx)
        });
        self.all_tracks_view = Some(view);
    }

    fn show_playlist_tracks(
        &mut self,
        playlist: music_library::PlaylistSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.new(|cx| {
            PlaylistTracksView::new(
                playlist.name.clone().into(),
                QueueSource::Playlist(playlist.id),
                window,
                cx,
            )
        });
        self.redo.abandon();
        self.stack.push(NavEntry::PlaylistTracks {
            view,
            target: RedoTarget::Playlist(playlist.id),
        });
        cx.emit(LibraryViewEvent::StateChanged);
        cx.notify();
    }
}

fn resolve_artist_page(
    artist_id: i64,
    preferred: ArtistGrouping,
    listed_for_credit: impl FnOnce() -> Option<i64>,
    summary: impl Fn(i64, ArtistGrouping) -> Option<music_library::ArtistSummary>,
) -> Option<(music_library::ArtistSummary, ArtistGrouping)> {
    let other = match preferred {
        ArtistGrouping::TrackArtist => ArtistGrouping::AlbumArtist,
        ArtistGrouping::AlbumArtist => ArtistGrouping::TrackArtist,
    };
    let open = |id, grouping| summary(id, grouping).map(|artist| (artist, grouping));
    open(artist_id, preferred)
        .or_else(|| {
            listed_for_credit()
                .filter(|&listed| listed != artist_id)
                .and_then(|listed| open(listed, preferred))
        })
        .or_else(|| open(artist_id, other))
}

impl EventEmitter<LibraryViewEvent> for LibraryView {}

impl Render for LibraryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Mouse side buttons (BTN_SIDE/BTN_BACK and BTN_EXTRA/BTN_FORWARD),
        // which GPUI reports as navigation buttons on both Wayland and X11.
        div()
            .relative()
            .size_full()
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, _, cx| this.go_back(cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| this.go_forward(window, cx)),
            )
            .child({
                // Keyed by where we are, not by frame: a new tab or a new depth
                // starts the fade, everything else leaves it alone.
                let key = self.stack.len() as u64 * 100
                    + self.current_tab().map(|tab| tab as u64).unwrap_or(99);
                let content = match self.stack.last() {
                Some(NavEntry::Root(LibraryRootTab::Albums)) => {
                    v_flex().size_full().child(self.albums_view.clone())
                }
                Some(NavEntry::Root(LibraryRootTab::Artists)) => {
                    v_flex().size_full().child(self.artists_view.clone())
                }
                Some(NavEntry::Root(LibraryRootTab::Genres)) => match &self.genres {
                    Some(genres) => v_flex().size_full().child(genres.view.clone()),
                    None => v_flex().size_full(),
                },
                Some(NavEntry::Root(LibraryRootTab::Folders)) => match &self.folders {
                    Some(folders) => v_flex().size_full().child(folders.view.clone()),
                    None => v_flex().size_full(),
                },
                Some(NavEntry::Root(LibraryRootTab::Playlists)) => {
                    v_flex().size_full().child(self.playlists_view.clone())
                }
                Some(NavEntry::Root(LibraryRootTab::Rewind)) => match &self.rewind_view {
                    Some(rewind) => v_flex().size_full().child(rewind.clone()),
                    None => v_flex().size_full(),
                },
                Some(NavEntry::Root(LibraryRootTab::AllTracks)) => match &self.all_tracks_view {
                    Some(all_tracks) => v_flex().size_full().child(all_tracks.clone()),
                    None => v_flex().size_full(),
                },
                Some(NavEntry::AlbumTracks { view, .. }) => {
                    v_flex().size_full().child(view.clone())
                }
                Some(NavEntry::FolderLevel { view, .. }) => {
                    v_flex().size_full().child(view.clone())
                }
                Some(
                    NavEntry::ArtistTracks { view, .. }
                    | NavEntry::GenreTracks { view, .. }
                    | NavEntry::FolderTracks { view, .. },
                ) => v_flex().size_full().child(view.clone()),
                Some(NavEntry::PlaylistTracks { view, .. }) => {
                    v_flex().size_full().child(view.clone())
                }
                    None => v_flex().size_full(),
                };
                crate::animations::fade_in(
                    crate::animations::AnimationKind::ViewFade,
                    ("view-fade", key),
                    content,
                    cx,
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const MAIN: i64 = 1;
    const FEATURED: i64 = 2;
    const GUEST: i64 = 3;
    const VARIOUS: i64 = 4;

    fn summary(id: i64, grouping: ArtistGrouping) -> Option<music_library::ArtistSummary> {
        let listed = match grouping {
            ArtistGrouping::AlbumArtist => [MAIN, VARIOUS].contains(&id),
            ArtistGrouping::TrackArtist => [MAIN, FEATURED, GUEST].contains(&id),
        };
        listed.then(|| music_library::ArtistSummary {
            id,
            name: id.to_string(),
            sort_name: id.to_string(),
            track_count: 1,
        })
    }

    fn resolve(
        artist_id: i64,
        preferred: ArtistGrouping,
        listed: Option<i64>,
    ) -> (Option<(i64, ArtistGrouping)>, bool) {
        let asked = Cell::new(false);
        let found = resolve_artist_page(
            artist_id,
            preferred,
            || {
                asked.set(true);
                listed
            },
            summary,
        );
        (
            found.map(|(artist, grouping)| (artist.id, grouping)),
            asked.get(),
        )
    }

    #[rstest::rstest]
    #[case::listed_artist_opens_directly(MAIN, ArtistGrouping::AlbumArtist, Some(MAIN), Some((MAIN, ArtistGrouping::AlbumArtist)))]
    #[case::featured_credit_opens_its_listed_artist(FEATURED, ArtistGrouping::AlbumArtist, Some(MAIN), Some((MAIN, ArtistGrouping::AlbumArtist)))]
    #[case::compilation_guest_keeps_their_own_page(GUEST, ArtistGrouping::AlbumArtist, None, Some((GUEST, ArtistGrouping::TrackArtist)))]
    #[case::album_artist_link_in_track_grouping(VARIOUS, ArtistGrouping::TrackArtist, None, Some((VARIOUS, ArtistGrouping::AlbumArtist)))]
    #[case::listed_same_as_clicked_falls_through(FEATURED, ArtistGrouping::AlbumArtist, Some(FEATURED), Some((FEATURED, ArtistGrouping::TrackArtist)))]
    #[case::unknown_artist_goes_nowhere(99, ArtistGrouping::AlbumArtist, None, None)]
    fn artist_navigation_picks_the_first_page_that_exists(
        #[case] clicked: i64,
        #[case] preferred: ArtistGrouping,
        #[case] listed: Option<i64>,
        #[case] expected: Option<(i64, ArtistGrouping)>,
    ) {
        assert_eq!(resolve(clicked, preferred, listed).0, expected);
    }

    #[test]
    fn the_credit_lookup_runs_only_when_the_clicked_artist_is_not_listed() {
        assert!(!resolve(MAIN, ArtistGrouping::AlbumArtist, Some(MAIN)).1);
        assert!(resolve(FEATURED, ArtistGrouping::AlbumArtist, Some(MAIN)).1);
    }

    fn rock() -> RedoTarget {
        RedoTarget::Genre("Rock".to_string())
    }

    fn jazz() -> RedoTarget {
        RedoTarget::Genre("Jazz".to_string())
    }

    /// Steps through what a replay does: take the next frame, let the opener it
    /// goes through call `abandon`, then end the replay.
    fn replay(redo: &mut RedoHistory) -> Option<RedoTarget> {
        let next = redo.take_next();
        redo.abandon();
        redo.end_replay();
        next
    }

    /// Two backs then two forwards must both land. A frame opener that cleared
    /// the queue on the way through would leave nothing for the second press,
    /// so the forward button would only ever work once.
    #[test]
    fn redo_replays_as_many_frames_as_were_went_back() {
        let mut redo = RedoHistory::default();
        redo.push(rock());
        redo.push(jazz());

        assert_eq!(replay(&mut redo), Some(jazz()));
        assert!(
            redo.can_redo(),
            "the frame behind the replay must survive it"
        );
        assert_eq!(replay(&mut redo), Some(rock()));
        assert_eq!(replay(&mut redo), None);
    }

    #[test]
    fn opening_something_new_abandons_the_whole_forward_queue() {
        let mut redo = RedoHistory::default();
        redo.push(rock());
        redo.push(jazz());

        redo.abandon();

        assert!(!redo.can_redo());
        assert_eq!(redo.take_next(), None);
    }

    /// Back, back, forward, back: the frame just left is queued again behind
    /// the one that was never revisited, so forward walks them in order.
    #[test]
    fn going_back_again_after_a_replay_keeps_both_frames_queued() {
        let mut redo = RedoHistory::default();
        redo.push(rock());
        redo.push(jazz());

        assert_eq!(replay(&mut redo), Some(jazz()));
        redo.push(jazz());

        assert_eq!(replay(&mut redo), Some(jazz()));
        assert_eq!(replay(&mut redo), Some(rock()));
        assert!(!redo.can_redo());
    }

    #[test]
    fn a_frame_popped_during_a_replay_is_not_lost() {
        let mut redo = RedoHistory::default();
        redo.push(rock());

        assert_eq!(redo.take_next(), Some(rock()));
        assert!(!redo.can_redo(), "the target is claimed, not just peeked");
        redo.abandon();
        redo.end_replay();
        assert_eq!(redo.take_next(), None);
    }
}
