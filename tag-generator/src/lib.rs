use std::sync::atomic::{AtomicU64, Ordering};

use imap_types::core::Tag;
#[cfg(not(debug_assertions))]
use rand::distributions::{Alphanumeric, DistString};

static GLOBAL_TAG_GENERATOR_COUNT: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct TagGenerator {
    global: u64,
    counter: u64,
}

impl TagGenerator {
    /// Generate an instance of a `TagGenerator`
    ///
    /// Returns a `TagGenerator` generating tags with a unique prefix.
    #[allow(clippy::new_without_default)]
    pub fn new() -> TagGenerator {
        // There is no synchronization required and we only care about each thread seeing a unique value.
        let global = GLOBAL_TAG_GENERATOR_COUNT.fetch_add(1, Ordering::Relaxed);
        let counter = 0;

        TagGenerator { global, counter }
    }

    /// Generate a unique `Tag`
    ///
    /// The tag has the form `<Instance>.<Counter>.<Random>`, and is guaranteed to be unique and not
    /// guessable ("forward-secure").
    ///
    /// Rational: `Instance` and `Counter` improve IMAP trace readability.
    /// The non-guessable `Random` hampers protocol-confusion attacks (to a limiting extend).
    pub fn generate(&mut self) -> Tag<'static> {
        #[cfg(not(debug_assertions))]
        let inner = {
            let token = Alphanumeric.sample_string(&mut rand::thread_rng(), 8);
            format!("{}.{}.{token}", self.global, self.counter)
        };

        // Minimize randomness lending the library for security analysis.
        #[cfg(debug_assertions)]
        let inner = format!("{}.{}", self.global, self.counter);

        let tag = Tag::unvalidated(inner);
        self.counter = self.counter.wrapping_add(1);
        tag
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, thread, time::Duration};

    use rand::random;

    use super::TagGenerator;

    #[test]
    fn test_generator_generator() {
        const THREADS: usize = 1000;
        const INVOCATIONS: usize = 5;
        const TOTAL_INVOCATIONS: usize = THREADS * INVOCATIONS;

        let (sender, receiver) = std::sync::mpsc::channel();

        thread::scope(|s| {
            for _ in 1..=THREADS {
                let sender = sender.clone();

                s.spawn(move || {
                    let mut generator = TagGenerator::new();
                    thread::sleep(Duration::from_millis(random::<u8>() as u64));

                    for _ in 1..=INVOCATIONS {
                        sender.send(generator.generate()).unwrap();
                    }
                });
            }

            let mut set = BTreeSet::new();

            while set.len() != TOTAL_INVOCATIONS {
                let tag = receiver.recv().unwrap();

                // Make sure insertion worked, i.e., no duplicate was found.
                // Note: `Tag` doesn't implement `Ord` so we insert a `String`.
                assert!(set.insert(tag.as_ref().to_owned()), "duplicate tag found");
            }
        });
    }
}
