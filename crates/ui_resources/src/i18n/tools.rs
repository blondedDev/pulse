use gpui::SharedString;

use super::fill;

pub struct ToolsStrings {
    pub ai_prompt_size_t: SharedString,
    pub ai_answer_created_t: SharedString,
    pub covers_searching_t: SharedString,
    pub covers_found_t: SharedString,
    pub covers_apply_t: SharedString,
    pub covers_applying_t: SharedString,
    pub covers_summary_t: SharedString,
    pub covers_skip_cover_exists_t: SharedString,
    pub covers_skip_image_exists_t: SharedString,
    pub covers_skip_search_failed_t: SharedString,
    pub covers_skip_download_failed_t: SharedString,
    pub covers_skip_write_failed_t: SharedString,
    pub tools_timer: SharedString,
    pub timer_off: SharedString,
    pub timer_pauses_at_t: SharedString,
    pub timer_pauses_after_track: SharedString,
    pub timer_auto_started: SharedString,
    pub timer_start: SharedString,
    pub timer_minutes_t: SharedString,
    pub timer_end_of_track: SharedString,
    pub timer_extend_t: SharedString,
    pub timer_cancel: SharedString,
    pub timer_fade: SharedString,
    pub timer_fade_desc: SharedString,
    pub timer_auto: SharedString,
    pub timer_auto_desc: SharedString,
    pub timer_auto_duration: SharedString,
    pub timer_seconds_t: SharedString,
    pub timer_minutes_unit: SharedString,
    pub timer_run: SharedString,
}

impl ToolsStrings {
    pub fn ai_prompt_size(&self, chars: usize) -> String {
        fill(&self.ai_prompt_size_t, &[&chars.to_string()])
    }

    pub fn ai_answer_created(&self, name: &str, found: usize, total: usize) -> String {
        fill(
            &self.ai_answer_created_t,
            &[name, &found.to_string(), &total.to_string()],
        )
    }

    pub fn covers_searching(&self, done: usize, total: usize) -> String {
        fill(
            &self.covers_searching_t,
            &[&done.to_string(), &total.to_string()],
        )
    }

    pub fn covers_found(&self, found: usize, total: usize) -> String {
        fill(
            &self.covers_found_t,
            &[&found.to_string(), &total.to_string()],
        )
    }

    pub fn covers_apply(&self, count: usize) -> String {
        fill(&self.covers_apply_t, &[&count.to_string()])
    }

    pub fn covers_applying(&self, done: usize, total: usize) -> String {
        fill(
            &self.covers_applying_t,
            &[&done.to_string(), &total.to_string()],
        )
    }

    pub fn covers_summary(&self, found: usize, total: usize, written: usize) -> String {
        fill(
            &self.covers_summary_t,
            &[&found.to_string(), &total.to_string(), &written.to_string()],
        )
    }

    pub fn covers_skip_cover_exists(&self, name: &str) -> String {
        fill(&self.covers_skip_cover_exists_t, &[name])
    }

    pub fn covers_skip_image_exists(&self, name: &str) -> String {
        fill(&self.covers_skip_image_exists_t, &[name])
    }

    pub fn covers_skip_search_failed(&self, error: &str) -> String {
        fill(&self.covers_skip_search_failed_t, &[error])
    }

    pub fn covers_skip_download_failed(&self, error: &str) -> String {
        fill(&self.covers_skip_download_failed_t, &[error])
    }

    pub fn covers_skip_write_failed(&self, error: &str) -> String {
        fill(&self.covers_skip_write_failed_t, &[error])
    }

    pub fn timer_pauses_at(&self, clock: &str) -> String {
        fill(&self.timer_pauses_at_t, &[clock])
    }

    pub fn timer_minutes(&self, minutes: u32) -> String {
        fill(&self.timer_minutes_t, &[&minutes.to_string()])
    }

    pub fn timer_extend(&self, minutes: u32) -> String {
        fill(&self.timer_extend_t, &[&minutes.to_string()])
    }

    pub fn timer_seconds(&self, seconds: u32) -> String {
        fill(&self.timer_seconds_t, &[&seconds.to_string()])
    }
}

pub fn tools_strings() -> &'static ToolsStrings {
    &EN
}

static EN: ToolsStrings = ToolsStrings {
    ai_prompt_size_t: SharedString::new_static("{} characters"),
    ai_answer_created_t: SharedString::new_static("Created playlist “{}”: {} of {} tracks"),
    covers_searching_t: SharedString::new_static("Searching: {} of {}"),
    covers_found_t: SharedString::new_static("Found {} of {}"),
    covers_apply_t: SharedString::new_static("Save selected ({})"),
    covers_applying_t: SharedString::new_static("Saving: {} of {}"),
    covers_summary_t: SharedString::new_static("Found {} of {}, saved {}"),
    covers_skip_cover_exists_t: SharedString::new_static("{} is already there"),
    covers_skip_image_exists_t: SharedString::new_static("the folder already has an image: {}"),
    covers_skip_search_failed_t: SharedString::new_static("search failed: {}"),
    covers_skip_download_failed_t: SharedString::new_static("download failed: {}"),
    covers_skip_write_failed_t: SharedString::new_static("could not save: {}"),
    tools_timer: SharedString::new_static("Sleep timer"),
    timer_off: SharedString::new_static("Off"),
    timer_pauses_at_t: SharedString::new_static("Pauses at {}"),
    timer_pauses_after_track: SharedString::new_static("Pauses when the current track ends"),
    timer_auto_started: SharedString::new_static("started automatically"),
    timer_start: SharedString::new_static("Pause after"),
    timer_minutes_t: SharedString::new_static("{} min"),
    timer_end_of_track: SharedString::new_static("End of track"),
    timer_extend_t: SharedString::new_static("+{} min"),
    timer_cancel: SharedString::new_static("Turn off"),
    timer_fade: SharedString::new_static("Fade out"),
    timer_fade_desc: SharedString::new_static(
        "Lower the volume gradually before the pause. Not available in exclusive mode.",
    ),
    timer_auto: SharedString::new_static("Automatic sleep timer"),
    timer_auto_desc: SharedString::new_static(
        "Starts the timer by itself when music plays during these hours. You can also start it by hand in Tools.",
    ),
    timer_auto_duration: SharedString::new_static("Duration"),
    timer_seconds_t: SharedString::new_static("{} s"),
    timer_minutes_unit: SharedString::new_static("min"),
    timer_run: SharedString::new_static("Start"),
};
