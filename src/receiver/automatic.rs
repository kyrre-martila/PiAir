//! Retry timing is independent of detection thresholds and RTP scheduling.
use std::time::{Duration, Instant};
pub(super) struct Retry {
    at: Instant,
    delay: u64,
}
impl Retry {
    pub fn new(now: Instant) -> Self {
        Self { at: now, delay: 2 }
    }
    pub fn ready(&self, now: Instant, meaningful: bool) -> bool {
        meaningful && now >= self.at
    }
    pub fn failed(&mut self, now: Instant) -> u64 {
        let delay = self.delay;
        self.at = now + Duration::from_secs(delay);
        self.delay = (delay * 2).min(60);
        delay
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authentication_and_connection_errors_back_off_and_silence_never_redials() {
        for _failure in ["authentication", "connection", "disconnect"] {
            let start = Instant::now();
            let mut r = Retry::new(start);
            let mut attempts = 0;
            for ms in 0..600000 {
                let now = start + Duration::from_millis(ms);
                assert!(!r.ready(now, false));
                if r.ready(now, true) {
                    attempts += 1;
                    let delay = r.failed(now);
                    assert!((2..=60).contains(&delay));
                    assert!(!r.ready(now, true));
                }
            }
            assert!(attempts < 16);
            assert!(r.ready(start + Duration::from_secs(1000), true));
        }
    }
}
