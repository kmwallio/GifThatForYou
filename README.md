# GifThatForYou
GIF Screen Recorder

A GTK4 screen recording utility written in Rust, compatible with Wayland.

## Features

- **Record entire screen** or **select a region** to record
- **Floating recording indicator** – a small window with a **Stop** button appears while recording
- **Keyboard shortcut** – press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd> to stop recording at any time
- Output is saved as a high-quality GIF to `~/Pictures/`

## Requirements

| Tool | Purpose |
|------|---------|
| [`wf-recorder`](https://github.com/ammen99/wf-recorder) | Wayland screen capture (wlr-screencopy protocol) |
| `ffmpeg` | Convert the captured video to GIF |
| GTK 4.6+ | UI toolkit |

Install on Arch Linux:
```bash
sudo pacman -S wf-recorder ffmpeg gtk4
```

Install on Ubuntu / Debian (24.04+):
```bash
sudo apt install wf-recorder ffmpeg libgtk-4-dev
```

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
   Click **Select Region…** to draw a selection rectangle on the screen.
3. A small floating **Recording…** indicator appears with a **■ Stop** button.
4. Click **■ Stop** (or press <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd>) to stop recording.
5. The GIF is saved to `~/Pictures/recording-<timestamp>.gif`.

## Notes

- The region selector works by overlaying a full-screen transparent GTK4 window. Press <kbd>Escape</kbd> to cancel the selection.
- `wf-recorder` uses the `wlr-screencopy` Wayland protocol, so it requires a compatible compositor (Sway, Hyprland, etc.). GNOME and KDE support may vary.
