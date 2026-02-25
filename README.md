# GifThatForYou
GIF Screen Recorder

A GTK4 screen recording utility written in Rust, compatible with Wayland.

## Features

- **Record entire screen** or **select a region** to record
- **Works on all Wayland compositors** – uses the XDG Desktop Portal ScreenCast API (GNOME, KDE, Sway, Hyprland, etc.)
- **Floating recording indicator** – a small window with a **Stop** button appears while recording
- **Keyboard shortcut** – press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd> to stop recording at any time
- Output is saved as a high-quality GIF to `~/Pictures/`

## Requirements

| Component | Purpose |
|-----------|---------|
| GTK 4.6+ | UI toolkit |
| `xdg-desktop-portal` | Standard Wayland screen sharing API |
| GStreamer + PipeWire plugin | Records from the portal's PipeWire stream |
| `ffmpeg` | Converts the captured video to GIF |

Install on Arch Linux:
```bash
sudo pacman -S gtk4 xdg-desktop-portal gstreamer gst-plugins-base gst-plugins-good gst-plugins-ugly gst-plugin-pipewire ffmpeg
```

Install on Ubuntu / Debian (24.04+):
```bash
sudo apt install libgtk-4-dev xdg-desktop-portal gstreamer1.0-tools \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-ugly gstreamer1.0-pipewire ffmpeg
```

**Portal backends** — you also need a portal backend for your compositor:
- GNOME: `xdg-desktop-portal-gnome` (usually pre-installed)
- KDE Plasma: `xdg-desktop-portal-kde` (usually pre-installed)
- Sway / wlroots: `xdg-desktop-portal-wlr`
- Hyprland: `xdg-desktop-portal-hyprland`

## Building

```bash
cargo build --release
```

The binary is placed at `target/release/gif-that-for-you`.

## Usage

1. Launch the application:
   ```bash
   ./target/release/gif-that-for-you
   ```
2. Click **Record Entire Screen** to start recording the full display.  
   *or*  
   Click **Select Region…** to draw a crop rectangle, then record.
3. A system dialog appears asking you to select which screen or window to share.
4. After confirming, a small floating **Recording…** indicator appears with a **■ Stop** button.
5. Click **■ Stop** (or press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd>) to stop recording.
6. The GIF is saved to `~/Pictures/recording-<timestamp>.gif`.

## How it works

1. The app uses the **XDG Desktop Portal** `org.freedesktop.portal.ScreenCast` D-Bus API to negotiate screen access.  This works across all major Wayland compositors.
2. The portal returns a PipeWire stream; a **GStreamer** pipeline (`pipewiresrc → x264enc → mp4mux`) records it to a temporary file.
3. When you stop recording, **ffmpeg** converts the MP4 to a high-quality palette-based GIF.
4. If you selected a crop region, the crop is applied during the GIF conversion step.

## Notes

- The region selector works by overlaying a full-screen transparent GTK4 window.  Press <kbd>Escape</kbd> to cancel the selection.
- No extra Rust crate dependencies are needed beyond `gtk4` and `glib` — the portal interaction is implemented directly using GIO's D-Bus bindings.
