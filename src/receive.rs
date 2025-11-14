use imap_codec::{
    decode::Decoder,
    fragmentizer::{
        DecodeMessageError, FragmentInfo, Fragmentizer, LineEnding, LiteralAnnouncement,
    },
    imap_types::{
        core::{LiteralMode, Tag},
        secret::Secret,
        IntoStatic,
    },
};

use crate::{fragment::read_fragment, Interrupt, Io};

pub struct ReceiveState {
    crlf_relaxed: bool,
    fragmentizer: Fragmentizer,
    message_has_invalid_line_ending: bool,
}

impl ReceiveState {
    pub fn new(crlf_relaxed: bool, max_message_size: Option<u32>) -> Self {
        let fragmentizer = match max_message_size {
            Some(max_message_size) => Fragmentizer::new(max_message_size),
            None => Fragmentizer::without_max_message_size(),
        };

        Self {
            crlf_relaxed,
            fragmentizer,
            message_has_invalid_line_ending: false,
        }
    }

    pub fn enqueue_input(&mut self, bytes: &[u8]) {
        self.fragmentizer.enqueue_bytes(bytes);
    }

    /// Discard the current message immediately without receiving it completely.
    ///
    /// This operation is dangerous because the next message might start in untrusted bytes.
    /// You should only use it if you can reasonably assume that you won't receive the remaining
    /// bytes of the message, e.g. the server rejected the literal of message.
    pub fn discard_message(&mut self) -> Secret<Box<[u8]>> {
        let discarded_bytes = Secret::new(self.fragmentizer.message_bytes().into());
        self.fragmentizer.skip_message();
        discarded_bytes
    }

    /// Discard the current message once it will be received completely.
    ///
    /// This operation is safe because it ensures that next message will start at a sane point.
    /// To achieve this the fragments of the current message will be parsed until the end of the
    /// message. Then the message will be discarded without being decoded.
    pub fn poison_message(&mut self) {
        self.fragmentizer.poison_message();
    }

    /// Tries to decode the tag of the current message before it was received completely.
    pub fn message_tag(&self) -> Option<Tag<'static>> {
        let tag = self.fragmentizer.decode_tag()?;
        Some(tag.into_static())
    }

    pub fn next<C>(&mut self, codec: &C) -> Result<ReceiveEvent<C>, Interrupt<ReceiveError>>
    where
        C: Decoder,
        for<'a> C::Message<'a>: IntoStatic<Static = C::Message<'static>>,
    {
        loop {
            // Parse the next fragment
            let fragment_info = read_fragment(&mut self.fragmentizer);

            // We only need to handle line fragments
            match fragment_info {
                Some(FragmentInfo::Line {
                    announcement,
                    ending,
                    ..
                }) => {
                    // Check for line ending compatibility
                    if !self.crlf_relaxed && ending == LineEnding::Lf {
                        self.fragmentizer.poison_message();
                        self.message_has_invalid_line_ending = true;
                    }

                    match announcement {
                        Some(LiteralAnnouncement { mode, length }) => {
                            // The line announces a literal, allow the caller to handle it
                            return Ok(ReceiveEvent::LiteralAnnouncement { mode, length });
                        }
                        None => {
                            // The message is now complete and can be decoded
                            let result = match self.fragmentizer.decode_message(codec) {
                                Ok(message) => {
                                    Ok(ReceiveEvent::DecodingSuccess(message.into_static()))
                                }
                                Err(DecodeMessageError::DecodingFailure(_)) => {
                                    let discarded_bytes =
                                        Secret::new(self.fragmentizer.message_bytes().into());
                                    Err(Interrupt::Error(ReceiveError::DecodingFailure {
                                        discarded_bytes,
                                    }))
                                }
                                Err(DecodeMessageError::DecodingRemainder { .. }) => {
                                    let discarded_bytes =
                                        Secret::new(self.fragmentizer.message_bytes().into());
                                    Err(Interrupt::Error(ReceiveError::DecodingFailure {
                                        discarded_bytes,
                                    }))
                                }
                                Err(DecodeMessageError::MessageTooLong { .. }) => {
                                    let discarded_bytes =
                                        Secret::new(self.fragmentizer.message_bytes().into());
                                    Err(Interrupt::Error(ReceiveError::MessageTooLong {
                                        discarded_bytes,
                                    }))
                                }
                                Err(DecodeMessageError::MessagePoisoned { .. }) => {
                                    let discarded_bytes =
                                        Secret::new(self.fragmentizer.message_bytes().into());

                                    if self.message_has_invalid_line_ending {
                                        Err(Interrupt::Error(ReceiveError::ExpectedCrlfGotLf {
                                            discarded_bytes,
                                        }))
                                    } else {
                                        Err(Interrupt::Error(ReceiveError::MessageIsPoisoned {
                                            discarded_bytes,
                                        }))
                                    }
                                }
                            };

                            self.message_has_invalid_line_ending = false;
                            return result;
                        }
                    }
                }
                Some(FragmentInfo::Literal { .. }) => {
                    // We don't need to handle literal fragments
                    continue;
                }
                None => {
                    // Not enough bytes for decoding the message, request more bytes
                    return Err(Interrupt::Io(Io::NeedMoreInput));
                }
            }
        }
    }
}

pub enum ReceiveEvent<C: Decoder> {
    DecodingSuccess(C::Message<'static>),
    LiteralAnnouncement { mode: LiteralMode, length: u32 },
}

pub enum ReceiveError {
    DecodingFailure { discarded_bytes: Secret<Box<[u8]>> },
    ExpectedCrlfGotLf { discarded_bytes: Secret<Box<[u8]>> },
    MessageIsPoisoned { discarded_bytes: Secret<Box<[u8]>> },
    MessageTooLong { discarded_bytes: Secret<Box<[u8]>> },
}
