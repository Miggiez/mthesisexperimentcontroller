mod components;
pub mod types;
use crate::components::save::save_records_with_polars;
use types::*;

use iced::widget::{
    Canvas, Stack, button, column, container, image as iced_image, pick_list, row, text,
};
use iced::{Color, Element, Length, Subscription, Task};

use crate::components::{camera::sidecar_worker, vibration::mqtt_worker};

impl std::fmt::Display for LabelSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LabelSelection::None => write!(f, "None"),
            LabelSelection::Pass => write!(f, "Pass"),
            LabelSelection::Fail => write!(f, "Fail"),
        }
    }
}

impl LabelSelection {
    pub const ALL: [LabelSelection; 3] = [
        LabelSelection::None,
        LabelSelection::Pass,
        LabelSelection::Fail,
    ];
}

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

            label: None,
            selectd_label: LabelSelection::None,

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
                        cam0_bytes: records.cam0_bytes,
                        cam1_bytes: records.cam1_bytes,
                        label: records.label,
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
                    vib_data: records.vib_data,
                    cam1_bytes: records.cam1_bytes,
                    label: records.label,
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
                    vib_data: records.vib_data,
                    cam0_bytes: records.cam0_bytes,
                    label: records.label,
                };
                self.pending_records = self.records.populated_count();
                self.sys_message =
                    format!("Queued Images. Pending items: {}", self.pending_records);
                Task::none()
            }
            Message::QueueLabel => {
                let records = self.records.clone();
                self.records = QueuedRecord {
                    cam1_bytes: records.cam1_bytes,
                    vib_data: records.vib_data,
                    cam0_bytes: records.cam0_bytes,
                    label: self.label,
                };
                self.pending_records = self.records.populated_count();
                self.sys_message = format!("Queued Label. Pending items: {}", self.pending_records);
                Task::none()
            }
            Message::SelectedLabel(selection) => {
                self.selectd_label = selection;
                match selection {
                    LabelSelection::Pass => self.label = Some(1),
                    LabelSelection::Fail => self.label = Some(0),
                    LabelSelection::None => self.label = None,
                }
                Task::none()
            }
            Message::SaveAllToDisk => {
                if self.pending_records < 4 {
                    self.sys_message = "Need all 4 pending records to be saved".to_string();
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
                        self.records.refresh_on_save();
                        self.pending_records = self.records.populated_count();
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
            row![
                cam0_display,
                cam1_display,
                pick_list(
                    &LabelSelection::ALL[..],
                    Some(self.selectd_label),
                    Message::SelectedLabel
                )
            ]
            .spacing(15),
            row![
                text("").width(Length::Fill),
                button("Queue Image Top").on_press(Message::QueueImage1),
                button("Queue Image Bottom").on_press(Message::QueueImage2),
                button("Queue Selected Label").on_press(Message::QueueLabel)
            ]
            .spacing(3)
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
                text(format!("Pending Items: {} / 4", self.pending_records))
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
