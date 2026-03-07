use rumqttc::AsyncClient;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

#[derive(Default, Clone, Debug, Deserialize)]
pub struct InboundPayload {
    pub status_message: String,
}

#[derive(Default, Clone, Debug, Deserialize, Serialize)]
pub struct VibrationPayload {
    pub vibration: Vec<f64>,
}

#[derive(Deserialize)]
pub struct SidecarPayload {
    pub event: String,
    pub camera_id: Option<usize>,
    pub data: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Default)]
pub struct QueuedRecord {
    pub vib_data: Option<VibrationPayload>,
    pub cam0_bytes: Option<Vec<u8>>,
    pub cam1_bytes: Option<Vec<u8>>,
    pub label: Option<i32>,
}

#[derive(Debug, Clone)]
pub enum Message {
    ToggleConnection,
    MqttReady(Option<AsyncClient>),
    Connected,
    Connecting,
    TelemetryReceived(InboundPayload),
    VibrationDataReceived(VibrationPayload),
    PublishFinished(Result<String, String>),

    QueueVibration,
    QueueImage1,
    QueueImage2,
    QueueLabel,
    SelectedLabel(LabelSelection),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSelection {
    None,
    Pass,
    Fail,
}

pub struct UnifiedDashboard {
    // MQTT State
    pub should_connect: bool,
    pub mqtt_client: Option<AsyncClient>,
    pub telemetry: InboundPayload,
    pub vibration_data: VibrationPayload,
    pub mqtt_status_message: String,
    pub can_publish: bool,

    // Sidecar State
    pub camera_command_sender: Option<mpsc::UnboundedSender<String>>,
    pub is_streaming: bool,
    pub latest_frames: [Option<iced::widget::image::Handle>; 2],
    pub previous_frames: [Option<iced::widget::image::Handle>; 2],
    pub raw_image_bytes: [Option<Vec<u8>>; 2],
    pub camera_status_message: String,

    //GUI state
    pub label: Option<i32>,
    pub selectd_label: LabelSelection,

    // Record State
    pub records: QueuedRecord,
    pub pending_records: usize,
    pub total_vib_files: usize,
    pub total_img_files: usize,
    pub saved_records_count: usize,
    pub sys_message: String,
}

pub struct VibrationGraph<'a> {
    pub data: &'a [f64],
}
