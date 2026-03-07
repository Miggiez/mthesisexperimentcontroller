use std::time::Duration;

use crate::types::{InboundPayload, Message, VibrationGraph, VibrationPayload};

use iced::futures::channel::mpsc::Sender;
use iced::widget::canvas::{Frame, Path, Stroke};
use iced::{Color, Point, stream};
use iced::{futures::SinkExt, widget::canvas};
use rumqttc::{AsyncClient, MqttOptions};

pub fn mqtt_worker() -> impl iced::futures::Stream<Item = Message> {
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
