use std::{
    io::{Read, Write},
    net::TcpStream,
};

use imap_flow::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
    FlowInterrupt, FlowIo,
};
use imap_types::{
    command::{Command, CommandBody},
    core::Tag,
};

fn main() {
    let mut stream = TcpStream::connect("127.0.0.1:12345").unwrap();
    let mut read_buffer = [0; 128];
    let mut client = ClientFlow::new(ClientFlowOptions::default());

    let greeting = loop {
        match client.progress() {
            Err(interrupt) => match interrupt {
                FlowInterrupt::Io(FlowIo::NeedMoreInput) => {
                    let count = stream.read(&mut read_buffer).unwrap();
                    client.enqueue_input(&read_buffer[0..count]);
                }
                interrupt => panic!("unexpected interrupt: {interrupt:?}"),
            },
            Ok(event) => match event {
                ClientFlowEvent::GreetingReceived { greeting } => break greeting,
                event => println!("unexpected event: {event:?}"),
            },
        }
    };

    println!("received greeting: {greeting:?}");

    let handle = client.enqueue_command(Command {
        tag: Tag::try_from("A1").unwrap(),
        body: CommandBody::login("Al¹cE", "pa²²w0rd").unwrap(),
    });

    loop {
        match client.progress() {
            Err(interrupt) => match interrupt {
                FlowInterrupt::Io(FlowIo::NeedMoreInput) => {
                    let count = stream.read(&mut read_buffer).unwrap();
                    client.enqueue_input(&read_buffer[0..count]);
                }
                FlowInterrupt::Io(FlowIo::Output(bytes)) => {
                    stream.write_all(&bytes).unwrap();
                }
                FlowInterrupt::Error(error) => {
                    panic!("unexpected error: {error:?}");
                }
            },
            Ok(event) => match event {
                ClientFlowEvent::CommandSent {
                    handle: got_handle,
                    command,
                } => {
                    println!("command sent: {got_handle:?}, {command:?}");
                    assert_eq!(handle, got_handle);
                }
                ClientFlowEvent::CommandRejected {
                    handle: got_handle,
                    command,
                    status,
                } => {
                    println!("command rejected: {got_handle:?}, {command:?}, {status:?}");
                    assert_eq!(handle, got_handle);
                }
                ClientFlowEvent::DataReceived { data } => {
                    println!("data received: {data:?}");
                }
                ClientFlowEvent::StatusReceived { status } => {
                    println!("status received: {status:?}");
                }
                ClientFlowEvent::ContinuationRequestReceived {
                    continuation_request,
                } => {
                    println!("unexpected continuation request received: {continuation_request:?}");
                }
                event => {
                    println!("{event:?}");
                }
            },
        }
    }
}
