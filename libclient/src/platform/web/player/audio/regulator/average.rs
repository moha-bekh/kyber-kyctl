pub(crate) struct Average {
    pub(crate) avg: f32,
    range: u32,
    count: u32,
}

impl Average {
    pub(crate) fn new(range: u32) -> Self {
        Self {
            avg: 0f32,
            range,
            count: 0,
        }
    }

    pub(crate) fn push(&mut self, value: f32) {
        if self.count < self.range {
            self.count += 1;
        }

        self.avg = ((self.count - 1) as f32 * self.avg + value) / self.count as f32;
    }
}
