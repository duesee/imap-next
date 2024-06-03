[![main](https://github.com/duesee/imap-next/actions/workflows/main.yml/badge.svg)](https://github.com/duesee/imap-next/actions/workflows/main.yml)
[![audit](https://github.com/duesee/imap-next/actions/workflows/audit.yml/badge.svg)](https://github.com/duesee/imap-next/actions/workflows/audit.yml)
[![Coverage](https://coveralls.io/repos/github/duesee/imap-next/badge.svg?branch=main)](https://coveralls.io/github/duesee/imap-next?branch=main)

# imap-next

```mermaid
%%{init: {'theme': 'neutral' } }%%
flowchart LR
    imap-types --> imap-codec
    imap-codec --> imap-next
    imap-next -.-> proxy
    imap-next -.-> imap-client
    
    style imap-codec stroke-dasharray: 10 5
    style imap-next stroke-width:4px
    
    click imap-types href "https://github.com/duesee/imap-codec/tree/main/imap-types"
    click imap-codec href "https://github.com/duesee/imap-codec"
    click imap-next href "https://github.com/duesee/imap-next"
    click proxy href "https://github.com/duesee/imap-next/tree/main/proxy"
    click imap-client href "https://github.com/soywod/imap-client"
```

`imap-next` is a thin abstraction over IMAP's distinct "protocol flows".
These are literal handling, AUTHENTICATE, and IDLE.

The way these flows were defined in IMAP couples networking, parsing, and business logic.
`imap-next` untangles these flows, providing a minimal interface allowing sending and receiving coherent messages.
It's a thin layer paving the ground for higher-level client or server implementations.

## Lower-level Libraries

`imap-next` uses [`imap-codec`](https://github.com/duesee/imap-codec) internally for parsing and serialization, and re-exposes [`imap-types`](https://github.com/duesee/imap-codec/tree/main/imap-types).

## Higher-level Libraries

* [`proxy`](https://github.com/duesee/imap-next/tree/main/proxy) is an IMAP proxy that gracefully forwards unsolicited responses, abstracts over literals, and `Debug`-prints messages.
* [`imap-client`](https://github.com/soywod/imap-client) is a methods-based client library with a `client.capability()`, `client.login()`, ... interface.

## Usage

```rust,no_run
use std::error::Error;
use imap_next::{
    client::{ClientFlow, ClientFlowEvent, ClientFlowOptions},
    stream::Stream,
};
use imap_types::{
    command::{Command, CommandBody},
    core::Tag,
};
use tokio::net::TcpStream;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut stream = Stream::insecure(TcpStream::connect("127.0.0.1:1143").await?);
    let mut client = ClientFlow::new(ClientFlowOptions::default());

    loop {
        match stream.progress(&mut client).await? {
            event => {
                println!("{event:?}");

                if matches!(event, ClientFlowEvent::GreetingReceived { .. }) {
                    break;
                }
            }
        }
    }

    let handle = client.enqueue_command(Command::new("A1", CommandBody::login("Al¹cE", "pa²²w0rd")?)?);

    loop {
        match stream.progress(&mut client).await? {
            event => println!("{event:?}"),
        }
    }
}
```

## Playground

This repository also serves as a playground for crates built on `imap-next`.
These will eventually be moved into their own repositories.

* `tag-generator` generates process-wide unique (and unguessable) IMAP tags.
  This crate is here for organizational reasons and may be moved (or inlined) eventually.

# License

This crate is dual-licensed under Apache 2.0 and MIT terms.

# Thanks

Thanks to the [NLnet Foundation](https://nlnet.nl/) for supporting `imap-next` through their [NGI Assure](https://nlnet.nl/assure/) program!

<div align="right">
    <img alt="NLnet logo" height="100px" src="https://user-images.githubusercontent.com/8997731/215262095-ab12d43a-ca8a-4d44-b79b-7e99ab91ca01.png"/>
    <img alt="Whitespace" height="100px" src="https://user-images.githubusercontent.com/8997731/221422192-60d28ed4-10bb-441e-957d-93af58166707.png"/>
    <img alt="NGI Assure logo" height="100px" src="https://user-images.githubusercontent.com/8997731/215262235-0db02da9-7c6c-498e-a3d2-7ea7901637bf.png"/>
</div>
