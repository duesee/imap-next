use bstr::ByteSlice;
use imap_codec::{
    decode::Decoder, encode::Encoder, AuthenticateDataCodec, CommandCodec, GreetingCodec,
    IdleDoneCodec, ResponseCodec,
};
use imap_types::{
    command::Command,
    response::{CommandContinuationRequest, Data, Greeting, Response, Status},
};

/// Contains all codecs from `imap-codec`.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct Codecs {
    pub greeting_codec: GreetingCodec,
    pub command_codec: CommandCodec,
    pub response_codec: ResponseCodec,
    pub authenticate_data_codec: AuthenticateDataCodec,
    pub idle_done_codec: IdleDoneCodec,
}

impl Codecs {
    pub fn encode_greeting(&self, greeting: &Greeting) -> Vec<u8> {
        self.greeting_codec.encode(greeting).dump()
    }

    pub fn encode_command(&self, command: &Command) -> Vec<u8> {
        self.command_codec.encode(command).dump()
    }

    pub fn encode_response(&self, response: &Response) -> Vec<u8> {
        self.response_codec.encode(response).dump()
    }

    pub fn encode_continuation_request(
        &self,
        continuation_request: &CommandContinuationRequest,
    ) -> Vec<u8> {
        self.response_codec
            .encode(&Response::CommandContinuationRequest(
                continuation_request.clone(),
            ))
            .dump()
    }

    pub fn encode_data(&self, data: &Data) -> Vec<u8> {
        self.response_codec
            .encode(&Response::Data(data.clone()))
            .dump()
    }

    pub fn encode_status(&self, status: &Status) -> Vec<u8> {
        self.response_codec
            .encode(&Response::Status(status.clone()))
            .dump()
    }

    pub fn decode_greeting<'a>(&self, bytes: &'a [u8]) -> Greeting<'a> {
        match self.greeting_codec.decode(bytes) {
            Ok((rem, greeting)) => {
                if !rem.is_empty() {
                    panic!(
                        "Expected single greeting but there are remaining bytes {:?}",
                        rem.as_bstr()
                    )
                }
                greeting
            }
            Err(err) => {
                panic!(
                    "Got error {:?} when parsing greeting from bytes {:?}",
                    err,
                    bytes.as_bstr()
                )
            }
        }
    }

    pub fn decode_command<'a>(&self, bytes: &'a [u8]) -> Command<'a> {
        match self.command_codec.decode(bytes) {
            Ok((rem, command)) => {
                if !rem.is_empty() {
                    panic!(
                        "Expected single command but there are remaining bytes {:?}",
                        rem.as_bstr()
                    )
                }
                command
            }
            Err(err) => {
                panic!(
                    "Got error {:?} when parsing command from bytes {:?}",
                    err,
                    bytes.as_bstr()
                )
            }
        }
    }

    pub fn decode_response<'a>(&self, bytes: &'a [u8]) -> Response<'a> {
        match self.response_codec.decode(bytes) {
            Ok((rem, response)) => {
                if !rem.is_empty() {
                    panic!(
                        "Expected single response but there are remaining bytes {:?}",
                        rem.as_bstr()
                    )
                }
                response
            }
            Err(err) => {
                panic!(
                    "Got error {:?} when parsing response bytes {:?}",
                    err,
                    bytes.as_bstr()
                )
            }
        }
    }

    pub fn decode_continuation_request<'a>(
        &self,
        bytes: &'a [u8],
    ) -> CommandContinuationRequest<'a> {
        let Response::CommandContinuationRequest(expected_data) = self.decode_response(bytes)
        else {
            panic!("Got wrong response type when parsing continuation request from {bytes:?}")
        };
        expected_data
    }

    pub fn decode_data<'a>(&self, bytes: &'a [u8]) -> Data<'a> {
        let Response::Data(expected_data) = self.decode_response(bytes) else {
            panic!("Got wrong response type when parsing data from {bytes:?}")
        };
        expected_data
    }

    pub fn decode_status<'a>(&self, bytes: &'a [u8]) -> Status<'a> {
        let Response::Status(expected_status) = self.decode_response(bytes) else {
            panic!("Got wrong response type when parsing status from {bytes:?}")
        };
        expected_status
    }

    pub fn decode_greeting_normalized<'a>(&self, bytes: &'a [u8]) -> Greeting<'a> {
        let greeting = self.decode_greeting(bytes);
        let normalized_bytes = self.encode_greeting(&greeting);
        assert_eq!(
            normalized_bytes.as_bstr(),
            bytes.as_bstr(),
            "Bytes must contain a normalized greeting"
        );
        greeting
    }

    pub fn decode_command_normalized<'a>(&self, bytes: &'a [u8]) -> Command<'a> {
        let command = self.decode_command(bytes);
        let normalized_bytes = self.encode_command(&command);
        assert_eq!(
            normalized_bytes.as_bstr(),
            bytes.as_bstr(),
            "Bytes must contain a normalized command"
        );
        command
    }

    pub fn decode_response_normalized<'a>(&self, bytes: &'a [u8]) -> Response<'a> {
        let response = self.decode_response(bytes);
        let normalized_bytes = self.encode_response(&response);
        assert_eq!(
            normalized_bytes.as_bstr(),
            bytes.as_bstr(),
            "Bytes must contain a normalized response"
        );
        response
    }

    pub fn decode_continuation_request_normalized<'a>(
        &self,
        bytes: &'a [u8],
    ) -> CommandContinuationRequest<'a> {
        let continuation_request = self.decode_continuation_request(bytes);
        let normalized_bytes = self.encode_continuation_request(&continuation_request);
        assert_eq!(
            normalized_bytes.as_bstr(),
            bytes.as_bstr(),
            "Bytes must contain a normalized continuation request"
        );
        continuation_request
    }

    pub fn decode_data_normalized<'a>(&self, bytes: &'a [u8]) -> Data<'a> {
        let data = self.decode_data(bytes);
        let normalized_bytes = self.encode_data(&data);
        assert_eq!(
            normalized_bytes.as_bstr(),
            bytes.as_bstr(),
            "Bytes must contain a normalized data"
        );
        data
    }

    pub fn decode_status_normalized<'a>(&self, bytes: &'a [u8]) -> Status<'a> {
        let status = self.decode_status(bytes);
        let normalized_bytes = self.encode_status(&status);
        assert_eq!(
            normalized_bytes.as_bstr(),
            bytes.as_bstr(),
            "Bytes must contain a normalized status"
        );
        status
    }
}
