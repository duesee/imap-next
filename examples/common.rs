use std::io::BufRead;

use tokio::sync::mpsc::Receiver;

pub struct Lines {
    receiver: Receiver<String>,
}

impl Lines {
    pub fn new() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        tokio::task::spawn_blocking(move || loop {
            for line in std::io::stdin().lock().lines() {
                sender.blocking_send(line.unwrap()).unwrap();
            }
        });

        Self { receiver }
    }

    pub async fn progress(&mut self) -> String {
        self.receiver.recv().await.unwrap()
    }
}
