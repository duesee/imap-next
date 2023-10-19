use std::{
    collections::HashMap,
    io::stdout,
    sync::{Arc, Mutex},
};

use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use imap_codec::{
    decode::Decoder,
    imap_types::{
        command::Command,
        response::{Data, Greeting, Status},
    },
    CommandCodec,
};
use imap_flow::{
    client::{ClientFlow, ClientFlowCommandHandle, ClientFlowEvent, ClientFlowOptions},
    stream::AnyStream,
};
use ratatui::{prelude::*, widgets::*};
use tokio::{net::TcpStream, select};
use tokio_rustls::{
    rustls::{ClientConfig, OwnedTrustAnchor, RootCertStore, ServerName},
    TlsConnector,
};

struct App {
    client: ClientFlow,
    trace: Vec<Item>,
    scroll: (u16, u16),
    command_line: String,
    commands: HashMap<ClientFlowCommandHandle, Command<'static>>,
}

enum Item {
    Greeting(Greeting<'static>),
    Command(Command<'static>),
    Data(Data<'static>),
    Status(Status<'static>),
}

pub(crate) async fn tui(host: &str, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    let _ = tui_inner(host, port).await;

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

async fn tui_inner(host: &str, port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = { Terminal::new(CrosstermBackend::new(stdout()))? };

    let mut keys = {
        let (sender, receiver) = tokio::sync::mpsc::channel(16);

        // TODO: is this sane?
        tokio::task::spawn(async move {
            loop {
                if let Ok(Ok(event)) = tokio::task::spawn_blocking(|| event::read()).await {
                    let _ = sender.send(event).await;
                }
            }
        });

        receiver
    };

    // TODO: Don't force await on connection to simplify this?
    let app = {
        // TODO: TLS
        let stream = {
            let stream = TcpStream::connect((host, port)).await?;

            let mut root_cert_store = RootCertStore::empty();
            #[allow(deprecated)]
            root_cert_store.add_trust_anchors(webpki_roots::TLS_SERVER_ROOTS.0.iter().map(|ta| {
                OwnedTrustAnchor::from_subject_spki_name_constraints(
                    ta.subject,
                    ta.spki,
                    ta.name_constraints,
                )
            }));
            let config = ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(root_cert_store)
                .with_no_client_auth();
            let connector = TlsConnector::from(Arc::new(config));
            let dnsname = ServerName::try_from(host).unwrap();

            connector.connect(dnsname, stream).await?
        };

        let (client, greeting) =
            ClientFlow::receive_greeting(AnyStream::new(stream), ClientFlowOptions::default())
                .await?;

        Arc::new(Mutex::new(App {
            client,
            trace: vec![Item::Greeting(greeting)],
            scroll: (0, 0),
            command_line: String::default(),
            commands: HashMap::default(),
        }))
    };

    loop {
        terminal.draw({
            let app = app.lock().map_err(|_| "")?;

            move |frame| {
                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(vec![Constraint::Min(0), Constraint::Length(3)])
                    .split(frame.size());

                let block = Block::default().title("Trace").borders(Borders::ALL);
                let lines = app
                    .trace
                    .iter()
                    .map(|item| {
                        let (prefix, color, debug) = match item {
                            Item::Greeting(greeting) => {
                                ("S: ", Color::Blue, format!("{greeting:?}"))
                            }
                            Item::Command(command) => ("C: ", Color::Red, format!("{command:?}")),
                            Item::Data(data) => ("S: ", Color::Blue, format!("{data:?}")),
                            Item::Status(status) => ("S: ", Color::Blue, format!("{status:?}")),
                        };

                        Line {
                            spans: vec![
                                Span::styled(prefix, Style::new().bold()),
                                Span::styled(debug, Style::new().fg(color)),
                            ],
                            alignment: None,
                        }
                    })
                    .collect();
                let msgs = Paragraph::new(Text { lines })
                    .block(block)
                    .scroll(app.scroll);

                let block = Block::default().title("REPL").borders(Borders::ALL);
                let repl = Paragraph::new(format!("> {}", app.command_line)).block(block);

                frame.render_widget(msgs, layout[0]);
                frame.render_widget(repl, layout[1]);
            }
        })?;

        let mut app = app.lock().map_err(|_| "")?;

        select! {
            flow_event = app.client.progress() => {
                match flow_event? {
                    // TODO: Why not the full `Command`?
                    // TODO: Remove `tag`?
                    ClientFlowEvent::CommandSent { handle, .. } => {
                        let cmd = app.commands.remove(&handle).unwrap();
                        app.trace.push(Item::Command(cmd));
                    }
                    ClientFlowEvent::DataReceived { data } => {
                        app.trace.push(Item::Data(data));
                    }
                    ClientFlowEvent::StatusReceived { status } => {
                        app.trace.push(Item::Status(status));
                    }
                }
            }
            key_event = keys.recv() => {
                if let Some(Event::Key(key)) = key_event {
                    if key.kind == event::KeyEventKind::Press {
                        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
                            break;
                        }

                        match key.code {
                            KeyCode::Char(char) => app.command_line.push(char),
                            KeyCode::Backspace => {
                                let _ = app.command_line.pop();
                            }
                            KeyCode::Enter => {
                                let _ = app.command_line.push_str("\r\n");

                                if app.command_line.to_lowercase().as_str() == "done\r\n" {
                                    unimplemented!();
                                }

                                if let Ok((_, cmd)) = CommandCodec::default().decode_static(app.command_line.as_bytes()) {
                                    let handle = app.client.enqueue_command(cmd.clone());
                                    app.commands.insert(handle, cmd);
                                }

                                let _ = app.command_line.clear();
                            }
                            KeyCode::Up => {
                                app.scroll.0 = app.scroll.0.saturating_sub(5);
                            },
                            KeyCode::Down => {
                                app.scroll.0 = app.scroll.0.saturating_add(5);
                            },
                            KeyCode::Left => {
                                app.scroll.1 = app.scroll.1.saturating_sub(10);
                            },
                            KeyCode::Right => {
                                app.scroll.1 = app.scroll.1.saturating_add(10);
                            },
                            _ => {}
                        }
                    }
                }
            }
        };
    }

    Ok(())
}
