use super::mutex::JSMutex;

pub(crate) struct SharedMemory {
    _sab: js_sys::SharedArrayBuffer,
    // Atomic fields
    pub(crate) fields: js_sys::Int32Array,
    // Circular buffer
    pub(crate) data: js_sys::Uint8Array,

    pub(crate) mutex: JSMutex,
    pub(crate) sample_size: u32,
    pub(crate) alloc_size: u32, // in samples
}

impl SharedMemory {
    #[allow(unused)] // Reimplemented in the AudioWorklet
    const FIELD_MUTEX: u32 = 0;

    const FIELD_HEAD: u32 = 1;
    const FIELD_TAIL: u32 = 2;
    const FIELD_UNDERFLOW: u32 = 3;
    const FIELD_RECEIVED: u32 = 4;
    const FIELD_PLAYED: u32 = 5;

    const FIELDS: u32 = 6;
    const FIELDS_SIZE: u32 = Self::FIELDS * std::mem::size_of::<i32>() as u32;

    pub(crate) fn create_sab(sample_size: u32, capacity: u32) -> js_sys::SharedArrayBuffer {
        // The actual capacity is (alloc_size - 1) so that head == tail is non-ambiguous
        let alloc_size = capacity + 1; // in samples
        let memory_size = Self::FIELDS_SIZE + sample_size * alloc_size;
        js_sys::SharedArrayBuffer::new(memory_size)
    }

    pub(crate) fn from(sab: js_sys::SharedArrayBuffer, sample_size: u32, capacity: u32) -> Self {
        let fields =
            js_sys::Int32Array::new_with_byte_offset_and_length(&sab, 0, Self::FIELDS as _);
        let mutex = JSMutex::new(fields.subarray(0, 1));
        let alloc_size = capacity + 1; // in samples
        let data = js_sys::Uint8Array::new_with_byte_offset_and_length(
            &sab,
            Self::FIELDS_SIZE as _,
            sample_size * alloc_size,
        );
        Self {
            _sab: sab,
            fields,
            data,
            mutex,
            sample_size,
            alloc_size,
        }
    }

    pub(crate) fn get_head(&self) -> u32 {
        js_sys::Atomics::load(&self.fields, Self::FIELD_HEAD)
            .unwrap()
            .try_into()
            .unwrap()
    }

    pub(crate) fn set_head(&self, head: u32) {
        js_sys::Atomics::store(&self.fields, Self::FIELD_HEAD, head.try_into().unwrap()).unwrap();
    }

    pub(crate) fn get_tail(&self) -> u32 {
        js_sys::Atomics::load(&self.fields, Self::FIELD_TAIL)
            .unwrap()
            .try_into()
            .unwrap()
    }

    pub(crate) fn set_tail(&self, tail: u32) {
        js_sys::Atomics::store(&self.fields, Self::FIELD_TAIL, tail.try_into().unwrap()).unwrap();
    }

    pub(crate) fn consume_underflow(&self) -> u32 {
        js_sys::Atomics::exchange(&self.fields, Self::FIELD_UNDERFLOW, 0)
            .unwrap()
            .try_into()
            .unwrap()
    }

    pub(crate) fn reset_underflow(&self) {
        js_sys::Atomics::store(&self.fields, Self::FIELD_UNDERFLOW, 0).unwrap();
    }

    #[allow(unused)] // Reimplemented in the AudioWorklet
    pub(crate) fn add_underflow(&self, underflow: u32) {
        js_sys::Atomics::add(
            &self.fields,
            Self::FIELD_UNDERFLOW,
            underflow.try_into().unwrap(),
        )
        .unwrap();
    }

    #[allow(unused)] // Reimplemented in the AudioWorklet
    pub(crate) fn get_received(&self) -> bool {
        js_sys::Atomics::load(&self.fields, Self::FIELD_RECEIVED).unwrap() != 0
    }

    pub(crate) fn set_received(&self, received: bool) {
        js_sys::Atomics::store(
            &self.fields,
            Self::FIELD_RECEIVED,
            if received { 1 } else { 0 },
        )
        .unwrap();
    }

    pub(crate) fn get_played(&self) -> bool {
        js_sys::Atomics::load(&self.fields, Self::FIELD_PLAYED).unwrap() != 0
    }

    #[allow(unused)] // Reimplemented in the AudioWorklet
    pub(crate) fn set_played(&self, played: bool) {
        js_sys::Atomics::store(&self.fields, Self::FIELD_PLAYED, if played { 1 } else { 0 })
            .unwrap();
    }
}
