# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased] - YYYY-MM-DD

### Added

* Chore
    * Created README, CHANGELOG, badges, rustfmt.toml, ...
    * Created project board
    * Setup CI: Check, Build, Lint, Audit, Coverage, ...
    * Licensed as "APACHE OR MIT"

* Committed `imap-flow` prototype (WIP)
    * Implemented literal handling, handles, events, and examples
    * Implemented a self-test, and tested against a few providers

* Committed `proxy` prototype (WIP)
    * Implemented argument processing and configuration
    * Smoke tested against a few providers (and a few MUAs)
    * Provided a README

* Committed `imap-tasks` prototype (WIP)
    * Designed `Task`s trait
    * Implemented `Task` for a few commands
    * Implemented a task scheduler/manager
* Committed `tag-generator` prototype (WIP)

[Unreleased]: https://github.com/duesee/imap-flow/compare/0a89b5e180ad7dfd3d67d1184370fa1028ea92b4...HEAD
