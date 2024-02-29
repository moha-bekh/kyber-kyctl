pub struct AudioCompensator {
    sample_size: usize,
    frac: u32,
    incr: u64,
    remaining: u32,
}

impl AudioCompensator {
    pub fn new(sample_size: usize) -> Self {
        Self {
            sample_size,
            frac: 1 << 31,
            incr: 1 << 32,
            remaining: 0,
        }
    }

    pub fn set_compensation(&mut self, sample_delta: i32, distance: u32) {
        let incr = if distance == 0 {
            1 << 32
        } else {
            assert!((sample_delta.unsigned_abs()) < distance);
            ((1i64 << 32) - ((sample_delta as i64) << 32) / (distance as i64)) as u64
        };

        self.frac = 1 << 31; // middle of a single item
        self.incr = incr;
        self.remaining = distance;
    }

    pub fn filter(&mut self, dst: &mut [u8], src: &[u8]) -> usize {
        assert!(src.len() % self.sample_size == 0);

        let sample_size = self.sample_size;
        let src_samples = src.len() / sample_size;
        let dst_samples = dst.len() / sample_size;

        let mut i = 0;
        let mut src_index = self.frac as u64;

        if i < self.remaining as usize && self.incr != (1 << 32) {
            while i < dst_samples && i < self.remaining as usize {
                let src_idx = (src_index >> 32) as usize;
                if src_idx >= src_samples {
                    break;
                }
                dst[i * sample_size..(i + 1) * sample_size]
                    .copy_from_slice(&src[src_idx * sample_size..(src_idx + 1) * sample_size]);
                src_index += self.incr;
                i += 1;
            }

            self.remaining -= std::cmp::min(self.remaining, i as u32);
            self.frac = (src_index & 0xFFFFFFFF) as u32;
        }

        if i < dst_samples {
            let src_idx = (src_index >> 32) as usize;
            if src_idx < src_samples {
                let src_remaining = src_samples - src_idx;
                let dst_remaining = dst_samples - i;
                let len = std::cmp::min(src_remaining, dst_remaining);
                dst[i * sample_size..(i + len) * sample_size]
                    .copy_from_slice(&src[src_idx * sample_size..(src_idx + len) * sample_size]);
                i += len;
            }
        }

        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_compensation() {
        let mut x = AudioCompensator::new(8);
        x.set_compensation(0, 0);
        let src = (0..48000u32 * 8)
            .map(|i| (i % 100) as u8)
            .collect::<Vec<u8>>();
        let mut dst = vec![0; 50000 * 8];
        let len = x.filter(&mut dst, &src);
        assert_eq!(len, 48000);
        assert!(&src == &dst[..src.len()]);
    }

    #[test]
    fn single_pass_positive_compensation() {
        let mut x = AudioCompensator::new(8);
        let src = vec![42; 48000 * 8];
        let mut dst = vec![0; 50000 * 8];
        x.set_compensation(32, 48000);
        let len = x.filter(&mut dst, &src);
        assert_eq!(len, 48032);
        assert!(&dst[..len * 8] == vec![42; len * 8]);
    }

    #[test]
    fn single_pass_negative_compensation() {
        let mut x = AudioCompensator::new(8);
        let src = vec![42; 48000 * 8];
        let mut dst = vec![0; 50000 * 8];
        x.set_compensation(-20, 48000);
        let len = x.filter(&mut dst, &src);
        assert_eq!(len, 47980);
        assert!(&dst[..len * 8] == vec![42; len * 8]);
    }

    #[test]
    fn multi_pass_positive_compensation() {
        let mut x = AudioCompensator::new(8);
        let src = vec![42; 48000 * 8];
        let mut dst = vec![0; 50000 * 8];
        x.set_compensation(50, 48000);

        let mut dst_idx = 0;
        for i in 0..200 {
            // 200 packets of 240 src samples
            let packet = &src[i * 200 * 8..(i * 200 + 240) * 8];
            let written = x.filter(&mut dst[dst_idx * 8..], &packet);
            dst_idx += written;
        }
        let len = dst_idx;
        assert_eq!(len, 48050);
        assert!(&dst[..len * 8] == vec![42; len * 8]);
    }

    #[test]
    fn multi_pass_negative_compensation() {
        let mut x = AudioCompensator::new(8);
        let src = vec![42; 48000 * 8];
        let mut dst = vec![0; 50000 * 8];
        x.set_compensation(-150, 48000);

        let mut dst_idx = 0;
        for i in 0..200 {
            // 200 packets of 240 src samples
            let packet = &src[i * 200 * 8..(i * 200 + 240) * 8];
            let written = x.filter(&mut dst[dst_idx * 8..], &packet);
            dst_idx += written;
        }
        let len = dst_idx;
        assert_eq!(len, 47850);
        assert!(&dst[..len * 8] == vec![42; len * 8]);
    }

    #[test]
    fn single_pass_fast_compensation() {
        let mut x = AudioCompensator::new(8);
        let src = vec![42; 48000 * 8];
        let mut dst = vec![0; 50000 * 8];
        x.set_compensation(32, 1000);
        let len = x.filter(&mut dst, &src);
        assert_eq!(len, 48032);
        assert!(&dst[..len * 8] == vec![42; len * 8]);
    }
}
