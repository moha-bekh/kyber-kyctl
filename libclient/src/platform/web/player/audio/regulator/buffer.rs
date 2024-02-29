use super::SharedMemory;

use std::rc::Rc;

pub(crate) struct AudioBuffer {
    memory: Rc<SharedMemory>,
}

impl AudioBuffer {
    pub(crate) fn new(memory: Rc<SharedMemory>) -> Self {
        Self { memory }
    }

    #[allow(unused)]
    pub(crate) fn read(&self, dst: &mut [u8]) -> u32 {
        let dst_len: u32 = dst.len().try_into().unwrap();
        let alloc_size = self.memory.alloc_size;
        let sample_size = self.memory.sample_size;
        assert!(dst_len % sample_size == 0);

        let tail = self.memory.get_tail();
        let head = self.memory.get_head();

        let can_read = (alloc_size + head - tail) % alloc_size;
        let samples_count = std::cmp::min(dst_len / sample_size, can_read);

        let right_count = std::cmp::min(alloc_size - tail, samples_count);
        let src_slice = self
            .memory
            .data
            .subarray(tail * sample_size, (tail + right_count) * sample_size);
        src_slice.copy_to(&mut dst[..(right_count * sample_size) as usize]);

        if samples_count > right_count {
            let left_count = samples_count - right_count;
            let src_slice = self.memory.data.subarray(0, left_count * sample_size);
            src_slice.copy_to(
                &mut dst
                    [(right_count * sample_size) as usize..(samples_count * sample_size) as usize],
            );
        }

        let new_tail = (tail + samples_count) % alloc_size;
        self.memory.set_tail(new_tail);

        samples_count
    }

    pub(crate) fn drop(&self, samples: u32) -> u32 {
        let alloc_size = self.memory.alloc_size;

        let tail = self.memory.get_tail();
        let head = self.memory.get_head();

        let can_read = (alloc_size + head - tail) % alloc_size;
        let samples_count = std::cmp::min(samples, can_read);

        let new_tail = (tail + samples_count) % alloc_size;
        self.memory.set_tail(new_tail);

        samples_count
    }

    pub(crate) fn write(&self, src: &[u8]) -> u32 {
        let src_len: u32 = src.len().try_into().unwrap();
        let alloc_size = self.memory.alloc_size;
        let sample_size = self.memory.sample_size;
        assert!(src_len % sample_size == 0);

        let tail = self.memory.get_tail();
        let head = self.memory.get_head();

        let can_write = (alloc_size + tail - head - 1) % alloc_size;
        let samples_count = std::cmp::min(src_len / sample_size, can_write);

        let right_count = std::cmp::min(alloc_size - head, samples_count);
        let dst_slice = self
            .memory
            .data
            .subarray(head * sample_size, (head + right_count) * sample_size);
        dst_slice.copy_from(&src[..(right_count * sample_size) as usize]);

        if samples_count > right_count {
            let left_count = samples_count - right_count;
            let dst_slice = self.memory.data.subarray(0, left_count * sample_size);
            dst_slice.copy_from(
                &src[(right_count * sample_size) as usize..(samples_count * sample_size) as usize],
            );
        }

        let new_head = (head + samples_count) % alloc_size;
        self.memory.set_head(new_head);

        samples_count
    }

    pub(crate) fn write_silence(&self, samples: u32) -> u32 {
        let alloc_size = self.memory.alloc_size;
        let sample_size = self.memory.sample_size;

        let tail = self.memory.get_tail();
        let head = self.memory.get_head();

        let can_write = (alloc_size + tail - head - 1) % alloc_size;
        let samples_count = std::cmp::min(samples, can_write);

        let right_count = std::cmp::min(alloc_size - head, samples_count);
        let dst_slice = self
            .memory
            .data
            .subarray(head * sample_size, (head + right_count) * sample_size);
        dst_slice.fill(0, 0, dst_slice.length());

        if samples_count > right_count {
            let left_count = samples_count - right_count;
            let dst_slice = self.memory.data.subarray(0, left_count * sample_size);
            dst_slice.fill(0, 0, dst_slice.length());
        }

        let new_head = (head + samples_count) % alloc_size;
        self.memory.set_head(new_head);

        samples_count
    }

    #[inline(always)]
    pub(crate) fn capacity(&self) -> u32 {
        // The actual capacity is (alloc_size - 1) so that head == tail is
        // non-ambiguous:
        //  - empty: tail == head
        //  - full: ((tail + 1) % alloc_size) == head
        self.memory.alloc_size - 1
    }

    #[inline(always)]
    pub(crate) fn can_read(&self) -> u32 {
        let alloc_size = self.memory.alloc_size;
        let tail = self.memory.get_tail();
        let head = self.memory.get_head();
        (alloc_size + head - tail) % alloc_size
    }
}
