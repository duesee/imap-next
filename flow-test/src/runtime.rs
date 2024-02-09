use std::{future::Future, time::Duration};

use tokio::{join, runtime, time::sleep};

/// Options for creating an instance of `Runtime`.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct RuntimeOptions {
    pub timeout: Option<Duration>,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(1)),
        }
    }
}

/// Allows to execute one or more `Future`s by blocking the current thread.
///
/// We prefer to have single-threaded unit tests because it makes debugging easier.
/// This runtime allows us to execute server and client tasks in parallel on the same
/// thread the test is executed on.
pub struct Runtime {
    timeout: Option<Duration>,
    rt: runtime::Runtime,
}

impl Runtime {
    pub fn new(runtime_options: RuntimeOptions) -> Self {
        let rt = runtime::Builder::new_current_thread()
            .enable_time()
            .enable_io()
            .build()
            .unwrap();

        Runtime {
            timeout: runtime_options.timeout,
            rt,
        }
    }

    pub fn run<T>(&self, future: impl Future<Output = T>) -> T {
        match self.timeout {
            None => self.rt.block_on(future),
            Some(timeout) => self.rt.block_on(async {
                tokio::select! {
                    output = future => output,
                    () = sleep(timeout) => panic!("Timeout reached"),
                }
            }),
        }
    }

    pub fn run2<T1, T2>(
        &self,
        future1: impl Future<Output = T1>,
        future2: impl Future<Output = T2>,
    ) -> (T1, T2) {
        self.run(async { join!(future1, future2) })
    }
}
