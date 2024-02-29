use average::Average;
use buffer::AudioBuffer;
use shared_memory::SharedMemory;

use compensator::AudioCompensator;
#[allow(unused)]
use log::{debug, error, info, warn};
use std::rc::Rc;
use std::time::Duration;
use wasmtimer::std::Instant;

pub(crate) mod average;
pub(crate) mod buffer;
pub(crate) mod compensator;
pub(crate) mod mutex;
pub(crate) mod shared_memory;

pub(crate) struct AudioRegulator {
    memory: Rc<SharedMemory>,
    buf: AudioBuffer,
    compensator: AudioCompensator,
    avg_buffering: Average,
    sample_rate: u32,
    sample_size: u32,
    target_buffering: u32,
    samples_since_resync: u32,
    compensation: i32,
    tmp: Vec<u8>,

    next_expected_push: Option<Instant>,
}

impl AudioRegulator {
    pub(crate) fn new(
        sab: js_sys::SharedArrayBuffer,
        sample_rate: u32,
        sample_size: u32,
        capacity: u32,
        target_buffering: u32,
    ) -> Self {
        let memory = SharedMemory::from(sab, sample_size, capacity);
        let memory = Rc::new(memory);
        let buf = AudioBuffer::new(memory.clone());
        let compensator = AudioCompensator::new(sample_size as usize);
        let avg_buffering = Average::new(128);
        let tmp = vec![0; 16 * 1024 * sample_size as usize];
        Self {
            memory,
            buf,
            compensator,
            avg_buffering,
            sample_rate,
            sample_size,
            target_buffering,
            samples_since_resync: 0,
            compensation: 0,
            tmp,
            next_expected_push: None,
        }
    }

    pub(crate) fn push(&mut self, data: web_sys::AudioData) {
        let samples = data.number_of_frames();
        let bytes = samples * self.memory.sample_size;
        let mut tmp = vec![0u8; bytes as usize];
        data.copy_to_with_u8_array(
            &mut tmp,
            web_sys::AudioDataCopyToOptions::new(0)
                .format(web_sys::AudioSampleFormat::F32)
                .frame_count(data.number_of_frames()),
        );

        self.push_samples(&tmp);
    }

