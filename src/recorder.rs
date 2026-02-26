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
    /// 1 = MONITOR, 2 = WINDOW, 3 = BOTH — stored so stop() knows whether to
    /// auto-crop black borders after a window capture.
    source_types: u32,
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
                source_types: 3,
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
    /// `source_types`: 1=MONITOR, 2=WINDOW, 3=BOTH.
    /// `crop` optionally defines a region to crop during GIF conversion.
    /// `on_result` is called on the main thread when recording starts or fails.
    pub fn start_portal<F>(&self, source_types: u32, crop: Option<Region>, on_result: F)
    where
        F: Fn(Result<(), String>) + 'static,
    {
        let state = self.state.clone();
        portal::request_screencast(source_types, move |portal_result| {
            match portal_result {
                Ok(stream) => {
                    let result = spawn_gstreamer(&state, stream, source_types, crop);
                    on_result(result);
                }
                Err(e) => on_result(Err(e)),
            }
        });
    }

    /// Stop the ongoing recording and convert the captured video to a GIF.
    ///
    /// Returns the path of the saved GIF on success.
    pub fn stop(&self, fps: u32) -> Result<PathBuf, String> {
        let (mut child, raw_file, crop, source_types, pipewire_fd) = {
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
            let source_types = state.source_types;
            // Take the fd out but do NOT drop it yet — GStreamer must finish
            // reading from PipeWire before the fd is closed, otherwise the
            // source errors out mid-stream and the file is left incomplete.
            let pipewire_fd = state._pipewire_fd.take();
            (child, raw_file, crop, source_types, pipewire_fd)
        };

        // Signal gst-launch to send EOS and wait for a clean exit.  The
        // PipeWire fd must still be open at this point so the source can
        // drain cleanly.
        send_sigint(child.id());
        let _ = child.wait();

        // Now it is safe to close the PipeWire fd and end the portal session.
        drop(pipewire_fd);

        // Determine output path.
        let gif_file = gif_output_path();

        // When recording a single window the portal often delivers a
        // monitor-sized stream with the window content centred and the
        // surrounding area filled with black.  Detect and remove those borders
        // automatically before converting to GIF.
        let auto_crop_black = source_types == 2;

        convert_to_gif(&raw_file, &gif_file, crop, fps, auto_crop_black)?;

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
    source_types: u32,
    crop: Option<Region>,
) -> Result<(), String> {
    let mut locked = state.lock().unwrap();
    if locked.process.is_some() {
        return Err("Already recording".to_string());
    }

    // FFV1 is a truly lossless codec with no chroma subsampling and no DCT
    // block artefacts.  x264enc at any quality setting uses 4:2:0 chroma
    // subsampling which destroys colour detail before ffmpeg even sees the
    // file, and the DCT transform introduces block artefacts that pollute the
    // GIF palette.  FFV1 gives ffmpeg pixel-perfect screen content to work
    // from, which dramatically improves GIF quality.
    //
    // avenc_ffv1 comes from the gst-libav package:
    //   · Arch:   pacman -S gst-libav
    //   · Ubuntu: apt install gstreamer1.0-libav
    //
    // MKV is FFV1's natural container and writes data sequentially so the
    // file is readable even if gst-launch exits before writing the final
    // index — especially important for our SIGINT-based stop flow.
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
        "avenc_ffv1",
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
             Please install GStreamer with the PipeWire and libav plugins:\n\
             · Arch:   pacman -S gstreamer gst-plugins-base gst-plugins-good \
               gst-plugin-pipewire gst-libav\n\
             · Ubuntu: apt install gstreamer1.0-tools gstreamer1.0-plugins-base \
               gstreamer1.0-plugins-good gstreamer1.0-pipewire gstreamer1.0-libav"
        )
    })?;

    locked.process = Some(child);
    locked.raw_file = Some(raw_file);
    locked.crop = crop;
    locked.source_types = source_types;
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

