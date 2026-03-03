use core::f32;
use std::time::Duration;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use iced::futures::SinkExt;
use iced::futures::channel::mpsc::Sender;
use iced::widget::canvas::{Frame, Path, Stroke};
use iced::widget::{
    Canvas, Stack, button, canvas, column, container, image as iced_image, row, text,
};
use iced::{Color, Element, Length, Point, Subscription, Task, stream};
use polars::prelude::*;
use rumqttc::{AsyncClient, MqttOptions};
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use tokio::fs::{File as AsyncFile, create_dir_all};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as ProcessCommand;
use tokio::sync::mpsc;

// ---------------------------------------------------------
// DATA STRUCTURES
// ---------------------------------------------------------

#[derive(Default, Clone, Debug, Deserialize)]
struct InboundPayload {
    status_message: String,
}

#[derive(Default, Clone, Debug, Deserialize, Serialize)]
struct VibrationPayload {
    vibration: Vec<f64>,
}

#[derive(Deserialize)]
struct SidecarPayload {
    event: String,
    camera_id: Option<usize>,
    data: Option<String>,
    message: Option<String>,
}

#[derive(Clone, Default)]
struct QueuedRecord {
    vib_data: Option<VibrationPayload>,
    cam0_bytes: Option<Vec<u8>>,
    cam1_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
enum Message {
    ToggleConnection,
    MqttReady(Option<AsyncClient>),
    Connected,
    Connecting,
    TelemetryReceived(InboundPayload),
    VibrationDataReceived(VibrationPayload),
    PublishFinished(Result<String, String>),

    // Updated Queue & Save Messages
    QueueVibration,
    QueueImage1,
    QueueImage2,
    SaveAllToDisk,
    SaveAllFinished(Result<usize, String>),

    StartVib,
    StopVib,
    ProcessReady(mpsc::UnboundedSender<String>),
    FrameReceived(usize, Vec<u8>),
    FrameDecoded(usize, Option<iced::widget::image::Handle>),
    SidecarMessage(String),
    StartCamera,
    StopCamera,
    InitFileCounts((usize, usize, usize)),
}

struct UnifiedDashboard {
    // MQTT State
    should_connect: bool,
    mqtt_client: Option<AsyncClient>,
    telemetry: InboundPayload,
    vibration_data: VibrationPayload,
    mqtt_status_message: String,
    can_publish: bool,

    // Sidecar State
    camera_command_sender: Option<mpsc::UnboundedSender<String>>,
    is_streaming: bool,
    latest_frames: [Option<iced::widget::image::Handle>; 2],
    previous_frames: [Option<iced::widget::image::Handle>; 2],
    raw_image_bytes: [Option<Vec<u8>>; 2],
    camera_status_message: String,

