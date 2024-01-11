[![Build & Test](https://github.com/duesee/imap-flow/actions/workflows/build_and_test.yml/badge.svg)](https://github.com/duesee/imap-flow/actions/workflows/build_and_test.yml)
[![Audit](https://github.com/duesee/imap-flow/actions/workflows/audit.yml/badge.svg)](https://github.com/duesee/imap-flow/actions/workflows/audit.yml)
[![Coverage](https://coveralls.io/repos/github/duesee/imap-flow/badge.svg?branch=main)](https://coveralls.io/github/duesee/imap-flow?branch=main)
<!--TODO-->
<!--[![Documentation](https://docs.rs/imap-flow/badge.svg)](https://docs.rs/imap-flow)-->

# imap-flow

`imap-flow` is a thin abstraction over IMAP's distinct "protocol flows".
These are literal handling, AUTHENTICATE, and IDLE.

The way these flows were defined in IMAP couples networking, parsing, and business logic.
`imap-flow` untangles these flows, providing a minimal interface allowing sending and receiving coherent messages.
It's a thin layer paving the ground for a correct client or server implementation.

## Playground

This repository also serves as a playground for crates built on `imap-flow`.
These will eventually be moved into their own repositories.

Notably, we have the `proxy`, `tasks`, and `tag-generator` workspace members.

* `proxy` is an already usable (but still not production-ready) IMAP proxy.
  It gracefully forwards unsolicited responses, abstracts away literal processing, and `Debug`-prints messages.
  Proxies are great for challenging the usability of a library, and we use them to validate our design decisions.
  (See the [README](./proxy/README.md).)
* `tasks` is our prototype of a higher-level IMAP library that abstracts away command and response handling into `Task`s.
  This crate will eventually become what a client or server implementor should use to get IMAP right.
  Currently, only the client side is implemented.
* `tag-generator` generates process-wide unique (and unguessable) IMAP tags.
  This crate is here for organizational reasons and may be moved (or inlined) eventually.

# License

This crate is dual-licensed under Apache 2.0 and MIT terms.

# Thanks

Thanks to the [NLnet Foundation](https://nlnet.nl/) for supporting `imap-flow` through their [NGI Assure](https://nlnet.nl/assure/) program!

<div align="right">
    <img alt="NLnet logo" height="100px" src="https://user-images.githubusercontent.com/8997731/215262095-ab12d43a-ca8a-4d44-b79b-7e99ab91ca01.png"/>
    <img alt="Whitespace" height="100px" src="https://user-images.githubusercontent.com/8997731/221422192-60d28ed4-10bb-441e-957d-93af58166707.png"/>
    <img alt="NGI Assure logo" height="100px" src="https://user-images.githubusercontent.com/8997731/215262235-0db02da9-7c6c-498e-a3d2-7ea7901637bf.png"/>
</div>