/// Run ffmpeg's `cropdetect` filter on `input` and return the detected crop
/// string (e.g. `"crop=1280:800:320:140"`) ready for use in a `-vf` chain.
///
/// The same 0.3 s trim that the GIF conversion applies is used here so that
/// cropdetect sees exactly the frames that will appear in the output.  Using
/// skip=10 on the raw file instead caused the detector to see startup frames
/// that land outside the trimmed range, producing wrong crop coordinates.
///
/// Returns `None` if cropdetect produces no output or the video has no black
/// borders.
fn detect_crop_black(input: &PathBuf) -> Option<String> {
    let output = Command::new("ffmpeg")
        .args(["-i"])
        .arg(input)
        .args([
            "-vf",
            // Trim the same startup frames the GIF conversion will skip, then
            // run cropdetect on the stable content.
            // limit=10: only treat near-zero pixels as black.  limit=24 is too
            //   aggressive and clips window shadows / dark-themed app borders.
            //   The portal's solid black surround is 0,0,0 so limit=10 is
            //   still more than sufficient to detect it.
            // round=2:  keep even dimensions (safe for all codecs).
            // reset=0:  accumulate the widest crop over all analysed frames.
            "trim=start=0.3,setpts=PTS-STARTPTS,cropdetect=limit=10:round=2:reset=0",
            "-frames:v",
            "240",
            "-f",
            "null",
            "-",
        ])
        .output()
        .ok()?;

    // cropdetect writes "crop=W:H:X:Y" to stderr; take the last line (most
    // stable after accumulation).
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr.lines().rev().find_map(|line| {
        let pos = line.find("crop=")?;
        let rest = &line[pos + 5..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit() && c != ':')
            .unwrap_or(rest.len());
        let crop = &rest[..end];
        (crop.split(':').count() == 4).then(|| format!("crop={crop}"))
    })
}

/// Convert a raw video file to GIF using `ffmpeg`.
///
/// When `crop` is provided the video is cropped to the given region before
/// being converted.  When `auto_crop_black` is true, `cropdetect` is run
/// first to strip black borders that portals add around window captures.
/// Uses a palette-based approach for high-quality output.
fn convert_to_gif(
    input: &PathBuf,
    output: &PathBuf,
    crop: Option<Region>,
    fps: u32,
    auto_crop_black: bool,
) -> Result<(), String> {
    let mut filters = String::new();

    // Trim the first 0.3 s of the raw video.  The GStreamer/PipeWire pipeline
    // produces partially-initialised frames during stream negotiation that
    // appear pixelated in the output GIF.  setpts resets timestamps after the
    // trim so the GIF starts cleanly at t=0.
    filters.push_str("trim=start=0.3,setpts=PTS-STARTPTS,");

    // Auto-crop black borders produced by window-capture portal streams.
    if auto_crop_black {
        if let Some(crop_filter) = detect_crop_black(input) {
            filters.push_str(&crop_filter);
            filters.push(',');
        }
    }

    // Manual crop region (from the region selector).
    if let Some(c) = crop {
        filters.push_str(&format!(
            "crop={}:{}:{}:{},",
            c.width, c.height, c.x, c.y
        ));
    }

    filters.push_str(&format!(
        // palettegen stats_mode=diff: build the palette from inter-frame
        // pixel differences rather than all pixels, which gives much better
        // colour representation for animated content.
        //
        // paletteuse floyd_steinberg + diff_mode=rectangle: error-diffusion
        // dithering (smooth gradients, no ordered-dither grid pattern) and
        // only re-encode the rectangular region that changed each frame.
        //
        // No yadif — PipeWire screen captures are always progressive so
        // deinterlacing is unnecessary and can misinterpret FFV1 frame flags.
        "fps={fps},split[s0][s1];\
         [s0]palettegen=stats_mode=diff[p];\
         [s1][p]paletteuse=dither=floyd_steinberg:diff_mode=rectangle"
    ));

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
