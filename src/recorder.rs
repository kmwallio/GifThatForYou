use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

/// A rectangular screen region for recording.
#[derive(Debug, Clone, Copy)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Region {
    /// Format the region as the geometry string expected by `wf-recorder`.
    pub fn as_geometry(&self) -> String {
        format!("{},{} {}x{}", self.x, self.y, self.width, self.height)
    }
}

/// All shared, mutable recording state kept behind a single `Arc<Mutex<…>>`.
struct RecorderState {
    process: Option<Child>,
    raw_file: Option<PathBuf>,
}

/// High-level recorder that manages the `wf-recorder` subprocess.
///
/// The raw video is captured to a temporary MP4 file and then converted to a
/// GIF using `ffmpeg` when recording stops.
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
            })),
        }
    }

    /// Returns `true` if a recording is currently in progress.
    pub fn is_recording(&self) -> bool {
        self.state.lock().unwrap().process.is_some()
    }

    /// Start recording the given `region`, or the entire screen when `None`.
    ///
    /// The video is written to a temporary file; call [`stop`] to finalise the
    /// GIF output.
    ///
    /// Returns `Err` if `wf-recorder` could not be spawned.
    pub fn start(&self, region: Option<Region>) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state.process.is_some() {
            return Err("Already recording".to_string());
        }

        let raw_file = std::env::temp_dir().join("gif-that-for-you-raw.mp4");

        let mut cmd = Command::new("wf-recorder");
        cmd.arg("--file").arg(&raw_file);

        if let Some(r) = region {
            cmd.arg("--geometry").arg(r.as_geometry());
        }

        let child = cmd.spawn().map_err(|e| {
            format!(
                "Failed to start wf-recorder: {e}\n\
                 Please install wf-recorder (https://github.com/ammen99/wf-recorder)."
            )
        })?;

        state.process = Some(child);
        state.raw_file = Some(raw_file);
        Ok(())
    }

    /// Stop the ongoing recording and convert the captured video to a GIF.
    ///
    /// Returns the path of the saved GIF on success.
    pub fn stop(&self) -> Result<PathBuf, String> {
        // Extract the child process and file path while holding the lock, then
        // release it so other code (e.g. `is_recording`) isn't blocked.
        let (child, raw_file) = {
            let mut state = self.state.lock().unwrap();
            let child = state
                .process
                .take()
                .ok_or_else(|| "Not recording".to_string())?;
            let raw_file = state
                .raw_file
                .take()
                .ok_or_else(|| "Recording file path is missing".to_string())?;
            (child, raw_file)
        };

        // Send SIGINT so wf-recorder flushes and closes the file cleanly.
        send_sigint(child.id());

        // Give wf-recorder a moment to finish writing.
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Determine output path: ~/Pictures/recording-<timestamp>.gif
        let gif_file = gif_output_path();

        convert_to_gif(&raw_file, &gif_file)?;

        // Clean up the raw video.
        let _ = std::fs::remove_file(&raw_file);

        Ok(gif_file)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

/// Convert an MP4 file to a GIF using `ffmpeg`.
///
/// Uses a two-pass palette approach for high-quality output.
fn convert_to_gif(input: &PathBuf, output: &PathBuf) -> Result<(), String> {
    let filter = "fps=10,scale=800:-1:flags=lanczos,split[s0][s1];\
                  [s0]palettegen[p];[s1][p]paletteuse";

    let status = Command::new("ffmpeg")
        .args(["-y", "-i"])
        .arg(input)
        .args(["-vf", filter])
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
