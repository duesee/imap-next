use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct RawHandle {
    generator_id: u64,
    handle_id: u64,
}

impl RawHandle {
    pub fn generator_id(&self) -> u64 {
        self.generator_id
    }

    pub fn handle_id(&self) -> u64 {
        self.handle_id
    }
}

#[derive(Debug)]
pub struct HandleGenerator<H> {
    create_handle: fn(RawHandle) -> H,
    /// This ID is used to bind the handles to the generator instance, i.e. it's possible to
    /// distinguish handles generated by different generators. We hope that this might
    /// prevent bugs when the library user is dealing with handles from different sources.
    generator_id: u64,
    next_handle_id: u64,
}

impl<H> HandleGenerator<H> {
    pub fn generate(&mut self) -> H {
        let handle_id = self.next_handle_id;
        self.next_handle_id += self.next_handle_id.wrapping_add(1);

        let create_handle = self.create_handle;

        create_handle(RawHandle {
            generator_id: self.generator_id,
            handle_id,
        })
    }
}

#[derive(Debug)]
pub struct HandleGeneratorGenerator<H> {
    create_handle: fn(RawHandle) -> H,
    next_handle_generator_id: AtomicU64,
}

impl<H> HandleGeneratorGenerator<H> {
    pub const fn new(create_handle: fn(RawHandle) -> H) -> Self {
        Self {
            create_handle,
            next_handle_generator_id: AtomicU64::new(0),
        }
    }

    pub fn generate(&self) -> HandleGenerator<H> {
        // There is no synchronization required and we only care about each thread seeing a
        // unique value.
        let generator_id = self
            .next_handle_generator_id
            .fetch_add(1, Ordering::Relaxed);

        HandleGenerator {
            create_handle: self.create_handle,
            generator_id,
            next_handle_id: 0,
        }
    }
}
