/**
 * Clock estimator
 *
 * This clock computes an exponentially weighted moving average (EWMA) of the
 * offset between the stream time (PTS) and the system time of received frames.
 *
 * The estimation is then used to compute the target system time a frame with a
 * certain PTS should be displayed.
 */

pub(crate) struct Clock {
    pub range: usize,
    pub offset: u64,
}

impl Clock {
    const RANGE: usize = 32;

    pub fn new() -> Self {
        Self {
            range: 0,
            offset: 0,
        }
    }

    pub fn update(&mut self, system: u64, stream: u64) {
        if self.range < Self::RANGE {
            self.range += 1
        }

        let offset = system - stream;
        self.offset = ((self.range as u64 - 1) * self.offset + offset) / self.range as u64;
    }

    pub fn to_system_time(&self, stream: u64) -> u64 {
        assert!(self.range > 0);
        stream + self.offset
    }
}
