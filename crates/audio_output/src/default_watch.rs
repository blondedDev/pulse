use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct DefaultDeviceWatcher {
    changed: Arc<AtomicBool>,
    _inner: platform::Inner,
}

impl DefaultDeviceWatcher {
    pub(crate) fn start() -> Option<Self> {
        let changed = Arc::new(AtomicBool::new(false));
        match platform::Inner::start(changed.clone()) {
            Ok(inner) => Some(Self {
                changed,
                _inner: inner,
            }),
            Err(e) => {
                log::warn!("audio output: can't watch the system default device: {e}");
                None
            }
        }
    }

    pub(crate) fn take_changed(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }
}

mod platform {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use parking_lot::Mutex;

    use crate::device::pulse_default_sink;

    pub(super) struct Inner {
        child: Mutex<Child>,
    }

    impl Inner {
        pub(super) fn start(changed: Arc<AtomicBool>) -> Result<Self, String> {
            let mut child = Command::new("pactl")
                .arg("subscribe")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| format!("pactl subscribe: {e}"))?;
            let stdout = child.stdout.take().ok_or("pactl subscribe: no stdout")?;
            let mut last = pulse_default_sink();
            std::thread::Builder::new()
                .name("audio-default-watch".into())
                .spawn(move || {
                    for line in BufReader::new(stdout).lines() {
                        let Ok(line) = line else { break };
                        if !line.contains("on server") {
                            continue;
                        }
                        let current = pulse_default_sink();
                        if current.is_some() && current != last {
                            last = current;
                            changed.store(true, Ordering::Release);
                        }
                    }
                })
                .map_err(|e| format!("spawn watcher thread: {e}"))?;
            Ok(Self {
                child: Mutex::new(child),
            })
        }
    }

    impl Drop for Inner {
        fn drop(&mut self) {
            let child = self.child.get_mut();
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
