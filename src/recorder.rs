use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

use crate::portal::{self, PortalStream};

/// A rectangular screen region used for cropping the recorded video.
#[derive(Debug, Clone, Copy)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Shared, mutable recording state behind `Arc<Mutex<…>>`.
struct RecorderState {
    process: Option<Child>,
    raw_file: Option<PathBuf>,
    crop: Option<Region>,
    /// Keep the PipeWire fd alive for the duration of the recording.
    _pipewire_fd: Option<std::os::fd::OwnedFd>,
}

/// High-level recorder that records the screen via the XDG ScreenCast portal
/// and converts the output to GIF.
///
/// The portal presents a system dialog so the user can choose which screen or
/// window to share.  Recording is done via a GStreamer pipeline that reads
/// from the PipeWire stream.
#[derive(Clone)]
pub struct Recorder {
    state: Arc<Mutex<RecorderState>>,
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RecorderState {
                process: None,
                raw_file: None,
                crop: None,
                _pipewire_fd: None,
            })),
        }
    }

    /// Returns `true` if a recording is currently in progress.
    pub fn is_recording(&self) -> bool {
        self.state.lock().unwrap().process.is_some()
    }

    /// Asynchronously start recording via the XDG ScreenCast portal.
    ///
    /// The portal will present a system dialog for the user to select a screen
    /// or window.  After the user confirms, a GStreamer pipeline is spawned to
    /// record from the resulting PipeWire stream.
    ///
    /// `crop` optionally defines a region to crop during GIF conversion.
    /// `on_result` is called on the main thread when recording starts or fails.
    pub fn start_portal<F>(&self, crop: Option<Region>, on_result: F)
    where
        F: Fn(Result<(), String>) + 'static,
    {
        let state = self.state.clone();
        portal::request_screencast(move |portal_result| {
            match portal_result {
                Ok(stream) => {
                    let result = spawn_gstreamer(&state, stream, crop);
                    on_result(result);
                }
                Err(e) => on_result(Err(e)),
            }
        });
    }

    /// Stop the ongoing recording and convert the captured video to a GIF.
    ///
    /// Returns the path of the saved GIF on success.
    pub fn stop(&self) -> Result<PathBuf, String> {
        let (mut child, raw_file, crop) = {
            let mut state = self.state.lock().unwrap();
            let child = state
                .process
                .take()
                .ok_or_else(|| "Not recording".to_string())?;
            let raw_file = state
                .raw_file
                .take()
                .ok_or_else(|| "Recording file path is missing".to_string())?;
            let crop = state.crop.take();
            // Drop the PipeWire fd to close the portal session.
            state._pipewire_fd = None;
            (child, raw_file, crop)
        };

        // Ask GStreamer to flush and finalize the file via SIGINT, then wait
        // for the process to fully exit.  mp4mux writes the moov atom only on
        // clean shutdown, so we must not touch the file until it exits.
        send_sigint(child.id());
        let _ = child.wait();

        // Determine output path.
        let gif_file = gif_output_path();

        convert_to_gif(&raw_file, &gif_file, crop)?;

        // Clean up the raw video.
        let _ = std::fs::remove_file(&raw_file);

        Ok(gif_file)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Spawn a GStreamer pipeline that records from a PipeWire stream to a file.
fn spawn_gstreamer(
    state: &Arc<Mutex<RecorderState>>,
    stream: PortalStream,
    crop: Option<Region>,
) -> Result<(), String> {
    let mut locked = state.lock().unwrap();
    if locked.process.is_some() {
        return Err("Already recording".to_string());
    }

    // Use MKV (Matroska) instead of MP4: matroskamux writes data
    // sequentially so the file is always valid regardless of how the
    // pipeline is terminated, avoiding the "moov atom not found" error
    // that mp4mux produces when EOS doesn't propagate cleanly.
    let raw_file = std::env::temp_dir().join("gif-that-for-you-raw.mkv");
    let raw_fd = stream.fd.as_raw_fd();

    let mut cmd = Command::new("gst-launch-1.0");
    cmd.args([
        "pipewiresrc",
        &format!("fd={raw_fd}"),
        &format!("path={}", stream.node_id),
        "do-timestamp=true",
        "!",
        "videoconvert",
        "!",
        "x264enc",
        "qp=0",
        "speed-preset=ultrafast",
        "!",
        "matroskamux",
        "!",
        "filesink",
        &format!("location={}", raw_file.display()),
    ]);

    // The PipeWire fd is opened with CLOEXEC.  Clear the flag so the child
    // process inherits it.
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(move || {
            clear_cloexec(raw_fd);
            Ok(())
        });
    }

    let child = cmd.spawn().map_err(|e| {
        format!(
            "Failed to start gst-launch-1.0: {e}\n\
             Please install GStreamer and the PipeWire GStreamer plugin:\n\
             · Arch: pacman -S gstreamer gst-plugins-base gst-plugins-good gst-plugin-pipewire\n\
             · Ubuntu: apt install gstreamer1.0-tools gstreamer1.0-plugins-base \
               gstreamer1.0-plugins-good gstreamer1.0-plugins-ugly gstreamer1.0-pipewire"
        )
    })?;

    locked.process = Some(child);
    locked.raw_file = Some(raw_file);
    locked.crop = crop;
    locked._pipewire_fd = Some(stream.fd);
    Ok(())
}

