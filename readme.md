# System Architecture

### The project follows a distributed hardware-in-the-loop (HIL) design to ensure high-performance UI responsiveness while handling heavy computer vision tasks.

- The Controller (Rust + iced-rs): Acts as the central nervous system. It manages the state of the experiment and renders real-time telemetry.
- The Vision Bridge (Python + OpenCV): Handles image processing on the copper-clad boards. This can be integrated via a sidecar process or a library bridge (like PyO3).
- The Sensory Layer (ESP32): An edge device that monitors physical vibrations on the board, publishing data to the MQTT broker.
- Communication (rumqttc): An asynchronous MQTT client that links the Rust frontend to the ESP32 sensor data.

```mermaid
graph TD;
    A[ESP32 + Vibration Sensor] -- MQTT Pub --> B(MQTT Broker)
    B -- MQTT Sub --> C[Rust App / iced-rs]
    D[Copper-Clad Board] -- Camera Feed --> E[Python / OpenCV]
    E -- Analysis Data --> C```

# Prerequisites

To get this environment running, you will need the following dependencies installed:
## Rust Environment
- Rustup: Latest stable toolchain.
- Dependencies: cmake and pkg-config (required for compiling some underlying crates like rumqttc or opencv-rust if used).

## Pixi Environment
- Install Pixi: https://pixi.prefix.dev/latest/
- Add it to your 

## Embedded & IoT
- MQTT Broker: A running instance of Mosquitto (local) or a cloud-based broker like HiveMQ.
- ESP32 Toolchain: Arduino IDE with ESP32 support or ESP-IDF.
- Libraries: An MQTT client library for the ESP32 (like PubSubClient).

## Getting Started
- Clone the repo:
    ```https://github.com/Miggiez/mthesisexperimentcontroller.git```
- ```cd mthesisexperimentcontroller```
- ```pixi install```
- To build opencv from source and get gstreamer: ```pixi run build```
- Start your MQTT Broker:
- Flash the ESP32:
    Navigate to /firmware and upload the vibration sensor code.
- Run the Rust Application:
    ```pixi run gui```
