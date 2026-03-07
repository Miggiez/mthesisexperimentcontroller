use crate::types::QueuedRecord;

use polars::prelude::*;
use std::fs::OpenOptions;
use tokio::fs::{File as AsyncFile, create_dir_all};
use tokio::io::AsyncWriteExt;

impl QueuedRecord {
    pub fn populated_count(&self) -> usize {
        (self.vib_data.is_some() as usize)
            + (self.cam0_bytes.is_some() as usize)
            + (self.cam1_bytes.is_some() as usize)
            + (self.label.is_some() as usize)
    }
    pub fn refresh_on_save(&self) -> Self {
        Self {
            vib_data: None,
            cam0_bytes: None,
            cam1_bytes: None,
            label: None,
        }
    }
}

pub async fn save_records_with_polars(records: QueuedRecord, id: usize) -> Result<usize, String> {
    create_dir_all("records").await.map_err(|e| e.to_string())?;

    let mut ids = Vec::new();
    let mut vib_paths = Vec::new();
    let mut cam0_paths = Vec::new();
    let mut cam1_paths = Vec::new();
    let mut labels = Vec::new();

    let records_count = 1;

    let current_id = id;

    ids.push(current_id as u32);

    let mut v_path = String::new();
    if let Some(v_data) = records.vib_data {
        let path = format!("records/vib_{}.json", current_id);
        if let Ok(json) = serde_json::to_string_pretty(&v_data) {
            if let Ok(mut f) = AsyncFile::create(&path).await {
                let _ = f.write_all(json.as_bytes()).await;
                v_path = path;
            }
        }
    }
    vib_paths.push(v_path);

    let mut c0_path = String::new();
    if let Some(c0_bytes) = records.cam0_bytes {
        let path = format!("records/cam0_{}.jpg", current_id);
        if let Ok(mut f) = AsyncFile::create(&path).await {
            let _ = f.write_all(&c0_bytes).await;
            c0_path = path;
        }
    }
    cam0_paths.push(c0_path);

    let mut c1_path = String::new();
    if let Some(c1_bytes) = records.cam1_bytes {
        let path = format!("records/cam1_{}.jpg", current_id);
        if let Ok(mut f) = AsyncFile::create(&path).await {
            let _ = f.write_all(&c1_bytes).await;
            c1_path = path;
        }
    }
    cam1_paths.push(c1_path);

    let mut label = 0;
    if let Some(label_val) = records.label {
        label = label_val;
    } else {
        label = 0;
    }

    labels.push(label);

    let result = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        let mut df = df!(
            "record_id" => &ids,
            "vibration_path" => &vib_paths,
            "cam0_path" => &cam0_paths,
            "cam1_path" => &cam1_paths,
            "label" => &labels
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
