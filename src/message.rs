#![allow(unused)]

use std::ops::RangeInclusive;

use imap_codec::decode::Decoder;
use imap_types::core::LiteralMode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Line<'a> {
    pub bytes: &'a [u8],
    pub announcement: Option<LiteralAnnouncement>,
    pub ending: LineEnding,
}

impl Line<'_> {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

/// The character sequence used for ending a line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LiteralAnnouncement {
    pub length: u32,
    pub mode: LiteralMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Literal<'a> {
    pub bytes: &'a [u8],
}

impl Literal<'_> {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message<'a> {
    pub bytes: &'a [u8],
}

impl<'a> Message<'a> {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn decode<C: Decoder>(&self, codec: &C) -> Result<C::Message<'a>, DecodeError<'a, C>> {
        let (remainder, message) = match codec.decode(self.bytes) {
            Ok(res) => res,
            Err(err) => return Err(DecodeError::Error(err)),
        };

        if !remainder.is_empty() {
            return Err(DecodeError::HasRemainder { message, remainder });
        }

        Ok(message)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError<'a, C: Decoder> {
    Error(C::Error<'a>),
    HasRemainder {
        message: C::Message<'a>,
        remainder: &'a [u8],
    },
}

#[derive(Clone, Debug)]
pub struct MessageParser {
    message_start: usize,
    fragment_start: usize,
    next_event: NextEvent,
}

impl MessageParser {
    pub fn new() -> Self {
        Self {
            message_start: 0,
            fragment_start: 0,
            next_event: NextEvent::start_new_line(),
        }
    }

    pub fn reset(&mut self) {
        self.message_start = 0;
        self.fragment_start = 0;
        self.next_event = NextEvent::start_new_line();
    }

    pub fn skip_message(&mut self) {
        self.message_start = self.fragment_start;
        self.next_event = NextEvent::start_new_line();
    }

    pub fn next<'a>(&mut self, bytes: &'a [u8]) -> Option<Event<'a>> {
        match self.next_event {
            NextEvent::Line { seen_bytes_in_line } => self.next_line(bytes, seen_bytes_in_line),
            NextEvent::Literal { length } => self.next_literal(bytes, length),
            NextEvent::Message { message_end } => self.next_message(bytes, message_end),
        }
    }