    // Record State
    records: QueuedRecord,
    pending_records: usize,
    total_vib_files: usize,
    total_img_files: usize,
    saved_records_count: usize,
    sys_message: String,
}

// ---------------------------------------------------------
// BACKGROUND WORKERS
// ---------------------------------------------------------

fn mqtt_worker() -> impl iced::futures::Stream<Item = Message> {
    stream::channel(100, |mut output: Sender<Message>| async move {
        let mut mqtt_options = MqttOptions::new("vibration_gui", "127.0.0.1", 1883);
        mqtt_options.set_keep_alive(Duration::from_secs(5));

        let (client, mut eventloop) = AsyncClient::new(mqtt_options, 10);

        if client
            .subscribe("/inspection/#", rumqttc::QoS::AtMostOnce)
            .await
            .is_ok()
        {
            let _ = output.send(Message::MqttReady(Some(client))).await;
        } else {
            let _ = output.send(Message::MqttReady(None)).await;
        }

        loop {
            match eventloop.poll().await {
                Ok(rumqttc::Event::Incoming(rumqttc::Packet::ConnAck(_))) => {
                    let _ = output.send(Message::Connected).await;
                }
                Ok(rumqttc::Event::Incoming(rumqttc::Packet::Publish(publish))) => {
                    if publish.topic == "/inspection/status" {
                        if let Ok(payload) =
                            serde_json::from_slice::<InboundPayload>(&publish.payload)
                        {
                            let _ = output.send(Message::TelemetryReceived(payload)).await;
                        }
                    } else if publish.topic == "/inspection/data/stream" {
                        if let Ok(payload) =
                            serde_json::from_slice::<VibrationPayload>(&publish.payload)
                        {
                            let _ = output.send(Message::VibrationDataReceived(payload)).await;
                        }
                    }
                }
                Err(_) => {
                    let _ = output.send(Message::Connecting).await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                _ => {}
            }
        }
    })
}

fn sidecar_worker() -> impl iced::futures::Stream<Item = Message> {
    stream::channel(100, |mut output: Sender<Message>| async move {
        let child_result = ProcessCommand::new("pixi")
            .arg("run")
            .arg("inspect")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
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

// ---------------------------------------------------------
// CANVAS GRAPH IMPLEMENTATION
// ---------------------------------------------------------
struct VibrationGraph<'a> {
    data: &'a [f64],
}

impl<'a> canvas::Program<Message> for VibrationGraph<'a> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        if self.data.is_empty() {
            return vec![frame.into_geometry()];
        }

        let x_max = (self.data.len().saturating_sub(1)) as f64;
        let y_min = self
            .data
            .iter()
            .copied()
            .fold(f64::INFINITY, |a, b| a.min(b));
        let y_max = self
            .data
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, |a, b| a.max(b));
        let (y_min, y_max) = if (y_max - y_min).abs() < f64::EPSILON {
            (y_min - 1.0, y_max + 1.0)
        } else {
            (y_min, y_max)
        };

        let width_f64 = bounds.width as f64;
        let height_f64 = bounds.height as f64;

        let path = Path::new(|builder| {
            for (i, &y) in self.data.iter().enumerate() {
                let x = i as f64;
                let px_f64 = if x_max > 0.0 {
                    (x / x_max) * width_f64
                } else {
                    0.0
                };
                let py_f64 = height_f64 - ((y - y_min) / (y_max - y_min)) * height_f64;
                let pt = Point {
                    x: px_f64 as f32,
                    y: py_f64 as f32,
                };
                if i == 0 {
                    builder.move_to(pt);
                } else {
                    builder.line_to(pt);
                }
            }
        });

        frame.stroke(
            &path,
            Stroke::default()
                .with_color(Color::from_rgb(0.0, 0.7, 1.0))
                .with_width(2.0),
        );
        vec![frame.into_geometry()]
    }
}

// ---------------------------------------------------------
// Queue Records Impl
// ---------------------------------------------------------

impl QueuedRecord {
    pub fn populated_count(&self) -> usize {
        (self.vib_data.is_some() as usize)
            + (self.cam0_bytes.is_some() as usize)
            + (self.cam1_bytes.is_some() as usize)
    }
}

// ---------------------------------------------------------
// MAIN APPLICATION
// ---------------------------------------------------------

impl UnifiedDashboard {
    fn new() -> Self {
        Self {
            should_connect: false,
            mqtt_client: None,
            telemetry: InboundPayload::default(),
            vibration_data: VibrationPayload::default(),
            mqtt_status_message: "Disconnected".to_string(),
            can_publish: false,

            camera_command_sender: None,
            is_streaming: false,
            latest_frames: [None, None],
            previous_frames: [None, None],
            raw_image_bytes: [None, None],
            camera_status_message: "Starting up...".to_string(),

            records: QueuedRecord::default(),
            pending_records: 0,
            total_vib_files: 0,
            total_img_files: 0,
            saved_records_count: 0,
            sys_message: "".to_string(),
        }
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Connecting => {
                self.mqtt_status_message = String::from("Connecting...");
                self.can_publish = false;
                Task::none()
            }
            Message::Connected => {
                self.mqtt_status_message = String::from("Connected");
                self.can_publish = true;
                Task::none()
            }
            Message::ToggleConnection => {
                self.should_connect = !self.should_connect;
                if !self.should_connect {
                    self.mqtt_client = None;
                    self.mqtt_status_message = String::from("Disconnected");
                } else {
                    self.mqtt_status_message = String::from("Connecting...");
                }
                Task::none()
            }
            Message::MqttReady(client) => {
                self.mqtt_client = client;
                if self.mqtt_client.is_some() {
                    self.mqtt_status_message = String::from("Connected to Robot!");
                }
                Task::none()
            }
            Message::TelemetryReceived(payload) => {
                self.telemetry = payload;
                Task::none()
            }
            Message::VibrationDataReceived(payload) => {
                self.vibration_data = payload;
                Task::none()
            }
            Message::StartVib => {
                self.publish_command("/inspection/cmd".to_string(), "start".to_string())
            }
            Message::StopVib => {
                self.publish_command("/inspection/cmd".to_string(), "stop".to_string())
            }
            Message::PublishFinished(result) => {
                if let Err(e) = result {
                    self.sys_message = format!("MQTT Error: {}", e);
                }
                Task::none()
            }
            Message::QueueVibration => {
                if !self.vibration_data.vibration.is_empty() {
                    let records = self.records.clone();
                    self.records = QueuedRecord {
                        vib_data: Some(self.vibration_data.clone()),
                        ..records
                    };

                    self.pending_records = self.records.populated_count();
                    self.sys_message = format!(
                        "Queued Vibration Data. Pending items: {}",
                        self.pending_records
                    );
                };
                Task::none()
            }
            Message::QueueImage1 => {
                let records = self.records.clone();
                self.records = QueuedRecord {
                    cam0_bytes: self.raw_image_bytes[0].clone(),
                    ..records
                };
                self.pending_records = self.records.populated_count();
                self.sys_message =
                    format!("Queued Images. Pending items: {}", self.pending_records);
                Task::none()
            }

            Message::QueueImage2 => {
                let records = self.records.clone();
                self.records = QueuedRecord {
                    cam1_bytes: self.raw_image_bytes[1].clone(),
                    ..records
                };
                self.pending_records = self.records.populated_count();
                self.sys_message =
                    format!("Queued Images. Pending items: {}", self.pending_records);
                Task::none()
            }
            Message::SaveAllToDisk => {
                if self.pending_records < 3 {
                    self.sys_message = "Need all 3 pending records to be saved".to_string();
                    return Task::none();
                }

                let records_to_save = std::mem::take(&mut self.records);
                let start_id = if self.saved_records_count > 0 {
                    self.saved_records_count - 1
                } else {
                    0
                };

                Task::perform(
                    async move { save_records_with_polars(records_to_save, start_id).await },
                    Message::SaveAllFinished,
                )
            }
            Message::SaveAllFinished(result) => {
                match result {
                    Ok(saved_count) => {
                        self.saved_records_count += saved_count;
                        self.total_img_files += 2;
                        self.total_vib_files += 1;
                        self.sys_message = format!(
                            "Successfully saved {} records to disk and CSV!",
                            saved_count
                        );
                    }
                    Err(e) => self.sys_message = format!("Failed to save records: {}", e),
                }
                Task::none()
            }

            // --- SIDECAR UPDATES ---
            Message::ProcessReady(tx) => {
                self.camera_command_sender = Some(tx);

                Task::perform(
                    async move {
                        let _ = tokio::fs::create_dir_all("records").await;

                        let mut vib = 0;
                        let mut img = 0;
                        if let Ok(mut entries) = tokio::fs::read_dir("records").await {
                            while let Ok(Some(entry)) = entries.next_entry().await {
                                if let Ok(name) = entry.file_name().into_string() {
                                    if name.ends_with(".json") {
                                        vib += 1;
                                    } else if name.ends_with(".jpg") {
                                        img += 1;
                                    }
                                }
                            }
                        }

                        let csv_rows = tokio::task::spawn_blocking(|| {
                            use polars::prelude::*;
                            use std::fs::File;
                            use std::path::Path;

                            let file_path = "records/inventory.csv";

                            // 1. If the file doesn't exist, create it and instantly return 0!
                            if !Path::new(file_path).exists() {
                                let _ = File::create(file_path);
                                return 0;
                            }

                            // 2. If it does exist, read it normally
                            let reader_result = CsvReadOptions::default()
                                .with_has_header(true)
                                .try_into_reader_with_file_path(Some(file_path.into()));

                            if let Ok(reader) = reader_result {
                                if let Ok(df) = reader.finish() {
                                    return df.height();
                                }
                            }

                            0
                        })
                        .await
                        .unwrap_or(0);

                        Ok((csv_rows, vib, img))
                    },
                    |res: Result<(usize, usize, usize), ()>| Message::InitFileCounts(res.unwrap()),
                )
            }
            Message::FrameReceived(cam_id, bytes) => {
                if cam_id < 2 {
                    self.raw_image_bytes[cam_id] = Some(bytes.clone());

                    Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                let img = image::load_from_memory(&bytes).ok()?;
                                let rgba = img.to_rgba8();
                                let (width, height) = rgba.dimensions();

                                Some(iced::widget::image::Handle::from_rgba(
                                    width,
                                    height,
                                    rgba.into_raw(),
                                ))
                            })
                            .await
                            .unwrap_or(None)
                        },
                        move |handle| Message::FrameDecoded(cam_id, handle),
                    )
                } else {
                    Task::none()
                }
            }
            Message::FrameDecoded(cam_id, handle) => {
                if cam_id < 2 {
                    if let Some(val) = handle {
                        self.previous_frames[cam_id] = self.latest_frames[cam_id].take();
                        self.latest_frames[cam_id] = Some(val);
                    }
                }
                Task::none()
            }
            Message::SidecarMessage(msg) => {
                self.camera_status_message = msg.clone();
                self.sys_message = msg;
                Task::none()
            }
            Message::StartCamera => {
                self.is_streaming = true;
                if let Some(tx) = &self.camera_command_sender {
                    let _ = tx.send("START".to_string());
                    self.sys_message = "Start Cameras".to_string();
                }
                Task::none()
            }
            Message::StopCamera => {
                self.is_streaming = false;
                if let Some(tx) = &self.camera_command_sender {
                    let _ = tx.send("STOP".to_string());
                }
                Task::none()
            }
            Message::InitFileCounts((records_count, vib, img)) => {
                self.total_vib_files = vib;
                self.total_img_files = img;
                self.saved_records_count = records_count;
                self.sys_message = format!(
                    "System Ready | Inventory: {} rows | Found {} JSONs and {} JPGs",
                    records_count, vib, img
                );
                Task::none()
            }
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![Subscription::run(sidecar_worker)];

        if self.should_connect {
            subs.push(Subscription::run(mqtt_worker));
        }

        Subscription::batch(subs)
    }

    fn publish_command(&self, topic: String, command: String) -> Task<Message> {
        if let Some(client) = &self.mqtt_client {
            let client = client.clone();
            Task::perform(
                async move {
                    client
                        .publish(
                            topic.clone(),
                            rumqttc::QoS::AtLeastOnce,
                            false,
                            command.clone(),
                        )
                        .await
                        .map(|_| format!("{} sent", command))
                        .map_err(|e| e.to_string())
                },
                Message::PublishFinished,
            )
        } else {
            Task::none()
        }
    }

    fn view(&self) -> Element<'_, Message> {
        let mqtt_color = if self.mqtt_status_message == "Connected" {
            Color::from_rgb(0.0, 1.0, 0.0)
        } else {
            Color::from_rgb(1.0, 0.0, 0.0)
        };
        let connect_btn = button(if self.should_connect {
            "Disconnect MQTT"
        } else {
            "Connect MQTT"
        })
        .on_press(Message::ToggleConnection);

        let mqtt_controls = row![
            button("Arm Solenoid").on_press(Message::StartVib),
            button("Stop").on_press(Message::StopVib),
        ]
        .spacing(10);

        let canvas_widget = Canvas::new(VibrationGraph {
            data: &self.vibration_data.vibration,
        })
        .width(Length::Fill)
        .height(300);

        let left_panel = column![
            text("Robot Telemetry & Vibration").size(24),
            text(&self.mqtt_status_message).color(mqtt_color),
            connect_btn,
            text(format!("Telemetry: {}", self.telemetry.status_message)),
            mqtt_controls,
            container(canvas_widget)
                .width(Length::Fill)
                .height(300)
                .style(|_theme| container::background(Color::from_rgb(0.1, 0.1, 0.12))),
            row![
                text("").width(Length::Fill),
                button("Queue Vibration").on_press(Message::QueueVibration)
            ]
        ]
        .spacing(15)
        .width(Length::FillPortion(1));

        // --- RIGHT COLUMN: DUAL CAMERA SIDECAR ---
        let cam_color = if self.camera_command_sender.is_some() {
            Color::from_rgb(0.0, 1.0, 0.0)
        } else {
            Color::from_rgb(1.0, 0.0, 0.0)
        };
        let cam_controls = row![
            button("Start Stream").on_press(Message::StartCamera),
            button("Stop Stream").on_press(Message::StopCamera),
        ]
        .spacing(10);

        let build_display = |latest: &Option<iced::widget::image::Handle>,
                             prev: &Option<iced::widget::image::Handle>,
                             title: String|
         -> Element<'_, Message> {
            let stack_container: Element<'_, Message> = if latest.is_some() || prev.is_some() {
                let mut layered_stack = Stack::new().width(Length::Fill).height(Length::Fill);

                if let Some(p) = prev {
                    layered_stack = layered_stack.push(
                        iced_image(p.clone())
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .content_fit(iced::ContentFit::Contain),
                    );
                }

                if let Some(c) = latest {
                    layered_stack = layered_stack.push(
                        iced_image(c.clone())
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .content_fit(iced::ContentFit::Contain),
                    );
                }
                layered_stack.into()
            } else {
                container(text("Waiting...").size(14))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .into()
            };

            column![
                text(title).size(16),
                container(stack_container)
                    .width(Length::Fill)
                    .height(250)
                    .style(|_theme| container::background(Color::from_rgb(0.05, 0.05, 0.05)))
            ]
            .spacing(5)
            .width(Length::FillPortion(1))
            .into()
        };

        let cam0_display = build_display(
            &self.latest_frames[0],
            &self.previous_frames[0],
            "Camera 0".to_string(),
        );
        let cam1_display = build_display(
            &self.latest_frames[1],
            &self.previous_frames[1],
            "Camera 1".to_string(),
        );

        let right_panel = column![
            text("Inspection Cameras").size(24),
            text(&self.camera_status_message).color(cam_color),
            cam_controls,
            row![cam0_display, cam1_display].spacing(15),
            row![
                text("").width(Length::Fill),
                button("Queue Image Top").on_press(Message::QueueImage1),
                button("Queue Image Bottom").on_press(Message::QueueImage2)
            ]
        ]
        .spacing(15)
        .width(Length::FillPortion(1));

        // --- MASTER LAYOUT ---
        let master_layout = column![
            text("RPP Master Control Panel").size(32),
            row![left_panel, right_panel]
                .spacing(40)
                .width(Length::Fill),
            row![
                text(format!("Pending Items: {} / 3", self.pending_records))
                    .size(20)
                    .width(Length::Fill),
                button(text("Save All Pending Data").size(20))
                    .on_press(Message::SaveAllToDisk)
                    .padding(15)
            ]
            .align_y(iced::Alignment::Center)
            .width(Length::Fill),
            text(&self.sys_message)
                .size(16)
                .color(Color::from_rgb(0.8, 0.8, 0.2))
        ]
        .spacing(30)
        .padding(40);

        container(master_layout)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