    fn push_samples(&mut self, data: &[u8]) {
        let sample_size = self.sample_size;
        assert!(data.len() % sample_size as usize == 0);
        let input_samples = (data.len() / sample_size as usize) as u32;

        let now = Instant::now();
        if let Some(next_expected_push) = self.next_expected_push {
            // Instant::duration_since() does not support now < next_expected_push
            if now > next_expected_push
                && now.duration_since(next_expected_push) > Duration::from_millis(200)
            {
                // Consider it is a discontinuity (typically because silence packets
                // were not captured/transmitted)
                let can_read = self.buf.can_read();
                if input_samples + can_read < self.target_buffering {
                    let silence = self.target_buffering - can_read - input_samples;
                    self.buf.write_silence(silence);
                }

                // Reset state
                self.avg_buffering.avg = self.target_buffering as f32;
                self.compensator.set_compensation(0, 0);
                self.compensation = 0;
                self.samples_since_resync = 0;
                self.memory.reset_underflow();
            }
        }

        let packet_duration =
            Duration::from_micros(input_samples as u64 * 1_000_000 / self.sample_rate as u64);
        self.next_expected_push = Some(now + packet_duration);

        let mut samples: u32 = self
            .compensator
            .filter(&mut self.tmp, data)
            .try_into()
            .unwrap();
        let mut packet = &self.tmp[..(samples * sample_size) as usize];

        let cap = self.buf.capacity();
        if samples > cap {
            // Very very unlikely: a single resampled frame should never
            // exceed the audio buffer size (or something is very wrong).
            // Ignore the first bytes anyway.
            packet = &self.tmp[((samples - cap) * sample_size) as usize..];
            samples = cap;
        }

        let mut skipped_samples = 0;

        let mut written = self.buf.write(packet);
        if written < samples {
            let mut remaining = samples - written;

            self.memory.mutex.lock();

            // Retry with the lock
            written += self.buf.write(
                &packet[(written * sample_size) as usize
                    ..((written + remaining) * sample_size) as usize],
            );

            if written < samples {
                remaining = samples - written;
                // Still insufficient, drop old samples to make space
                skipped_samples = self.buf.drop(remaining);
                assert_eq!(skipped_samples, remaining);

                // Now there is enough space
                let w = self.buf.write(
                    &packet[(written * sample_size) as usize
                        ..((written + remaining) * sample_size) as usize],
                );
                assert_eq!(w, remaining);
            }

            self.memory.mutex.unlock();
        }

        let mut underflow = 0;
        let played = self.memory.get_played();
        let max_buffered_samples = if played {
            underflow = self.memory.consume_underflow();
            self.target_buffering * 11 / 10 + 60 * 48
        } else {
            // Playback not started yet, do not accumulate too much, this would
            // cause unnecessary delay (and glitches to compensate) on start
            self.target_buffering + 10 * 48
        };

        let mut can_read = self.buf.can_read();
        if can_read > max_buffered_samples {
            let mut skip_samples = 0;

            self.memory.mutex.lock();

            can_read = self.buf.can_read();
            if can_read > max_buffered_samples {
                skip_samples = can_read - max_buffered_samples;
                let r = self.buf.drop(skip_samples);
                assert_eq!(r, skip_samples);
                skipped_samples += skip_samples;
            }

            self.memory.mutex.unlock();

            if skip_samples > 0 {
                if played {
                    debug!("[Audio] Buffering threshold exceeded, skipping {skip_samples} samples");
                } else {
                    debug!("[Audio] Playback not started, skipping {skip_samples} samples");
                }
            }
        }

        self.memory.set_received(true);

        if played {
            if underflow > 0 {
                debug!("[Audio] Buffer underflow: {underflow} samples");
            }

            let instant_compensation = written as i32 - input_samples as i32;
            let _inserted_silence = underflow as i32;
            let _dropped = skipped_samples as i32;

            // The compensation must apply instantly, it must not be smoothed
            //self.avg_buffering.avg += (instant_compensation + inserted_silence - dropped) as f32;
            self.avg_buffering.avg += instant_compensation as f32;
            if self.avg_buffering.avg < 0.0 {
                // Since dropping samples instantly reduces buffering, the
                // difference is applied immediately to the average value,
                // assuming that the delay between the producer and the
                // consumer will be caught up.
                //
                // However, when this assumption is not valid, the average
                // buffering may decrease indefinitely. Prevent it to become
                // negative to limit the consequences.
                self.avg_buffering.avg = 0.0;
            }

            // However, the buffering level must be smoothed
            self.avg_buffering.push(can_read as f32);

            //debug!(
            //    "[Audio] can_read={can_read} avg_buffering={}",
            //    self.avg_buffering.avg
            //);

            self.samples_since_resync += written;
            if self.samples_since_resync >= self.sample_rate {
                // Recompute compensation every second
                self.samples_since_resync = 0;

                let mut diff = self.target_buffering as i32 - self.avg_buffering.avg as i32;
                let threshold = if self.compensation != 0 {
                    self.sample_rate / 1000 // 1ms
                } else {
                    self.sample_rate * 4 / 1000 // 4ms
                };

                if diff.unsigned_abs() < threshold || diff < 0 && can_read < self.target_buffering {
                    // Do not compensate for small values, the error is just noise.
                    // Do not accelerate if the instant buffering level is
                    // below the target, this would increase underflow.
                    diff = 0;
                }

                // Compensate the diff over 4 seconds (but will be recomputed after 1 second)
                let distance = 4 * self.sample_rate;
                // Limit compensation rate to 2%
                let abs_max_diff = distance as i32 / 50;
                let diff = std::cmp::max(-abs_max_diff, std::cmp::min(abs_max_diff, diff));
                info!(
                    "[Audio] Buffering: target={} avg={} cur={} compensation={}",
                    self.target_buffering, self.avg_buffering.avg, can_read, diff
                );

                if diff != self.compensation {
                    self.compensator.set_compensation(diff, distance);
                    self.compensation = diff;
                }
            }
        }
    }
}