/// Clear the CLOEXEC flag on a file descriptor so it is inherited by children.
#[cfg(unix)]
unsafe fn clear_cloexec(fd: std::os::fd::RawFd) {
    // F_GETFD = 1, F_SETFD = 2, FD_CLOEXEC = 1
    let flags = libc_fcntl(fd, 1 /* F_GETFD */, 0);
    if flags >= 0 {
        libc_fcntl(fd, 2 /* F_SETFD */, flags & !1);
    }
}

extern "C" {
    /// Thin binding to libc `fcntl(2)`.
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

/// Wrapper for `fcntl(fd, cmd, arg)`.
unsafe fn libc_fcntl(fd: i32, cmd: i32, arg: i32) -> i32 {
    fcntl(fd, cmd, arg)
}

/// Send SIGINT to a process by PID using the system `kill` utility.
fn send_sigint(pid: u32) {
    let _ = Command::new("kill")
        .arg("-SIGINT")
        .arg(pid.to_string())
        .status();
}

/// Build the target GIF path inside `~/Pictures` (falling back to `$HOME`).
fn gif_output_path() -> PathBuf {
    let base = pictures_dir().unwrap_or_else(std::env::temp_dir);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    base.join(format!("recording-{timestamp}.gif"))
}

/// Return the user's `~/Pictures` directory, or `$HOME` if it doesn't exist.
fn pictures_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let pictures = PathBuf::from(&home).join("Pictures");
    if pictures.is_dir() {
        Some(pictures)
    } else {
        Some(PathBuf::from(home))
    }
}

/// Convert a raw video file to GIF using `ffmpeg`.
///
/// When `crop` is provided the video is cropped to the given region before
/// being converted.  Uses a palette-based approach for high-quality output.
fn convert_to_gif(
    input: &PathBuf,
    output: &PathBuf,
    crop: Option<Region>,
) -> Result<(), String> {
    let mut filters = String::new();

    if let Some(c) = crop {
        filters.push_str(&format!(
            "crop={}:{}:{}:{},",
            c.width, c.height, c.x, c.y
        ));
    }

    filters.push_str(
        "fps=10,split[s0][s1];\
         [s0]palettegen[p];[s1][p]paletteuse",
    );

    let status = Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(input)
        .args(["-vf", &filters])
        .arg(output)
        .status()
        .map_err(|e| format!("Failed to run ffmpeg: {e}\nPlease install ffmpeg."))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg exited with status {status} while converting to GIF"
        ))
    }
}
