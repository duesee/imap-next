use std::io::Write;

use anyhow::Result;
use argh::FromArgs;
use tracing_subscriber::EnvFilter;

use self::{query::query, tui::tui};

mod query;
mod tui;

/// IMAP CLI.
#[derive(Debug, FromArgs, PartialEq)]
struct CliArguments {
    #[argh(subcommand)]
    subcommand: SubCommand,
}

#[derive(Debug, FromArgs, PartialEq)]
#[argh(subcommand)]
enum SubCommand {
    Query(Query),
    Tui(Tui),
}

/// query data from an IMAP server
#[derive(Debug, FromArgs, PartialEq)]
#[argh(subcommand, name = "query")]
struct Query {
    /// host
    #[argh(positional)]
    host: String,

    /// port
    #[argh(positional)]
    port: u16,

    /// username (prompted when not provided)
    #[argh(option)]
    username: Option<String>,

    /// username (prompted when not provided)
    #[argh(option)]
    password: Option<String>,
}

/// start an interactive TUI
#[derive(Debug, FromArgs, PartialEq)]
#[argh(subcommand, name = "tui")]
struct Tui {
    /// host
    #[argh(positional)]
    host: String,

    /// port
    #[argh(positional)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .with_file(false)
        .with_line_number(false)
        .without_time()
        .init();

    let args: CliArguments = argh::from_env();

    match args.subcommand {
        SubCommand::Query(args) => {
            let username = match args.username {
                Some(username) => username,
                None => {
                    print!("Username: ");
                    std::io::stdout().flush()?;
                    let mut line = String::new();
                    std::io::stdin().read_line(&mut line)?;
                    line
                }
            };

            let password = match args.password {
                Some(username) => username,
                None => rpassword::prompt_password("Password: ")?,
            };

            // TODO: unwrap
            query(&args.host, args.port, &username, &password)
                .await
                .unwrap();
        }
        SubCommand::Tui(args) => {
            // TODO: unwrap
            tui(&args.host, args.port).await.unwrap();
        }
    }

    Ok(())
}
