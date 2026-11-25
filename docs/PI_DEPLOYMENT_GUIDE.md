# Raspberry Pi 4B (`aarch64`) + Tri-Display (3.5" & 7") Kiosk Deployment Guide

This guide covers end-to-end provisioning, hardware calibration, release packaging, single/dual display selection, and `systemd` kiosk deployment for **Thiruvalluvar AI** on **Raspberry Pi OS (64-bit / `aarch64`)** driving either a **3.5-inch `480×320` touchscreen LCD**, a **7-inch `800×480` DSI / `1024×600` HDMI touchscreen**, or both attached with USB/I2S microphone + dual amplified stereo speakers.

---

## 1. Architecture & Resource Budgets

| Component | Target Configuration |
| :--- | :--- |
| **OS / Display Stack** | Raspberry Pi OS 64-bit (`aarch64`), DRM/KMS (`vc4-kms-v3d`), minimal Wayland `cage` compositor (no X11 desktop environment, no Chromium) |
| **Audio Ownership** | `GST_AUDIO_SINK=fakesink` isolates WebKitGTK so `kural-audio` (`cpal`/ALSA) holds exclusive hardware access |
| **Models Footprint** | `<= 350 MB` total in `models/` (`embeddings`, `vad`, `stt_en`, `stt_ta`, `tts/en`, `tts/ta`) |
| **Memory Limits** | `systemd` cgroup guardrails `MemoryHigh=700M`, `MemoryMax=850M` |
| **microSD Protection** | `data/thirukkural.db` opened `?mode=ro&immutable=1`; caches (`XDG_CACHE_HOME=/tmp/kural-cache`, `XDG_DATA_HOME=/tmp/kural-xdg-data`) routed to RAM |

---

## 2. One-Command Pi Provisioning (`scripts/setup_pi.sh`)

On your Raspberry Pi 4B inside `thiruvalluvar-ai-rs/`:

```bash
chmod +x scripts/*.sh deploy/*.sh
./scripts/setup_pi.sh
```

What `scripts/setup_pi.sh` performs:
1. Enforces a space-free `CARGO_TARGET_DIR` (defaults to `/tmp/kural-build`) and adds `-C target-cpu=cortex-a72` on `aarch64`.
2. Installs native ARM64 packages via `apt-get`:
   `build-essential`, `cmake`, `pkg-config`, `curl`, `ca-certificates`, `alsa-utils`, `libasound2-dev`, `libwebkit2gtk-4.1-dev`, `libgtk-3-dev`, `espeak-ng`, `cage`, `fonts-noto-core`, `libinput-tools`.
3. Verifies `rustc >= 1.85.0` (installs minimal `rustup` if missing).
4. Runs `scripts/download_fast_models.sh`:
   - Downloads INT8 `model2vec` embeddings, `silero_vad.onnx`, Zipformer EN STT, NeMo CTC INT8 TA STT, Piper INT8 EN TTS, and Piper TA TTS (`<= 350 MB` enforced).
   - Prunes `test_wavs/` and redundant FP32 `.onnx` files when an `.int8.onnx` sibling exists.
   - Verifies or generates `models/SHA256SUMS`.
5. Checks `kural_cli stats` against `data/thirukkural.db` (`kural_vectors: 1330` and `kb_vectors >= 45`), running `index_embeddings` only if missing or `--reindex` is passed.
6. Lists ALSA capture/playback hardware (`arecord -l`, `aplay -l`, `audio_test --list`), optionally updates `[audio].input_device` and `[audio].output_device` in `config.toml`, optionally writes `/etc/asound.conf` (`--write-asound`), and runs `audio_test` to validate speaker + mic audio.
7. Configures `/etc/systemd/logind.conf.d/99-kural-kiosk.conf` (`IdleAction=ignore`, `HandlePowerKey=ignore`), appends `consoleblank=0` to `/boot/firmware/cmdline.txt` (or `/boot/cmdline.txt`), and inspects DRM/KMS + `libinput` touchscreen devices.

### Useful Flags

```bash
# Non-interactive setup with auto-detected non-HDMI USB/I2S ALSA devices & /etc/asound.conf
./scripts/setup_pi.sh --non-interactive --write-asound

# Explicit ALSA device substrings
./scripts/setup_pi.sh --input-device "USB PnP" --output-device "UACDemoV1.0" --skip-apt
```

---

## 3. ALSA & Hardware Verification

If playback or capture fails during setup:

1. Inspect device names reported by `cpal`:
   ```bash
   CARGO_TARGET_DIR=/tmp/kural-build cargo run --release -p kural-audio --features cli --bin audio_test -- --list
   ```
2. Set matching substrings in `config.toml`:
   ```toml
   [audio]
   input_device = "USB"
   output_device = "Headphones"
   ```
3. Run the 3-second loopback check:
   ```bash
   CARGO_TARGET_DIR=/tmp/kural-build cargo run --release -p kural-audio --features cli --bin audio_test -- --config config.toml
   ```

---

## 4. Tri-Display Support (3.5" `480×320` & 7" `800×480` / `1024×600`), KMS & Touch

