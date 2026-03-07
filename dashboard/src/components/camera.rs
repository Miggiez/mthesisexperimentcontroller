use crate::types::{Message, SidecarPayload};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use iced::futures::SinkExt;
use iced::futures::channel::mpsc::Sender;
use iced::stream;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as ProcessCommand;
use tokio::sync::mpsc;

pub fn sidecar_worker() -> impl iced::futures::Stream<Item = Message> {
    stream::channel(100, |mut output: Sender<Message>| async move {
        let child_result = ProcessCommand::new("pixi")
            .arg("run")
            .arg("inspect")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn();

        let mut child = match child_result {
            Ok(c) => c,
            Err(e) => {
                let _ = output
                    .send(Message::SidecarMessage(format!(
                        "Process spawn failed: {}",
                        e
                    )))
                    .await;
                return;
            }
        };

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();
        let _ = output.send(Message::ProcessReady(cmd_tx)).await;

        let mut reader = BufReader::new(stdout).lines();
        let mut writer = tokio::io::BufWriter::new(stdin);

        loop {
            tokio::select! {
                Some(cmd) = cmd_rx.recv() => {
                    let _ = writer.write_all(format!("{}\n", cmd).as_bytes()).await;
                    let _ = writer.flush().await;
                }
                Ok(Some(line)) = reader.next_line() => {
                    if let Ok(payload) = serde_json::from_str::<SidecarPayload>(&line) {
                        if payload.event == "frame" {
                            if let Some(b64) = payload.data {
                                if let Ok(bytes) = BASE64.decode(b64) {
                                    let cam_id = payload.camera_id.unwrap_or(0);
                                    let _ = output.send(Message::FrameReceived(cam_id, bytes)).await;
                                }
                            }
                        } else if let Some(msg) = payload.message {
                            let _ = output.send(Message::SidecarMessage(msg)).await;
                        }
                    }
                }
                else => break,
            }
        }
    })
}
