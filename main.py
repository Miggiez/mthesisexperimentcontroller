import base64
import json
import subprocess
import sys
import threading
import time

import cv2 as cv

capture_requested = False
start_stream = False
keep_running = True

# NEW: A lock to prevent the two camera threads from scrambling their JSON strings together!
print_lock = threading.Lock()

camera0 = "/dev/video0"
camera1 = "/dev/video2"


def listen_to_rust():
    global capture_requested, start_stream, keep_running

    for line in sys.stdin:
        cmd = line.strip()
        if cmd == "START":
            start_stream = True
        elif cmd == "STOP":
            start_stream = False
        elif cmd == "CAPTURE":
            if start_stream:
                capture_requested = True
        elif cmd == "QUIT":
            keep_running = False
            break

    keep_running = False


threading.Thread(target=listen_to_rust, daemon=True).start()

# Initialize hardware settings for BOTH cameras
subprocess.run(["v4l2-ctl", "-d", camera0, "-c", "auto_exposure=1"])
subprocess.run(["v4l2-ctl", "-d", camera0, "-c", "exposure_time_absolute=5000"])
subprocess.run(["v4l2-ctl", "-d", camera1, "-c", "auto_exposure=1"])
subprocess.run(["v4l2-ctl", "-d", camera1, "-c", "exposure_time_absolute=5000"])


def camera_worker(camera_id, device_path):
    # Distinct pipeline for each camera feed
    gstreamer_pipeline = (
        f"v4l2src device={device_path} ! "
        "image/jpeg, width=640, height=480 ! "
        "jpegdec ! videoconvert ! appsink drop=1 sync=false"
    )

    cap = cv.VideoCapture(gstreamer_pipeline, cv.CAP_GSTREAMER)

    if not cap.isOpened():
        with print_lock:
            print(
                json.dumps(
                    {"event": "error", "message": f"Failed to open {device_path}"}
                ),
                flush=True,
            )
        return
    else:
        with print_lock:
            print(
                json.dumps({"event": "status", "message": f"Camera {camera_id} Ready"}),
                flush=True,
            )

    try:
        while keep_running:
            if cap.isOpened():
                ret, frame = cap.read()

                if not ret:
                    time.sleep(0.1)
                    continue

                if not start_stream:
                    time.sleep(0.05)
                    continue

                encode_param = [int(cv.IMWRITE_JPEG_QUALITY), 50]
                success, buffer = cv.imencode(".jpg", frame, encode_param)

                if success:
                    b64_string = base64.b64encode(buffer.tobytes()).decode("utf-8")

                    with print_lock:
                        print(
                            json.dumps(
                                {
                                    "event": "frame",
                                    "camera_id": camera_id,
                                    "data": b64_string,
                                }
                            ),
                            flush=True,
                        )

                    time.sleep(0.3)
    finally:
        cap.release()


thread0 = threading.Thread(target=camera_worker, args=(0, camera0), daemon=True)
thread1 = threading.Thread(target=camera_worker, args=(1, camera1), daemon=True)

thread0.start()
thread1.start()

# Keep main thread alive
while keep_running:
    time.sleep(1)
