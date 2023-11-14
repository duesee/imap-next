#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]
pub mod client;
mod receive;
mod send;
pub mod server;
pub mod stream;

pub mod auth {
    use imap_codec::imap_types::secret::Secret;

    #[derive(Debug)]
    pub enum Authenticate {
        Plain {
            username: Vec<u8>,
            password: Secret<Vec<u8>>,
        },
    }
}
