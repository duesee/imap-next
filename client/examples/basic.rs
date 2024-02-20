use std::{error::Error, num::NonZeroU32};

use argh::FromArgs;
use client::ClientBuilder;
use imap_types::{
    core::{AString, Vec1},
    fetch::{MessageDataItemName, Part, Section},
    mailbox::Mailbox,
};

/// Basic example.
#[derive(Debug, FromArgs, PartialEq)]
struct Arguments {
    /// host
    #[argh(positional)]
    host: String,

    /// port
    #[argh(positional)]
    port: u16,

    /// username
    #[argh(positional)]
    username: String,

    /// password (prompted when not provided)
    #[argh(option)]
    password: Option<String>,

    /// additional trust anchors
    #[argh(option)]
    additional_trust_anchors: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Arguments = argh::from_env();

    fetch_inbox_top(
        args.host,
        args.port,
        args.username,
        args.password
            .unwrap_or_else(|| rpassword::prompt_password("Password: ").unwrap()),
        args.additional_trust_anchors,
    )
    .await?;

    Ok(())
}

async fn fetch_inbox_top(
    host: String,
    port: u16,
    username: String,
    password: String,
    additional_trust_anchors: Vec<String>,
) -> Result<(), Box<dyn Error>> {
    let (greeting, mut client) = ClientBuilder::new()
        .additional_trust_anchors(&additional_trust_anchors)
        .receive_greeting(&host, port)
        .await?;
    println!("# Greeting\n\n{greeting:#?}\n");

    let data = client.login(username, password).await?;
    println!("# Login\n\n{data:#?}\n");

    let data = client.select(Mailbox::Inbox).await?;
    println!("# Select\n\n{data:#?}\n");

    // Fetch everything ...
    let data = client
        .fetch(
            1,
            vec![
                MessageDataItemName::Body,
                // Variant 1
                MessageDataItemName::BodyExt {
                    section: None,
                    partial: None,
                    peek: false,
                },
                // Variant 2
                MessageDataItemName::BodyExt {
                    section: Some(Section::HeaderFields(
                        // TODO: Why is `Part` needed?
                        Some(Part(Vec1::from(NonZeroU32::new(1).unwrap()))),
                        Vec1::from(AString::try_from("Content-type").unwrap()),
                    )),
                    partial: Some((42, NonZeroU32::new(1337).unwrap())),
                    peek: true,
                },
                MessageDataItemName::BodyStructure,
                MessageDataItemName::Envelope,
                MessageDataItemName::Flags,
                MessageDataItemName::InternalDate,
                MessageDataItemName::Rfc822,
                MessageDataItemName::Rfc822Header,
                MessageDataItemName::Rfc822Size,
                MessageDataItemName::Rfc822Text,
                MessageDataItemName::Uid,
                // TODO: Only when advertised by server
                // MessageDataItemName::Binary {
                //     section: vec![],
                //     partial: None,
                //     peek: false,
                // },
                // MessageDataItemName::BinarySize { section: vec![] },
            ],
            false,
        )
        .await?;
    println!("# Fetch\n\n{data:#?}\n");

    let data = client.logout().await?;
    println!("# Logout\n\n{data:#?}\n");

    Ok(())
}
