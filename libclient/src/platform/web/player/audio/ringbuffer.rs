use std::cmp::min;
use std::mem::size_of;

use wasm_bindgen::JsValue;

const INDEXES_SIZE: u32 = 8;

pub(super) struct RingBuffer {
    _sab: js_sys::SharedArrayBuffer,
    read_ptr: js_sys::Int32Array,
    write_ptr: js_sys::Int32Array,

    capacity: i32,
    data: js_sys::Float32Array,
}

impl RingBuffer {
    pub(super) fn create_shared_array_buffer(size: u32) -> js_sys::SharedArrayBuffer {
        // Use 2 i32 for read/write index
        let buffer_size = size + INDEXES_SIZE + size_of::<f32>() as u32;
        js_sys::SharedArrayBuffer::new(buffer_size)
    }

    pub(super) fn from(sab: js_sys::SharedArrayBuffer) -> Self {
        let read_ptr = js_sys::Int32Array::new_with_byte_offset_and_length(&sab, 0, 1);
        let write_ptr = js_sys::Int32Array::new_with_byte_offset_and_length(&sab, 4, 1);

        let capacity = (sab.byte_length() - INDEXES_SIZE) as i32 / size_of::<f32>() as i32;
        let data = js_sys::Float32Array::new_with_byte_offset_and_length(
            &sab,
            INDEXES_SIZE,
            capacity as _,
        );

        Self {
            _sab: sab,
            read_ptr,
            write_ptr,
            capacity,
            data,
        }
    }

    pub(super) fn write(&self, data: web_sys::AudioData) -> Result<i32, JsValue> {
        let read_ptr = js_sys::Atomics::load(&self.read_ptr, 0)?;
        let write_ptr = js_sys::Atomics::load(&self.write_ptr, 0)?;

        let max_read_size = (write_ptr + self.capacity - read_ptr) % self.capacity;
        let max_write_size = self.capacity - 1 - max_read_size;

        if (write_ptr + 1) % self.capacity == read_ptr {
            return Ok(0);
        }

        // Compute the number of frames to write
        let frames = data.number_of_frames() as i32 * data.number_of_channels() as i32;
        let frames_to_write = min(max_write_size, frames);

        // AudioData API works with u8. Compute the final size as bytes, not
        // f32 count.
        // Interleaved f32: only one plane
        let u8_buffer_size = size_of::<f32>() as i32 * frames_to_write;

        let mut u8_buffer = vec![0u8; u8_buffer_size as _];
        let mut copy_options = web_sys::AudioDataCopyToOptions::new(0);
        copy_options.frame_count(frames_to_write as u32 / data.number_of_channels());

        data.copy_to_with_u8_array(&mut u8_buffer[..], &copy_options);

        // Copy to SharedArrayBuffer
        let f32_buffer = unsafe {
            std::slice::from_raw_parts(u8_buffer.as_ptr() as *const f32, frames_to_write as _)
        };

        let part_one_size = min(self.capacity - write_ptr, frames_to_write);
        let slice = self
            .data
            .subarray(write_ptr as _, (write_ptr + part_one_size) as u32);
        slice.copy_from(&f32_buffer[0..part_one_size as _]);

        let part_two_size = frames_to_write - part_one_size;
        let slice = self.data.subarray(0, part_two_size as _);
        slice.copy_from(&f32_buffer[part_one_size as _..]);

        // Make new write_ptr public
        let new_write_ptr = (write_ptr + frames_to_write) % self.capacity;
        js_sys::Atomics::store(&self.write_ptr, 0, new_write_ptr)?;

        Ok(frames_to_write)
    }
}
