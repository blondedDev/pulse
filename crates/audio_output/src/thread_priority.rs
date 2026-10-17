pub use imp::RenderThreadPriority;

mod imp {
    pub struct RenderThreadPriority;

    impl RenderThreadPriority {
        pub fn acquire(_buffer_frames: u32, _sample_rate: u32) -> Self {
            Self
        }
    }
}

pub fn boost_decoder_thread() {
    const DECODER_NICE: i32 = -5;
    let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, DECODER_NICE) };
    if rc != 0 {
        log::debug!("audio: decoder thread niceness unchanged (needs RLIMIT_NICE)");
    }
}
