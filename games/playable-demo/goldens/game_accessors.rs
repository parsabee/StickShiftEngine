// Copyright (c) 2026 Parsa Bagheri
//
// This software is released under the Apache-2.0 License.

// Appended to a copy of the playable demo's `game.rs` by `build.rs`, so these sit
// inside its module and can read and set `CarRunner`'s private state. Only the
// probe (`src/probe.rs`) uses them; the game never sees them.

/// The `CarRunner` state `game.rs` keeps private.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Probe {
    pub lane: usize,
    pub travel: f32,
    pub distance: f32,
    pub best: f32,
    pub run_time: f32,
    pub fade_remaining: f32,
    pub rebased: bool,
}

impl CarRunner {
    /// The private state, now.
    pub(crate) fn probe(&self) -> Probe {
        Probe {
            lane: self.lane,
            travel: self.travel,
            distance: self.distance,
            best: self.best,
            run_time: self.run_time,
            fade_remaining: self.fade_remaining,
            rebased: self.rebased_last_step,
        }
    }

    /// Put the car in `lane` at once, as the probe's dodging run does.
    pub(crate) fn set_lane(&mut self, lane: usize) {
        self.lane = lane;
    }

    /// Restart the lane picker from `seed`.
    pub(crate) fn set_seed(&mut self, seed: u64) {
        self.rng = Rng::new(seed);
    }

    /// Set the run clock the speed ramps with.
    pub(crate) fn set_run_time(&mut self, seconds: f32) {
        self.run_time = seconds;
    }

    /// The x of `lane`'s centre.
    pub(crate) fn lane_x_pub(lane: usize) -> f32 {
        Self::lane_x(lane)
    }
}
