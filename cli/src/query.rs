use std::{error::Error, io::Write, num::NonZeroU32};

use client::Client;
use imap_codec::imap_types::{
    core::NonEmptyVec,
    envelope::Envelope,
    fetch::{MessageDataItem, MessageDataItemName, Part, Section},
    mailbox::Mailbox,
};
use tracing::{info, warn};

/// Query data from an IMAP server.
pub(crate) async fn query(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
) -> Result<(), Box<dyn Error>> {
    let (_greeting, mut client) = Client::receive_greeting(host, port).await?;

    // Real-world:
    // * Handle `_greeting`
    //   * Handle Ok, PreAuth, Bye
    //   * Handle Code::Capability

    // TODO
    // if client.has_pre_auth_capabilities() {}

    // Real-world:
    // * Obtain capabilities
    //   * Ask for pre_auth_capabilities if not already present
    //   * Use allowed authentication mechanisms only
    //     * Don't use LOGIN when LOGINDISABLED found
    //     * Check for AUTH=

    let _ = client.login(username, password).await?;

    // Real-world:
    // * Ask for post_auth_capabilities if not already present
    // * Make use of features such as `LITERAL+`, `SASL-IR`, etc.

    let mailboxes = client.list("", "*").await?;

    loop {
        for (index, mailbox) in mailboxes.iter().enumerate() {
            let mailbox = match mailbox.2 {
                Mailbox::Inbox => "INBOX",
                Mailbox::Other(ref other) => {
                    std::str::from_utf8(other.as_ref()).unwrap_or("<invalid UTF-8>")
                }
            };

            // Real-world:
            // * Encoding
            println!("{index}: {}", mailbox);
        }

        println!();

        let to_be_selected_mailbox = {
            let index: usize = {
                print!("Query data from mailbox (index): ");
                std::io::stdout().flush()?;
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;

                match line.trim().parse() {
                    Ok(index) => index,
                    Err(_) => break,
                }
            };

            match mailboxes.get(index) {
                Some(mailbox) => mailbox,
                None => break,
            }
        };

        // We use EXAMINE instead of SELECT to select a mailbox read-only
        let mut session = client.examine(to_be_selected_mailbox.2.clone()).await?;
        println!("{:#?}\n", session.session_data);

        if session.session_data.exists == Some(0) {
            // Don't try to fetch emails from an empty mailbox
            info!("Skipping fetch from empty mailbox");
            continue;
        }

        let res = session
            .fetch(
                ..,
                vec![
                    MessageDataItemName::Body,
                    MessageDataItemName::BodyExt {
                        section: None,
                        partial: None,
                        peek: true,
                    },
                    // Toy around :-)
                    MessageDataItemName::BodyExt {
                        section: Some(Section::Header(Some(Part(NonEmptyVec::from(
                            NonZeroU32::try_from(1).unwrap(),
                        ))))),
                        partial: None,
                        peek: true,
                    },
                    // Toy around :-)
                    MessageDataItemName::BodyExt {
                        section: Some(Section::Header(Some(Part(NonEmptyVec::from(
                            NonZeroU32::try_from(1).unwrap(),
                        ))))),
                        partial: Some((0, NonZeroU32::try_from(1).unwrap())),
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
                ],
                false,
            )
            .await;

        match res {
            Ok(fetches) => {
                for (seq, items) in fetches {
                    let mut subject = None;

                    for item in items {
                        if let MessageDataItem::Envelope(Envelope {
                            subject: nstring, ..
                        }) = item
                        {
                            subject = Some(match nstring.0 {
                                Some(istring) => std::str::from_utf8(istring.as_ref())
                                    .map(ToOwned::to_owned)
                                    .unwrap_or_else(|_| String::from("<subject is not UTF-8>")),
                                None => String::from("<subject was nil>"),
                            })
                        }
                    }

                    let subject = match subject {
                        Some(subject) => subject,
                        None => String::from("<server didn't return an envelope>"),
                    };

                    println!("# {seq}: {subject}");
                }
            }
            Err(error) => {
                warn!(
                    ?error,
                    "fetch errored. This could mean the mailbox is empty"
                );
            }
        }
    }

    Ok(())
}
