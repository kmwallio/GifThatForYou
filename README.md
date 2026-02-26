# GifThatForYou
GIF Screen Recorder

A GTK4 screen recording utility written in Rust, compatible with Wayland.

## Features

- **Record entire screen**, **record a specific window**, or **select a region** to record
- **Works on all Wayland compositors** — uses the XDG Desktop Portal ScreenCast API (GNOME, KDE, Sway, Hyprland, etc.)
- **FPS selector** — choose 5, 10, 15, 20, 24, or 30 fps (default 15)
- **Floating recording indicator** — a small window with a **Stop** button appears while recording
- **Keyboard shortcut** — press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd> to stop recording at any time
- **Animated GIF spinner** — a processing window shows while ffmpeg converts the recording
- **Auto-crop for window recordings** — black borders added by the portal are automatically removed
- Output is saved as a high-quality GIF to `~/Pictures/`
- **MCP server** — a `gif-that-for-you-mcp` binary exposes recording as a Model Context Protocol tool

## Dependencies

### Runtime dependencies

| Component | Purpose |
|-----------|---------|
| GTK 4.6+ | UI toolkit |
| `xdg-desktop-portal` | Standard Wayland screen sharing API |
| GStreamer + PipeWire plugin | Records from the portal's PipeWire stream |
| GStreamer libav plugin (`gst-libav`) | Provides the FFV1 lossless video codec |
| `ffmpeg` | Converts the captured video to an animated GIF |

Install on **Arch Linux**:
```bash
sudo pacman -S gtk4 xdg-desktop-portal \
  gstreamer gst-plugins-base gst-plugins-good gst-libav \
  gst-plugin-pipewire ffmpeg
```

Install on **Ubuntu / Debian** (24.04+):
```bash
sudo apt install libgtk-4-dev xdg-desktop-portal \
  gstreamer1.0-tools gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good gstreamer1.0-libav \
  gstreamer1.0-pipewire ffmpeg
```

**Portal backends** — you also need a portal backend matching your compositor:

| Compositor | Package |
|------------|---------|
| GNOME | `xdg-desktop-portal-gnome` (usually pre-installed) |
| KDE Plasma | `xdg-desktop-portal-kde` (usually pre-installed) |
| Sway / wlroots | `xdg-desktop-portal-wlr` |
| Hyprland | `xdg-desktop-portal-hyprland` |

### Build dependencies

| Tool | Version | Notes |
|------|---------|-------|
| Rust + Cargo | 1.70+ | Install via [rustup](https://rustup.rs) |
| GLib / GTK4 development headers | 4.6+ | Included in `libgtk-4-dev` / `gtk4` packages above |
| pkg-config | any | Used by Cargo build scripts to locate system libraries |

On Arch: `sudo pacman -S base-devel`
On Ubuntu/Debian: `sudo apt install build-essential pkg-config`

## Building

### From source (native)

```bash
cargo build --release
```

This produces two binaries under `target/release/`:

| Binary | Description |
|--------|-------------|
| `gif-that-for-you` | Main GTK4 application |
| `gif-that-for-you-mcp` | MCP server for AI agent integration |

### Flatpak

The manifest `io.github.kmwallio.GifThatForYou.yml` targets the **GNOME 49 SDK**.

#### Prerequisites

Install `flatpak-builder` and the required runtimes:

```bash
# Install flatpak-builder (Arch)
sudo pacman -S flatpak-builder

# Install flatpak-builder (Ubuntu/Debian)
sudo apt install flatpak-builder

# Add Flathub and install the GNOME 49 SDK
flatpak remote-add --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install flathub org.gnome.Platform//49 org.gnome.Sdk//49
flatpak install flathub org.freedesktop.Sdk.Extension.rust-stable//24.08
```

#### 1. Fill in checksums

Download the source archives and compute their `sha256` values, then replace the `FILL_IN` placeholders in the manifest:

```bash
wget https://ffmpeg.org/releases/ffmpeg-7.1.tar.xz
sha256sum ffmpeg-7.1.tar.xz

wget https://gstreamer.freedesktop.org/src/gst-libav/gst-libav-1.26.0.tar.xz
sha256sum gst-libav-1.26.0.tar.xz
```

> **Note:** If the GNOME 49 SDK already bundles `gst-libav`, you can remove that entire module block from the manifest. Check with:
> ```bash
> flatpak info --show-extensions org.gnome.Sdk//49 | grep -i libav
> ```

#### 2. Generate offline Cargo sources

Flatpak builds run without internet access, so all crate dependencies must be pre-fetched:

```bash
pip install aiohttp toml
curl -LO https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
python3 flatpak-cargo-generator.py Cargo.lock -o cargo-sources.json
```

Commit `cargo-sources.json` alongside the manifest. It only needs to be regenerated when `Cargo.lock` changes.

#### 3. Build and install

```bash
flatpak-builder --user --install --force-clean build-dir \
    io.github.kmwallio.GifThatForYou.yml
```

Run the installed Flatpak:

```bash
flatpak run io.github.kmwallio.GifThatForYou
```

#### Build once without installing (for testing)

```bash
flatpak-builder --force-clean build-dir io.github.kmwallio.GifThatForYou.yml
flatpak-builder --run build-dir io.github.kmwallio.GifThatForYou.yml gif-that-for-you
```

## Usage

### GUI

1. Launch the application:
   ```bash
   ./target/release/gif-that-for-you
   ```
2. Select a frame rate from the dropdown (default: 15 fps).
3. Choose a recording mode:
   - **Record Entire Screen** — records the full display.
   - **Record Window…** — lets you pick a specific window; black borders are cropped automatically.
   - **Select Region…** — draw a crop rectangle first, then record.
4. A system dialog appears asking you to select which screen or window to share.
5. A small floating **Recording…** indicator appears with a **■ Stop** button.
6. Click **■ Stop** (or press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd>) to stop.
7. An "Animating GIF…" window appears while ffmpeg processes the recording.
8. The GIF is saved to `~/Pictures/recording-<timestamp>.gif`.

### MCP server

The `gif-that-for-you-mcp` binary implements the [Model Context Protocol](https://modelcontextprotocol.io) over stdin/stdout (newline-delimited JSON-RPC). It exposes two tools:

- **`start_recording`** — opens the XDG portal picker and starts capturing.
- **`stop_recording`** — stops the capture and returns the path to the saved GIF.
  - Optional parameter: `fps` (integer, 1–30, default 15)

## How it works

1. The app uses the **XDG Desktop Portal** `org.freedesktop.portal.ScreenCast` D-Bus API to negotiate screen access. This works across all major Wayland compositors.
2. The portal returns a PipeWire stream; a **GStreamer** pipeline (`pipewiresrc → videoconvert → avenc_ffv1 → matroskamux`) records it losslessly to a temporary MKV file.
3. When you stop recording, **ffmpeg** converts the MKV to a high-quality palette-based GIF using `floyd_steinberg` dithering and per-frame palette optimization.
4. For **window recordings**, a second ffmpeg pass runs `cropdetect` on the trimmed video to locate and strip the black border the portal places around the window.
5. If you selected a crop region, the crop is applied during the GIF conversion step.

## Notes

- The region selector works by overlaying a full-screen transparent GTK4 window. Press <kbd>Escape</kbd> to cancel the selection.
- Portal interaction is implemented directly using GIO's D-Bus bindings — no extra Rust crates beyond `gtk4`, `glib`, and `serde_json` are required.
- The PipeWire file descriptor is kept open until GStreamer finishes writing, then closed cleanly to avoid MKV/EBML corruption.
