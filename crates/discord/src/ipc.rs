use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use discord_rich_presence::activity::{Activity, ActivityType, Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use flume::{Receiver, RecvTimeoutError, Sender};

use crate::art::ArtCache;
use crate::{Header, Line, Presence, Timers, Verb};

const TICK: Duration = Duration::from_secs(1);
const MIN_INTERVAL: Duration = Duration::from_secs(5);
const RECONNECT_BACKOFF: Duration = Duration::from_secs(15);

enum Msg {
    Set(Presence),
    Clear,
    Shutdown,
}

#[derive(Clone)]
pub struct DiscordHandle {
    tx: Sender<Msg>,
}

impl DiscordHandle {
    pub fn spawn(client_id: String, art_cache_path: PathBuf) -> Option<Self> {
        let (tx, rx) = flume::unbounded();
        thread::Builder::new()
            .name("discord-rpc".to_string())
            .spawn(move || {
                let mut worker = Worker {
                    client: DiscordIpcClient::new(&client_id),
                    art: ArtCache::load(art_cache_path),
                    connected: false,
                    desired: None,
                    dirty: false,
                    last_apply: Instant::now()
                        .checked_sub(MIN_INTERVAL)
                        .unwrap_or_else(Instant::now),
                    next_connect: Instant::now(),
                };
                worker.run(rx);
            })
            .map_err(|e| log::error!("discord: failed to spawn worker thread: {e}"))
            .ok()?;
        Some(Self { tx })
    }

    pub fn set(&self, presence: Presence) {
        let _ = self.tx.send(Msg::Set(presence));
    }

    pub fn clear(&self) {
        let _ = self.tx.send(Msg::Clear);
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(Msg::Shutdown);
    }
}

struct Worker {
    client: DiscordIpcClient,
    art: ArtCache,
    connected: bool,
    desired: Option<Presence>,
    dirty: bool,
    last_apply: Instant,
    next_connect: Instant,
}

impl Worker {
    fn run(&mut self, rx: Receiver<Msg>) {
        loop {
            let received = if self.dirty {
                rx.recv_timeout(TICK)
            } else {
                rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
            };
            match received {
                Ok(msg) => {
                    if self.handle(msg) {
                        return;
                    }
                    while let Ok(msg) = rx.try_recv() {
                        if self.handle(msg) {
                            return;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.apply(Instant::now());
        }
    }

    fn handle(&mut self, msg: Msg) -> bool {
        match msg {
            Msg::Set(p) => {
                if self.desired.as_ref() != Some(&p) {
                    self.desired = Some(p);
                    self.dirty = true;
                }
            }
            Msg::Clear => {
                if self.desired.is_some() {
                    self.desired = None;
                    self.dirty = true;
                }
            }
            Msg::Shutdown => {
                if self.connected {
                    let _ = self.client.clear_activity();
                    let _ = self.client.close();
                }
                return true;
            }
        }
        false
    }

    fn apply(&mut self, now: Instant) {
        if !self.dirty {
            return;
        }
        if !self.connected && self.desired.is_none() {
            self.dirty = false;
            return;
        }
        if !self.connected {
            if now < self.next_connect {
                return;
            }
            match self.client.connect() {
                Ok(()) => self.connected = true,
                Err(_) => {
                    self.next_connect = now + RECONNECT_BACKOFF;
                    return;
                }
            }
        }
        if now.duration_since(self.last_apply) < MIN_INTERVAL {
            return;
        }
        // Cloned so the art cache can be borrowed mutably while the desired
        // presence is still in hand; a presence is a few strings.
        let result = match self.desired.clone() {
            Some(p) => self.build_and_set_activity(&p),
            None => self.client.clear_activity(),
        };
        match result {
            Ok(()) => {
                self.last_apply = now;
                self.dirty = false;
            }
            Err(_) => {
                self.connected = false;
                self.next_connect = now + RECONNECT_BACKOFF;
            }
        }
    }

    fn build_and_set_activity(
        &mut self,
        p: &Presence,
    ) -> Result<(), discord_rich_presence::error::Error> {
        let art = if p.settings.show_cover {
            self.art
                .resolve(p.album_artist.as_deref(), &p.artist, p.album.as_deref())
        } else {
            None
        };
        self.client.set_activity(activity_for(p, art.as_deref()))
    }
}

/// Build the activity Discord is shown.
///
/// Split out from the worker so it can be tested against the JSON that actually
/// goes over the socket: `Activity`'s fields are private, but it serialises, and
/// the serialised form is what Discord reads.
fn activity_for(p: &Presence, cover: Option<&str>) -> Activity<'static> {
    let s = &p.settings;

    let kind = match s.verb {
        Verb::Listening => ActivityType::Listening,
        Verb::Playing => ActivityType::Playing,
    };
    let mut activity = Activity::new().activity_type(kind);

    // Unset is what keeps Discord's application name on the top line; a space is
    // the one value it renders as nothing at all. An empty string would be
    // dropped and the app name would come back.
    if let Some(name) = header_name(p, s.header) {
        activity = activity.name(name);
    }

    if let Some(text) = line(p, s.line_one) {
        activity = activity.details(text);
    }

    // A paused activity has one job: say that it is paused. The second line is
    // the only place that shows without an uploaded asset, so it is taken over
    // when the user asked for that.
    let second = if p.paused && s.paused_line {
        Some(PAUSED.to_string())
    } else {
        line(p, s.line_two)
    };
    if let Some(text) = second {
        activity = activity.state(text);
    }

    let (small_image, small_text) = if p.paused {
        ("pause", PAUSED)
    } else {
        ("play", "Playing")
    };
    let mut assets = Assets::new()
        .small_image(small_image)
        .small_text(small_text);
    if let Some(url) = cover {
        assets = assets.large_image(url.to_string());
        if let Some(text) = line(p, s.cover_text) {
            assets = assets.large_text(text);
        }
    }
    activity = activity.assets(assets);

    // A paused timer would keep counting up, which is a lie, so the timestamps
    // are dropped rather than frozen.
    if !p.paused && s.timers != Timers::Off {
        let (start, end) = spans(p);
        let mut ts = Timestamps::new().start(start);
        if s.timers == Timers::Progress && let Some(end) = end {
            ts = ts.end(end);
        }
        activity = activity.timestamps(ts);
    }

    activity
}

const PAUSED: &str = "Paused";

/// The activity name, or `None` to leave Discord's application name in place.
fn header_name(p: &Presence, header: Header) -> Option<String> {
    let content = header.line()?;
    // A chosen field that this track does not have falls back to the blank
    // line, not to the app name: the user asked for the app name not to be
    // there, and a missing album should not quietly put it back.
    Some(line(p, content).unwrap_or_else(|| " ".to_string()))
}

/// The text one line of the activity shows, or `None` to leave the line out.
fn line(p: &Presence, content: Line) -> Option<String> {
    match content {
        Line::Off => None,
        Line::Title => non_empty(&p.title),
        Line::Artist => non_empty(&p.artist),
        Line::Album => p.album.as_deref().and_then(non_empty),
        Line::Year => p.year.map(|year| year.to_string()),
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_string())
}

fn spans(p: &Presence) -> (i64, Option<i64>) {
    let dur = p.duration.as_secs() as i64;
    (p.started_at, (dur > 0).then_some(p.started_at + dur))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PresenceSettings;
    use serde_json::Value;

    fn presence() -> Presence {
        Presence {
            artist: "Drake".to_string(),
            album_artist: None,
            title: "Take Care".to_string(),
            album: Some("Take Care".to_string()),
            year: Some(2011),
            duration: Duration::from_secs(180),
            started_at: 1000,
            paused: false,
            settings: PresenceSettings::default(),
        }
    }

    fn json(p: &Presence, cover: Option<&str>) -> Value {
        serde_json::to_value(activity_for(p, cover)).unwrap()
    }

    #[test]
    fn the_defaults_are_title_artist_and_a_progress_bar() {
        let value = json(&presence(), None);
        assert_eq!(value["details"], "Take Care");
        assert_eq!(value["state"], "Drake");
        assert_eq!(value["type"], 2, "Listening");
        assert_eq!(value["timestamps"]["start"], 1000);
        assert_eq!(value["timestamps"]["end"], 1180, "start + duration");
    }

    #[test]
    fn a_line_set_to_off_is_left_out() {
        let mut p = presence();
        p.settings.line_two = Line::Off;
        let value = json(&p, None);
        assert!(value.get("state").is_none());
        assert_eq!(value["details"], "Take Care");
    }

    #[test]
    fn the_album_can_take_the_second_line() {
        let mut p = presence();
        p.settings.line_two = Line::Album;
        assert_eq!(json(&p, None)["state"], "Take Care");

        p.settings.line_one = Line::Year;
        assert_eq!(json(&p, None)["details"], "2011");
    }

    #[test]
    fn a_missing_field_leaves_its_line_out_rather_than_empty() {
        let mut p = presence();
        p.album = None;
        p.settings.line_two = Line::Album;
        assert!(json(&p, None).get("state").is_none());
    }

    #[test]
    fn a_paused_activity_says_so_on_the_second_line() {
        let mut p = presence();
        p.paused = true;
        let value = json(&p, None);
        assert_eq!(value["details"], "Take Care");
        assert_eq!(value["state"], "Paused");
        assert!(
            value.get("timestamps").is_none(),
            "a paused timer would keep counting up"
        );
    }

    #[test]
    fn the_paused_line_can_be_turned_off() {
        let mut p = presence();
        p.paused = true;
        p.settings.paused_line = false;
        assert_eq!(json(&p, None)["state"], "Drake");
    }

    #[test]
    fn the_clock_can_count_up_or_be_turned_off() {
        let mut p = presence();
        p.settings.timers = Timers::Elapsed;
        let value = json(&p, None);
        assert_eq!(value["timestamps"]["start"], 1000);
        assert!(value["timestamps"].get("end").is_none());

        p.settings.timers = Timers::Off;
        assert!(json(&p, None).get("timestamps").is_none());
    }

    #[test]
    fn the_verb_is_the_users_choice() {
        let mut p = presence();
        assert_eq!(json(&p, None)["type"], 2);
        p.settings.verb = Verb::Playing;
        assert_eq!(json(&p, None)["type"], 0);
    }

    #[test]
    fn the_cover_carries_the_tooltip_and_only_when_there_is_a_cover() {
        let value = json(&presence(), Some("https://img/512x512bb.jpg"));
        assert_eq!(value["assets"]["large_image"], "https://img/512x512bb.jpg");
        assert_eq!(value["assets"]["large_text"], "Take Care");

        let mut p = presence();
        p.settings.cover_text = Line::Off;
        let value = json(&p, Some("https://img/512x512bb.jpg"));
        assert!(value["assets"].get("large_text").is_none());

        let value = json(&presence(), None);
        assert!(value["assets"].get("large_image").is_none());
    }

    #[test]
    fn the_header_is_the_app_name_until_the_user_changes_it() {
        assert!(
            json(&presence(), None).get("name").is_none(),
            "an unset name keeps Discord's application name"
        );

        let mut p = presence();
        p.settings.header = Header::Off;
        assert_eq!(json(&p, None)["name"], " ", "a space renders as nothing");

        p.settings.header = Header::Title;
        assert_eq!(json(&p, None)["name"], "Take Care");

        p.settings.header = Header::Year;
        assert_eq!(json(&p, None)["name"], "2011");
    }

    #[test]
    fn a_header_field_this_track_lacks_stays_blank() {
        let mut p = presence();
        p.album = None;
        p.settings.header = Header::Album;
        assert_eq!(
            json(&p, None)["name"],
            " ",
            "a missing album must not put the app name back"
        );
    }

    #[test]
    fn end_span_present_only_with_known_duration() {
        let known = presence();
        assert_eq!(spans(&known), (1000, Some(1180)));

        let unknown = Presence {
            duration: Duration::ZERO,
            ..known
        };
        assert_eq!(spans(&unknown), (1000, None));
    }
}
