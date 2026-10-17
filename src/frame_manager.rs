use sokol::{app as sapp, time as stm};

/// Time without events after which the app goes idle and stops rendering frames. This is enough for most transient
/// (e.g. UI) animations to stop.
const IDLE_TIMEOUT_SECS: f64 = 1.5;

/// Tracks frame timing and decides when the app has been idle long enough to skip rendering.
pub struct FrameManager {
    force_on_hidden: bool,
    last_frame_time: u64,
    last_event_time: u64,
    idle_disabled_count: usize,
    hidden: bool,
    fps: f64,
    frame_count: u64,
    fps_update_timer: f64,
    will_idle: bool,
}

#[allow(dead_code)]
impl FrameManager {
    /// Creates a frame manager. If force_on_hidden is true, the app goes idle as soon as the window is hidden, ignoring
    /// any disable_idle() calls.
    pub fn new(force_on_hidden: bool) -> Self {
        Self {
            force_on_hidden,
            last_frame_time: stm::now(),
            last_event_time: stm::now(),
            idle_disabled_count: 0,
            hidden: false,
            fps: 0.0,
            frame_count: 0,
            fps_update_timer: 0.0,
            will_idle: false,
        }
    }

    /// Any event wakes the app up and restarts the idle timeout.
    pub fn on_event(&mut self, event: &sapp::Event) {
        self.last_event_time = stm::now();
        match event._type {
            sapp::EventType::Iconified => self.hidden = true,
            sapp::EventType::Restored => self.hidden = false,
            _ => {}
        }
    }

    /// Returns the frame delta time, or `None` if the app is idle and the frame should be skipped.Must be called at the
    /// start of a frame callback.
    pub fn begin_frame(&mut self) -> Option<f64> {
        let now = stm::now();
        // We allow one frame to run before idling.
        let next_will_idle = self.should_idle(now);
        if self.will_idle && next_will_idle {
            // Keep the timestamp fresh so the time spent idling is not mistaken for one long frame after the app wakes
            // up.
            self.last_frame_time = now;
            return None;
        }
        self.will_idle = next_will_idle;
        Some(self.update_time(now))
    }

    /// Returns current average fps.
    pub fn fps(&self) -> f64 {
        self.fps
    }

    /// Returns true if the app will go idle on the next frame.
    pub fn will_idle(&self) -> bool {
        self.will_idle
    }

    /// Prevents the app from going idle.
    pub fn disable_idle(&mut self) {
        self.idle_disabled_count += 1;
    }

    /// Allows the app to go idle again. Must be paired with a previous disable_idle().
    pub fn enable_idle(&mut self) {
        assert!(
            self.idle_disabled_count > 0,
            "enable_idle called without matching disable_idle"
        );
        self.idle_disabled_count -= 1;
    }

    fn should_idle(&self, now: u64) -> bool {
        if self.hidden && self.force_on_hidden {
            return true;
        }
        self.idle_disabled_count == 0 && stm::sec(now - self.last_event_time) >= IDLE_TIMEOUT_SECS
    }

    fn update_time(&mut self, now: u64) -> f64 {
        let dt = stm::sec(now - self.last_frame_time);
        self.last_frame_time = now;
        self.frame_count += 1;
        self.fps_update_timer += dt;
        if self.fps_update_timer >= 0.5 {
            self.fps = self.frame_count as f64 / self.fps_update_timer;
            self.frame_count = 0;
            self.fps_update_timer = 0.0;
        }
        dt
    }
}