// ---------------------------------------------------------
// POLARS CSV & FILE SAVER WORKER
// ---------------------------------------------------------
async fn save_records_with_polars(records: QueuedRecord, id: usize) -> Result<usize, String> {
    create_dir_all("records").await.map_err(|e| e.to_string())?;

    let mut ids = Vec::new();
    let mut vib_paths = Vec::new();
    let mut cam0_paths = Vec::new();
    let mut cam1_paths = Vec::new();

    let records_count = 1;

    let current_id = id;

    ids.push(current_id as u32);

    let mut v_path = String::new();
    if let Some(v_data) = records.vib_data {
        let path = format!("records/vib_{}.json", current_id);
        if let Ok(json) = serde_json::to_string_pretty(&v_data) {
            if let Ok(mut f) = AsyncFile::create(&path).await {
                let _ = f.write_all(json.as_bytes());
                v_path = path;
            }
        }
    }
    vib_paths.push(v_path);

    let mut c0_path = String::new();
    if let Some(c0_bytes) = records.cam0_bytes {
        let path = format!("records/cam0_{}.jpg", current_id);
        if let Ok(mut f) = AsyncFile::create(&path).await {
            let _ = f.write_all(&c0_bytes);
            c0_path = path;
        }
    }
    cam0_paths.push(c0_path);

    let mut c1_path = String::new();
    if let Some(c1_bytes) = records.cam1_bytes {
        let path = format!("records/cam1_{}.jpg", current_id);
        if let Ok(mut f) = AsyncFile::create(&path).await {
            let _ = f.write_all(&c1_bytes);
            c1_path = path;
        }
    }
    cam1_paths.push(c1_path);

    let result = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let mut df = df!(
            "record_id" => &ids,
            "vibration_path" => &vib_paths,
            "cam0_path" => &cam0_paths,
            "cam1_path" => &cam1_paths
        )
        .map_err(|e| e.to_string())?;

        let mut file = OpenOptions::new()
            .write(true)
            .append(true)
            .create(true)
            .open("records/inventory.csv")
            .map_err(|e| e.to_string())?;

        let has_data = file.metadata().map(|m| m.len() > 0).unwrap_or(false);

        CsvWriter::new(&mut file)
            .include_header(!has_data)
            .finish(&mut df)
            .map_err(|e| e.to_string())?;

        Ok(records_count)
    })
    .await
    .map_err(|e| format!("Thread error: {}", e))??;

    Ok(result)
}

fn main() -> iced::Result {
    iced::application(
        UnifiedDashboard::new,
        UnifiedDashboard::update,
        UnifiedDashboard::view,
    )
    .title("RPP Unified Dashboard")
    .subscription(UnifiedDashboard::subscription)
    .run()
}