A single compiled `kural-app --kiosk` binary adapts automatically at runtime to the active panel resolution:
- **3.5-inch panel (`480×320`)**: Switches automatically into the compact single-card layout + 5-slot Pi dock.
- **7-inch panels (`800×480` DSI or `1024×600` HDMI)**: Renders the compact 2-column split + bottom voice dock automatically.
- **Both 3.5-inch and 7-inch displays attached simultaneously**: Control output selection via environment variables in `deploy/launch_kiosk.sh` without modifying any Rust code (`./deploy/launch_kiosk.sh --help` prints all options):
  ```bash
  # Default (single display 3.5" OR 7" attached) — automatic native fullscreen:
  /opt/thiruvalluvar-ai/deploy/launch_kiosk.sh

  # Both attached — target 7" HDMI explicitly:
  KURAL_DISPLAY_OUTPUT=HDMI-A-1 KURAL_DISPLAY_MODE=1024x600 /opt/thiruvalluvar-ai/deploy/launch_kiosk.sh

  # Both attached — target 7" DSI explicitly:
  KURAL_DISPLAY_OUTPUT=DSI-1 KURAL_DISPLAY_MODE=800x480 /opt/thiruvalluvar-ai/deploy/launch_kiosk.sh

  # Both attached — target 3.5" SPI/DRM panel explicitly:
  KURAL_DISPLAY_OUTPUT=SPI-1 KURAL_DISPLAY_MODE=480x320 /opt/thiruvalluvar-ai/deploy/launch_kiosk.sh
  ```
  For permanent `systemd` connector pinning, add `Environment=KURAL_DISPLAY_OUTPUT=HDMI-A-1` (and optional `KURAL_DISPLAY_MODE=1024x600`) under `[Service]` in `/etc/systemd/system/thiruvalluvar-ai.service` (or `sudo systemctl edit thiruvalluvar-ai.service`) and install `wlr-randr`.

Wayland `cage` requires DRM/KMS (`/dev/dri/card*`, enabled by `dtoverlay=vc4-kms-v3d` in `/boot/firmware/config.txt`). Legacy `fbtft` `/dev/fb1` SPI overlays without DRM/KMS cannot start a Wayland compositor.

### Rotating Touch Coordinates (`libinput`)

If touch coordinates do not match the rotated `480×320` landscape screen:

1. Inspect touch device events and names:
   ```bash
   sudo libinput list-devices | grep -E 'Device:|Kernel:|Calibration:'
   ```
2. Create `/etc/udev/rules.d/99-kural-touchscreen.rules` (example: 90° clockwise):
   ```udev
   ENV{ID_INPUT_TOUCHSCREEN}=="1", ENV{LIBINPUT_CALIBRATION_MATRIX}="0 1 0 -1 0 1"
   ```
   Other common 2x3 matrices:
   - 90° CCW: `"0 -1 1 1 0 0"`
   - 180° inverted: `"-1 0 1 0 -1 1"`
3. Reload `udev`:
   ```bash
   sudo udevadm control --reload-rules && sudo udevadm trigger
   ```

### Recommended `/tmp` tmpfs for microSD Longevity

Add to `/etc/fstab` (or verify `mount | grep /tmp` is already `tmpfs`):

```fstab
tmpfs /tmp tmpfs defaults,noatime,nosuid,size=512M 0 0
```

---

## 5. Building & Installing the Self-Contained Kiosk Bundle

Run the release builder on the Pi (or cross-verify locally):

```bash
./scripts/build_pi_release.sh
```

This builds `kural-app`, `audio_test`, `ai_cli`, `kural_cli`, and `index_embeddings` in `--release` mode (`RUSTFLAGS="-C target-cpu=cortex-a72"` on `aarch64`, `CARGO_TARGET_DIR=/tmp/kural-build`) and writes a complete bundle under `dist/thiruvalluvar-ai-pi/`:

- `kural-app`, `audio_test`, `ai_cli`, `kural_cli`, `index_embeddings`
- `config.toml` (`viewport_mode = "kiosk"`)
- `data/thirukkural.db`
- `models/` (if populated locally)
- `deploy/launch_kiosk.sh` & `deploy/thiruvalluvar-ai.service`
- `scripts/download_fast_models.sh` & `scripts/setup_pi.sh`
- `install_to_opt.sh` & `SHA256SUMS`

Install to `/opt/thiruvalluvar-ai` and enable auto-start on boot:

```bash
# Default kiosk user is `pi`; override with KURAL_KIOSK_USER=<user> if your Pi uses another non-root username
KURAL_KIOSK_USER=pi ./dist/thiruvalluvar-ai-pi/install_to_opt.sh
sudo systemctl start thiruvalluvar-ai.service
```

`install_to_opt.sh` stops any active service before overwriting binaries, preserves an existing `/opt/thiruvalluvar-ai/config.toml` (saving the bundle template as `config.toml.default`), sets ownership, rewrites `User=` in `/etc/systemd/system/thiruvalluvar-ai.service`, reloads `systemd`, and enables the unit.

---

## 6. Service Operations & Troubleshooting

- **Check service status and live logs**:
  ```bash
  sudo systemctl status thiruvalluvar-ai.service
  journalctl -u thiruvalluvar-ai.service -f
  ```
- **WebKitGTK white/blank screen under Wayland `cage`**:
  `deploy/launch_kiosk.sh` sets `GDK_BACKEND=wayland`, `WEBKIT_DISABLE_DMABUF_RENDERER=1`, and `WLR_NO_HARDWARE_CURSORS=1`. Verify `ls /dev/dri/card*` exists and `pi` belongs to `video` and `render`.
- **No audio or ALSA `Device or resource busy`**:
  Confirm `GST_AUDIO_SINK=fakesink` is active in `deploy/launch_kiosk.sh` and check `aplay -l` / `arecord -l` against `/opt/thiruvalluvar-ai/config.toml`.
- **Verify offline AI & DB latency directly on Pi**:
  ```bash
  /opt/thiruvalluvar-ai/kural_cli --db /opt/thiruvalluvar-ai/data/thirukkural.db stats
  /opt/thiruvalluvar-ai/ai_cli benchmark --config /opt/thiruvalluvar-ai/config.toml
  ```

