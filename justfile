export RUSTFLAGS := "-D warnings"
export RUSTDOCFLAGS := "-D warnings"

[private]
default:
    just -l --unsorted

###########
### RUN ###
###########

# Run (local) CI
ci: (ci_impl ""           ""               ) \
    (ci_impl ""           " --all-features") \
    (ci_impl " --release" ""               ) \
    (ci_impl " --release" " --all-features")

[private]
ci_impl mode features: (check_impl mode features) (test_impl mode features)

# Check syntax, formatting, clippy, deny, semver, ...
check: (check_impl ""           ""               ) \
       (check_impl ""           " --all-features") \
       (check_impl " --release" ""               ) \
       (check_impl " --release" " --all-features")

[private]
check_impl mode features: (cargo_check mode features) \
                          (cargo_hack mode) \
                          cargo_fmt \
                          (cargo_clippy mode features) \
                          cargo_deny \
                          cargo_semver

[private]
cargo_check mode features:
    cargo check --workspace --all-targets{{ mode }}{{ features }}
    cargo doc --no-deps --document-private-items --keep-going{{ mode }}{{ features }}

[private]
cargo_hack mode: install_cargo_hack
    cargo hack check --workspace --all-targets{{ mode }}

[private]
cargo_fmt: install_rust_nightly install_rust_nightly_fmt
    cargo +nightly fmt --check

[private]
cargo_clippy features mode: install_cargo_clippy
    cargo clippy --workspace --all-targets{{ features }}{{ mode }}

[private]
cargo_deny: install_cargo_deny
    cargo deny check

[private]
cargo_semver: install_cargo_semver_checks
    # TODO(#8)
    # cargo semver-checks check-release --only-explicit-features -p imap-next

# Test multiple configurations
test: (test_impl ""           ""               ) \
      (test_impl ""           " --all-features") \
      (test_impl " --release" ""               ) \
      (test_impl " --release" " --all-features")

[private]
test_impl mode features: (cargo_test mode features)

[private]
cargo_test features mode:
    cargo test --workspace --all-targets{{ features }}{{ mode }}

# Audit advisories, bans, licenses, and sources
audit: cargo_deny

# Measure test coverage
coverage: install_rust_llvm_tools_preview install_cargo_grcov
    mkdir -p target/coverage
    RUSTFLAGS="-Cinstrument-coverage" LLVM_PROFILE_FILE="coverage-%m-%p.profraw" cargo test -p imap-next -p integration-test --all-features
    grcov . \
        --source-dir . \
        --binary-path target/debug \
        --branch \
        --keep-only 'src/**' \
        --output-types "lcov" \
        --llvm > target/coverage/coverage.lcov
    # TODO: Create files in `target/coverage` only.
    rm *.profraw
    rm integration-test/*.profraw

# Check minimal dependency versions and MSRV
minimal_versions: install_rust_1_70 install_rust_nightly
    cargo +nightly update -Z minimal-versions
    cargo +1.70 check --workspace --all-targets --all-features 
    cargo +1.70 test --workspace --all-targets --all-features
    cargo update

###############
### INSTALL ###
###############

# Install required tooling (ahead of time)
install: install_rust_1_70 \
         install_rust_nightly \
         install_rust_nightly_fmt \
	 install_rust_llvm_tools_preview \
         install_cargo_clippy \
         install_cargo_deny \
         install_cargo_fuzz \
	 install_cargo_grcov \
         install_cargo_hack \
         install_cargo_semver_checks

[private]
install_rust_1_70:
    # Fix issue
    rustup update --no-self-update 1.70
    rustup set profile minimal
    # rustup toolchain install 1.70 --profile minimal

[private]
install_rust_nightly:
    # Fix issue
    rustup update --no-self-update nightly
    rustup set profile minimal
    # rustup toolchain install nightly --profile minimal

[private]
install_rust_nightly_fmt:
    rustup component add --toolchain nightly rustfmt

[private]
install_rust_llvm_tools_preview:
    rustup component add llvm-tools-preview

[private]
install_cargo_clippy:
    rustup component add clippy

[private]
install_cargo_deny:
    cargo install --locked cargo-deny
 
[private]
install_cargo_fuzz: install_rust_nightly
    cargo install cargo-fuzz

[private]
install_cargo_grcov:
    cargo install grcov

[private]
install_cargo_hack:
    cargo install --locked cargo-hack

[private]
install_cargo_semver_checks:
    cargo install --locked cargo-semver-checks

