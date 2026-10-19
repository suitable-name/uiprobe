//! Where the frames of a recording go: an ffmpeg pipe, a PNG sequence or memory.

use super::{RecorderOptions, SinkKind};
use crate::screenshot::Shot;
use std::{
    fs,
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, PoisonError},
    thread::JoinHandle,
};

/// How much of ffmpeg's stderr an error message carries.
const STDERR_TAIL: usize = 4096;

/// The highest constant rate factor libx264 accepts (8-bit).
const MAX_CRF: u8 = 51;

/// The frame consumer of one recording.
pub enum Sink {
    Ffmpeg(Box<FfmpegSink>),
    Png { dir: PathBuf, next: u64 },
    Memory(Vec<Shot>),
}

impl Sink {
    /// Opens the sink for frames of `size` (physical pixels).
    pub(crate) fn open(options: &RecorderOptions, size: (u32, u32)) -> Result<Self, String> {
        match &options.sink {
            SinkKind::Ffmpeg => Ok(Self::Ffmpeg(Box::new(FfmpegSink::open(options, size)?))),
            SinkKind::PngSequence(dir) => {
                fs::create_dir_all(dir)
                    .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
                Ok(Self::Png {
                    dir: dir.clone(),
                    next: 1,
                })
            }
            SinkKind::Memory => Ok(Self::Memory(Vec::new())),
        }
    }

    /// Takes one frame.
    pub(crate) fn write(&mut self, frame: &Shot) -> Result<(), String> {
        match self {
            Self::Ffmpeg(sink) => sink.write(frame),
            Self::Png { dir, next } => {
                let path = dir.join(format!("frame_{next:06}.png"));
                *next += 1;
                frame.save_png(&path)
            }
            Self::Memory(frames) => {
                frames.push(frame.clone());
                Ok(())
            }
        }
    }

    /// Ends the stream: for ffmpeg waits for the encoder and renames the `.part` file.
    pub(crate) fn close(self) -> Result<(), String> {
        match self {
            Self::Ffmpeg(sink) => sink.close(),
            Self::Png { .. } | Self::Memory(_) => Ok(()),
        }
    }

    /// The frames of a memory sink (empty for the others).
    pub(crate) fn frames(&self) -> &[Shot] {
        match self {
            Self::Memory(frames) => frames,
            Self::Ffmpeg(_) | Self::Png { .. } => &[],
        }
    }
}

/// An ffmpeg child process fed raw RGBA frames on its stdin.
pub struct FfmpegSink {
    child: Option<Child>,
    stdin: Option<BufWriter<ChildStdin>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    part: PathBuf,
    output: PathBuf,
}

/// `<output>.part`: the name the video is written under until it is complete.
pub fn part_path(output: &Path) -> PathBuf {
    let mut name = output.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    output.with_file_name(name)
}

