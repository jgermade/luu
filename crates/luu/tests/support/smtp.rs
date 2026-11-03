//! A mail server for tests: speaks just enough SMTP for `lettre` over plain
//! TCP on loopback, and keeps every message it is given. What luu's codes
//! by email are checked against without a network.

use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// One message: who it went to, and its text with headers.
#[derive(Debug, Clone)]
pub struct Sent {
    pub to: Vec<String>,
    pub data: String,
}

impl Sent {
    /// The six digits the subject carries.
    pub fn code(&self) -> Option<String> {
        let at = self.data.find("Your luu code: ")? + "Your luu code: ".len();
        let code: String = self.data[at..].chars().take(6).collect();
        (code.len() == 6 && code.chars().all(|c| c.is_ascii_digit())).then_some(code)
    }
}

pub struct Inbox {
    pub url: String,
    sent: Arc<Mutex<Vec<Sent>>>,
}

impl Inbox {
    pub fn sent(&self) -> Vec<Sent> {
        self.sent.lock().unwrap().clone()
    }

    /// The newest message to `to`, waiting up to ten seconds for it.
    pub async fn wait_for(&self, to: &str) -> Sent {
        for _ in 0..200 {
            if let Some(found) = self
                .sent()
                .into_iter()
                .rev()
                .find(|sent| sent.to.iter().any(|rcpt| rcpt.contains(to)))
            {
                return found;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("no mail to {to}: {:?}", self.sent());
    }
}

/// Starts it on a free port: `smtp://127.0.0.1:<port>`.
pub async fn start() -> Inbox {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let kept = sent.clone();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let kept = kept.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let _ = write.write_all(b"220 luu-test ESMTP\r\n").await;
                let mut to = Vec::new();
                while let Ok(Some(line)) = lines.next_line().await {
                    let upper = line.to_ascii_uppercase();
                    let reply: &[u8] = if upper.starts_with("EHLO") {
                        b"250-luu-test\r\n250 AUTH PLAIN LOGIN\r\n"
                    } else if upper.starts_with("AUTH") {
                        b"235 2.7.0 Authentication successful\r\n"
                    } else if upper.starts_with("RCPT TO:") {
                        to.push(line[8..].trim().to_string());
                        b"250 OK\r\n"
                    } else if upper.starts_with("DATA") {
                        let _ = write.write_all(b"354 go on\r\n").await;
                        let mut data = String::new();
                        while let Ok(Some(line)) = lines.next_line().await {
                            if line == "." {
                                break;
                            }
                            data.push_str(&line);
                            data.push('\n');
                        }
                        kept.lock().unwrap().push(Sent {
                            to: std::mem::take(&mut to),
                            data,
                        });
                        b"250 OK queued\r\n"
                    } else if upper.starts_with("QUIT") {
                        let _ = write.write_all(b"221 bye\r\n").await;
                        break;
                    } else {
                        b"250 OK\r\n"
                    };
                    if write.write_all(reply).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    Inbox {
        url: format!("smtp://127.0.0.1:{port}"),
        sent,
    }
}