    fn next_line<'a>(&mut self, bytes: &'a [u8], seen_bytes_in_line: usize) -> Option<Event<'a>> {
        let remaining_bytes = bytes.get(self.fragment_start..)?;

        let Some(line_ending_result) = find_line_ending(remaining_bytes, seen_bytes_in_line) else {
            // There is no full line in the remaining bytes.
            return None;
        };

        // We found a full line.
        let line_length = line_ending_result.lf_position.checked_add(1)?;
        let fragment_end = self.fragment_start.checked_add(line_length)?;
        let line_bytes = remaining_bytes.get(0..line_length)?;

        let announcement = parse_literal_announcement(line_bytes, line_ending_result.ending);

        match announcement {
            Some(LiteralAnnouncement { length, .. }) => {
                // We expect a literal because it was announced at the end of the line.
                self.fragment_start = fragment_end;
                self.next_event = NextEvent::Literal { length };
            }
            None => {
                // The message is complete.
                self.fragment_start = fragment_end;
                self.next_event = NextEvent::Message {
                    message_end: fragment_end,
                };
            }
        }

        Some(Event::Line(Line {
            bytes: line_bytes,
            announcement,
            ending: line_ending_result.ending,
        }))
    }

    fn next_literal<'a>(&mut self, bytes: &'a [u8], literal_length: u32) -> Option<Event<'a>> {
        let remaining_bytes = bytes.get(self.fragment_start..)?;

        if remaining_bytes.len() < literal_length as usize {
            // We haven't enough bytes for the literal.
            return None;
        }

        // We have enough bytes for the literal.
        let fragment_end = self.fragment_start.checked_add(literal_length as usize)?;
        let literal_bytes = &remaining_bytes.get(0..literal_length as usize)?;

        // We expect a line after a literal.
        self.fragment_start = fragment_end;
        self.next_event = NextEvent::start_new_line();

        Some(Event::Literal(Literal {
            bytes: literal_bytes,
        }))
    }

    fn next_message<'a>(&mut self, bytes: &'a [u8], message_end: usize) -> Option<Event<'a>> {
        let message_bytes = bytes.get(self.message_start..message_end)?;

        self.message_start = message_end;
        self.fragment_start = message_end;
        self.next_event = NextEvent::start_new_line();

        Some(Event::Message(Message {
            bytes: message_bytes,
        }))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event<'a> {
    Line(Line<'a>),
    Literal(Literal<'a>),
    Message(Message<'a>),
}

/// Next event that will be emitted...
#[derive(Clone, Copy, Debug)]
enum NextEvent {
    /// ... is a line.
    Line {
        /// How many bytes in the current line do we already have checked?
        /// This is important if we need multiple attempts to read from the underlying
        /// stream before the line is completely received.
        seen_bytes_in_line: usize,
    },
    /// ... is a literal with the given length.
    Literal { length: u32 },
    /// ... is the complete message.
    Message { message_end: usize },
}

impl NextEvent {
    fn start_new_line() -> Self {
        Self::Line {
            seen_bytes_in_line: 0,
        }
    }
}

/// Line ending found.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FindLineEndingResult {
    /// Position of `\n` symbol at the end of the found line.
    lf_position: usize,
    /// The character sequence at the end of the found line.
    ending: LineEnding,
}

/// Finds the line ending (`\n` or `\r\n`) for the current line.
///
/// Parameters:
/// - `remaining_bytes`: The buffer that contains the current line starting at index 0.
/// - `start`: At this index the search for `\n` will start. Note that the `\r` might be located
///    before this index.
fn find_line_ending(remaining_bytes: &[u8], start: usize) -> Option<FindLineEndingResult> {
    // Try to find `\n`
    let lf_position = start
        + remaining_bytes
            .get(start..)?
            .iter()
            .position(|item| *item == b'\n')?;

    // Check if there is a `\r` right before the `\n`
    let ending = if lf_position
        .checked_sub(1)
        .is_some_and(|i| remaining_bytes[i] == b'\r')
    {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };

    Some(FindLineEndingResult {
        lf_position,
        ending,
    })
}

fn parse_literal_announcement(
    line_bytes: &[u8],
    line_ending: LineEnding,
) -> Option<LiteralAnnouncement> {
    // We parse from the end
    let mut i = line_bytes.len().checked_sub(1)?;

    // Skip the line ending
    match line_ending {
        LineEnding::Lf => i = i.checked_sub(1)?,
        LineEnding::CrLf => i = i.checked_sub(2)?,
    }

    // Parse the manatory closing bracket
    if *line_bytes.get(i)? != b'}' {
        return None;
    }
    i = i.checked_sub(1)?;

    // Parse the mandatory length
    let mut digits: u32 = 0;
    let mut length: u32 = 0;
    loop {
        match *line_bytes.get(i)? {
            character @ b'0'..=b'9' => {
                let digit = (character - b'0') as u32;
                let power = u32::checked_pow(10, digits)?;
                let addend = digit.checked_mul(power)?;

                length = length.checked_add(addend)?;
                digits = digits.checked_add(1)?;
                i = i.checked_sub(1)?;
            }
            _ => break,
        }
    }

    // The length must have at least one digit
    if digits == 0 {
        return None;
    }

    // Parse the optional sign indicating non-sync mode
    let mode = if *line_bytes.get(i)? == b'+' {
        i = i.checked_sub(1)?;
        LiteralMode::NonSync
    } else {
        LiteralMode::Sync
    };

    // Parse the mandatory opening bracket
    if *line_bytes.get(i)? != b'{' {
        return None;
    }

    Some(LiteralAnnouncement { length, mode })
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use imap_types::core::LiteralMode;

    use super::{
        find_line_ending, parse_literal_announcement, Event, FindLineEndingResult, Line,
        LineEnding, Literal, LiteralAnnouncement, Message, MessageParser,
    };

    #[test]
    fn find_line_ending_examples() {
        assert_eq!(find_line_ending(b"", 0), None);

        assert_eq!(find_line_ending(b"foo", 0), None);

        assert_eq!(find_line_ending(b"\n", 1), None);

        assert_eq!(
            find_line_ending(b"\n", 0),
            Some(FindLineEndingResult {
                lf_position: 0,
                ending: LineEnding::Lf
            }),
        );

        assert_eq!(
            find_line_ending(b"\r\n", 0),
            Some(FindLineEndingResult {
                lf_position: 1,
                ending: LineEnding::CrLf
            }),
        );

        assert_eq!(
            find_line_ending(b"\r\n", 1),
            Some(FindLineEndingResult {
                lf_position: 1,
                ending: LineEnding::CrLf
            }),
        );

        assert_eq!(find_line_ending(b"\r\n", 2), None,);

        assert_eq!(
            find_line_ending(b"\r\nfoo\r\n", 2),
            Some(FindLineEndingResult {
                lf_position: 6,
                ending: LineEnding::CrLf
            }),
        );

        assert_eq!(
            find_line_ending(b"\r\nfoo\r\nbar", 6),
            Some(FindLineEndingResult {
                lf_position: 6,
                ending: LineEnding::CrLf
            }),
        );

        assert_eq!(find_line_ending(b"\r\nfoo\r\nbar", 7), None);
    }

    #[test]
    fn parse_literal_announcement_examples() {
        assert_eq!(parse_literal_announcement(b"", LineEnding::CrLf), None,);

        assert_eq!(
            parse_literal_announcement(b"{1}\r\n", LineEnding::CrLf),
            Some(LiteralAnnouncement {
                length: 1,
                mode: LiteralMode::Sync
            })
        );

        assert_eq!(
            parse_literal_announcement(b"{1}\n", LineEnding::Lf),
            Some(LiteralAnnouncement {
                length: 1,
                mode: LiteralMode::Sync
            })
        );

        assert_eq!(
            parse_literal_announcement(b"foo {1}\r\n", LineEnding::CrLf),
            Some(LiteralAnnouncement {
                length: 1,
                mode: LiteralMode::Sync
            })
        );

        assert_eq!(
            parse_literal_announcement(b"foo {2} {1}\r\n", LineEnding::CrLf),
            Some(LiteralAnnouncement {
                length: 1,
                mode: LiteralMode::Sync
            })
        );

        assert_eq!(
            parse_literal_announcement(b"foo {1} \r\n", LineEnding::CrLf),
            None
        );

        assert_eq!(
            parse_literal_announcement(b"foo {1} foo\r\n", LineEnding::CrLf),
            None
        );

        assert_eq!(
            parse_literal_announcement(b"foo {1\r\n", LineEnding::CrLf),
            None
        );

        assert_eq!(
            parse_literal_announcement(b"foo 1}\r\n", LineEnding::CrLf),
            None
        );

        assert_eq!(
            parse_literal_announcement(b"foo { 1}\r\n", LineEnding::CrLf),
            None
        );

        assert_eq!(
            parse_literal_announcement(b"foo {{1}\r\n", LineEnding::CrLf),
            Some(LiteralAnnouncement {
                length: 1,
                mode: LiteralMode::Sync
            })
        );

        assert_eq!(
            parse_literal_announcement(b"foo {42}\r\n", LineEnding::CrLf),
            Some(LiteralAnnouncement {
                length: 42,
                mode: LiteralMode::Sync
            })
        );

        assert_eq!(
            parse_literal_announcement(b"foo {+42}\r\n", LineEnding::CrLf),
            Some(LiteralAnnouncement {
                length: 42,
                mode: LiteralMode::NonSync
            })
        );

        assert_eq!(
            parse_literal_announcement(b"foo {++42}\r\n", LineEnding::CrLf),
            None
        );

        assert_eq!(
            parse_literal_announcement(b"foo {-42}\r\n", LineEnding::CrLf),
            None
        );

        assert_eq!(
            parse_literal_announcement(b"foo {4294967295}\r\n", LineEnding::CrLf),
            Some(LiteralAnnouncement {
                length: 4294967295,
                mode: LiteralMode::Sync
            })
        );

        assert_eq!(
            parse_literal_announcement(b"foo {4294967296}\r\n", LineEnding::CrLf),
            None
        );
    }

    #[test]
    fn single_message() {
        let bytes = b"* OK ...\r\n";

        let mut parser = MessageParser::new();

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b"* OK ...\r\n",
                announcement: None,
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Message(Message {
                bytes: b"* OK ...\r\n",
            }))
        );

        assert_eq!(None, parser.next(bytes));
    }

    #[test]
    fn multiple_messages() {
        let bytes = b"A1 OK ...\r\nA2 BAD ...\r\n";

        let mut parser = MessageParser::new();

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b"A1 OK ...\r\n",
                announcement: None,
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Message(Message {
                bytes: b"A1 OK ...\r\n",
            }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b"A2 BAD ...\r\n",
                announcement: None,
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Message(Message {
                bytes: b"A2 BAD ...\r\n",
            }))
        );

        assert_eq!(None, parser.next(bytes));
    }

    #[test]
    fn literal() {
        let bytes = b"A1 LOGIN {5}\r\nABCDE {5}\r\nFGHIJ\r\n";

        let mut parser = MessageParser::new();

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b"A1 LOGIN {5}\r\n",
                announcement: Some(LiteralAnnouncement {
                    length: 5,
                    mode: LiteralMode::Sync
                }),
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Literal(Literal { bytes: b"ABCDE" }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b" {5}\r\n",
                announcement: Some(LiteralAnnouncement {
                    length: 5,
                    mode: LiteralMode::Sync
                }),
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Literal(Literal { bytes: b"FGHIJ" }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b"\r\n",
                announcement: None,
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Message(Message {
                bytes: b"A1 LOGIN {5}\r\nABCDE {5}\r\nFGHIJ\r\n",
            }))
        );

        assert_eq!(parser.next(bytes), None);
    }

    #[test]
    fn skip_message_after_literal_announcement() {
        let bytes = b"A1 LOGIN {5}\r\nA2 OK ...\r\n";

        let mut parser = MessageParser::new();

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b"A1 LOGIN {5}\r\n",
                announcement: Some(LiteralAnnouncement {
                    length: 5,
                    mode: LiteralMode::Sync
                }),
                ending: LineEnding::CrLf,
            }))
        );

        parser.skip_message();

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b"A2 OK ...\r\n",
                announcement: None,
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Message(Message {
                bytes: b"A2 OK ...\r\n",
            }))
        );

        assert_eq!(parser.next(bytes), None);
    }

    #[test]
    fn reset_after_literal_announcement() {
        let bytes = b"A1 LOGIN {5}\r\nABCDE FGHIJ\r\n";

        let mut parser = MessageParser::new();

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b"A1 LOGIN {5}\r\n",
                announcement: Some(LiteralAnnouncement {
                    length: 5,
                    mode: LiteralMode::Sync
                }),
                ending: LineEnding::CrLf,
            }))
        );

        parser.reset();

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b"A1 LOGIN {5}\r\n",
                announcement: Some(LiteralAnnouncement {
                    length: 5,
                    mode: LiteralMode::Sync
                }),
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Literal(Literal { bytes: b"ABCDE" }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Line(Line {
                bytes: b" FGHIJ\r\n",
                announcement: None,
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(
            parser.next(bytes),
            Some(Event::Message(Message {
                bytes: b"A1 LOGIN {5}\r\nABCDE FGHIJ\r\n",
            }))
        );

        assert_eq!(parser.next(bytes), None);
    }

    #[test]
    fn byte_by_byte() {
        let mut input = b"A1 LOGIN {5}\r\nABCDE FGHIJ\r\n".to_vec();
        input.reverse();

        let mut bytes = Vec::new();

        let mut parser = MessageParser::new();

        for _ in 0..14 {
            assert_eq!(parser.next(&bytes), None);
            bytes.push(input.pop().unwrap());
        }

        assert_eq!(
            parser.next(&bytes),
            Some(Event::Line(Line {
                bytes: b"A1 LOGIN {5}\r\n",
                announcement: Some(LiteralAnnouncement {
                    length: 5,
                    mode: LiteralMode::Sync
                }),
                ending: LineEnding::CrLf,
            }))
        );

        for _ in 0..5 {
            assert_eq!(parser.next(&bytes), None);
            bytes.push(input.pop().unwrap());
        }

        assert_eq!(
            parser.next(&bytes),
            Some(Event::Literal(Literal { bytes: b"ABCDE" }))
        );

        for _ in 0..8 {
            assert_eq!(parser.next(&bytes), None);
            bytes.push(input.pop().unwrap());
        }

        assert_eq!(
            parser.next(&bytes),
            Some(Event::Line(Line {
                bytes: b" FGHIJ\r\n",
                announcement: None,
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(
            parser.next(&bytes),
            Some(Event::Message(Message {
                bytes: b"A1 LOGIN {5}\r\nABCDE FGHIJ\r\n",
            }))
        );

        assert_eq!(parser.next(&bytes), None);
    }

    #[test]
    fn change_bytes_unexpectedly() {
        let bytes1 = b"A1 OK";
        let bytes2 = b"A2 OK ...\r\n";
        let bytes3 = b"A3 OK ...\r\n";

        let mut parser = MessageParser::new();

        assert_eq!(None, parser.next(bytes1));

        assert_eq!(
            parser.next(bytes2),
            Some(Event::Line(Line {
                bytes: b"A2 OK ...\r\n",
                announcement: None,
                ending: LineEnding::CrLf,
            }))
        );

        assert_eq!(None, parser.next(bytes1));

        assert_eq!(
            parser.next(bytes3),
            Some(Event::Message(Message {
                bytes: b"A3 OK ...\r\n",
            }))
        );

        assert_eq!(None, parser.next(bytes1));
    }
}