impl FfmpegSink {
    fn open(options: &RecorderOptions, size: (u32, u32)) -> Result<Self, String> {
        let (width, height) = size;
        if width % 2 != 0 || height % 2 != 0 {
            return Err(format!(
                "the window picture is {width} x {height} pixels; H.264 needs even width and \
                 height. Size the window to an even number of physical pixels"
            ));
        }
        if options.crf > MAX_CRF {
            return Err(format!(
                "crf {} is out of range (0 to {MAX_CRF}); lower is better, 18 is visually lossless",
                options.crf
            ));
        }
        if let Some((w, h)) = options.encode_size
            && (w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0)
        {
            return Err(format!(
                "encode_size {w} x {h} must be non-zero and even for H.264"
            ));
        }
        if let Some(parent) = options
            .output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        let part = part_path(&options.output);
        let _ = fs::remove_file(&part);
        let mut command = Command::new(&options.ffmpeg);
        command
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgba"])
            .arg("-s")
            .arg(format!("{width}x{height}"))
            .arg("-r")
            .arg(options.fps.to_string())
            .args(["-i", "-"]);
        if let Some((w, h)) = options.encode_size.filter(|&s| s != size) {
            command
                .arg("-vf")
                .arg(format!("scale={w}:{h}:flags=lanczos"));
        }
        command
            .args(["-c:v", "libx264", "-preset", "slow", "-crf"])
            .arg(options.crf.to_string())
            .args([
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
                "-f",
                "mp4",
            ])
            .arg(&part)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| {
            format!(
                "cannot start ffmpeg ({}): {e}. Install ffmpeg and put it on PATH, or set \
                 UIPROBE_FFMPEG to its full path",
                options.ffmpeg.display()
            )
        })?;
        let stdin = child.stdin.take().ok_or("ffmpeg has no stdin pipe")?;
        let mut pipe = child.stderr.take().ok_or("ffmpeg has no stderr pipe")?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let collected = Arc::clone(&stderr);
        // Drain stderr continuously so a chatty ffmpeg never blocks on a full pipe.
        let reader = std::thread::spawn(move || {
            let mut chunk = [0_u8; 1024];
            while let Ok(read) = pipe.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                let mut buffer = collected.lock().unwrap_or_else(PoisonError::into_inner);
                buffer.extend_from_slice(&chunk[..read]);
                if buffer.len() > STDERR_TAIL {
                    let excess = buffer.len() - STDERR_TAIL;
                    buffer.drain(..excess);
                }
            }
        });
        Ok(Self {
            child: Some(child),
            stdin: Some(BufWriter::with_capacity(1 << 20, stdin)),
            stderr,
            reader: Some(reader),
            part,
            output: options.output.clone(),
        })
    }

    /// Writes one frame's bytes to ffmpeg.
    fn write(&mut self, frame: &Shot) -> Result<(), String> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err("the recording is already closed".to_owned());
        };
        match stdin.write_all(frame.bytes()) {
            Ok(()) => Ok(()),
            Err(e) => Err(self.failure(&format!("cannot send a frame to ffmpeg: {e}"))),
        }
    }

    /// Tears the encoder down after an error and builds the message with ffmpeg's stderr tail.
    fn failure(&mut self, what: &str) -> String {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
        self.join_reader();
        let _ = fs::remove_file(&self.part);
        format!("{what}; ffmpeg said: {}", self.tail())
    }

    fn join_reader(&mut self) {
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }

    /// The last of ffmpeg's stderr, trimmed.
    fn tail(&self) -> String {
        let text = {
            let buffer = self.stderr.lock().unwrap_or_else(PoisonError::into_inner);
            String::from_utf8_lossy(&buffer).trim().to_owned()
        };
        if text.is_empty() {
            "(nothing on stderr)".to_owned()
        } else {
            text
        }
    }

    /// Flushes and closes the pipe, waits for ffmpeg and moves the `.part` file into place.
    fn close(mut self) -> Result<(), String> {
        if let Some(mut stdin) = self.stdin.take()
            && let Err(e) = stdin.flush()
        {
            return Err(self.failure(&format!("cannot flush the last frames to ffmpeg: {e}")));
        }
        // Dropping the writer above closed ffmpeg's stdin: it now finishes the file.
        let Some(mut child) = self.child.take() else {
            return Err("the recording is already closed".to_owned());
        };
        let status = child
            .wait()
            .map_err(|e| format!("cannot wait for ffmpeg: {e}"))?;
        self.join_reader();
        if !status.success() {
            let _ = fs::remove_file(&self.part);
            return Err(format!("ffmpeg failed ({status}): {}", self.tail()));
        }
        fs::rename(&self.part, &self.output).map_err(|e| {
            format!(
                "cannot move {} to {}: {e}",
                self.part.display(),
                self.output.display()
            )
        })
    }
}

impl Drop for FfmpegSink {
    /// A recording that was never closed is abandoned: ffmpeg is killed, the `.part` removed.
    fn drop(&mut self) {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            self.join_reader();
            let _ = fs::remove_file(&self.part);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_part_file_sits_next_to_the_output() {
        assert_eq!(
            part_path(Path::new("videos/a.b.mp4")),
            Path::new("videos/a.b.mp4.part")
        );
    }

    fn options_with(crf: u8) -> RecorderOptions {
        RecorderOptions {
            crf,
            // Never started: the checks come before the spawn, and a missing executable is an
            // error of its own.
            ffmpeg: PathBuf::from("uiprobe-test-no-such-ffmpeg"),
            output: std::env::temp_dir().join("uiprobe-crf-test.mp4"),
            ..RecorderOptions::default()
        }
    }

    #[test]
    fn a_crf_above_51_is_refused_before_ffmpeg_starts() {
        for crf in [52, 100, 255] {
            let error = FfmpegSink::open(&options_with(crf), (64, 64))
                .err()
                .unwrap_or_default();
            assert!(
                error.contains("out of range") && error.contains(&crf.to_string()),
                "crf {crf}: {error}"
            );
        }
    }

    #[test]
    fn the_crf_limits_zero_and_51_are_accepted() {
        // They pass the check and fail later, at the missing executable.
        for crf in [0, 18, 51] {
            let error = FfmpegSink::open(&options_with(crf), (64, 64))
                .err()
                .unwrap_or_default();
            assert!(error.contains("cannot start ffmpeg"), "crf {crf}: {error}");
        }
    }
}
