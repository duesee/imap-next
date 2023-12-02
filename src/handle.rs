use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Handle {
    generator_id: u64,
    handle_id: u64,
}

impl Handle {
    pub fn generator_id(&self) -> u64 {
        self.generator_id
    }

    pub fn handle_id(&self) -> u64 {
        self.handle_id
    }
}

#[derive(Debug)]
pub struct HandleGenerator {
    /// This ID is used to bind the handles to the generator instance, i.e. it's possible to
    /// distinguish handles generated by different generators. We hope that this might
    /// prevent bugs when the library user is dealing with handles from different sources.
    generator_id: u64,
    next_handle_id: u64,
}

impl HandleGenerator {
    pub fn new(next_generator_id: &AtomicU64) -> Self {
        // There is no synchronization required and we only care about each thread seeing a
        // unique value.
        let generator_id = next_generator_id.fetch_add(1, Ordering::Relaxed);

        Self {
            generator_id,
            next_handle_id: 0,
        }
    }

    pub fn generate(&mut self) -> Handle {
        let handle_id = self.next_handle_id;
        self.next_handle_id += self.next_handle_id.wrapping_add(1);

        Handle {
            generator_id: self.generator_id,
            handle_id,
        }
    }
}
