# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] - YYYY-MM-DD

### Added

* Created README, CHANGELOG, badges, rustfmt.toml, ...
* Created project board
* Setup CI: Check, Build, Lint, Audit, Coverage, ...
* Licensed everything as "APACHE OR MIT"
* `imap-flow`
    * Implemented literal handling, handles, events, and examples
    * Implemented AUTHENTICATE and IDLE
    * Implemented a self-test, and tested against a few providers
* `proxy`
    * Implemented argument processing and configuration
    * Smoke tested against a few providers (and a few MUAs)
    * Provided a README
    * Supported capabilities are ...
	* AUTH={PLAIN,LOGIN,XOAUTH2,ScramSha1,ScramSha256}
	* SASL-IR
	* QUOTA*
	* MOVE
	* LITERAL+/LITERAL-
	* UNSELECT
	* ID
	* IDLE
    * Use ALPN==imap
* `imap-tasks` prototype
    * Designed `Task`s trait
    * Implemented `Task` for a few commands
    * Implemented a task scheduler/manager
* `tag-generator`

[Unreleased]: https://github.com/duesee/imap-flow/compare/0a89b5e180ad7dfd3d67d1184370fa1028ea92b4...HEAD
