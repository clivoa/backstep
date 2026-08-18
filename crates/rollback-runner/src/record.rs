//! Recording a session to video, by piping presented frames into ffmpeg.
//!
//! # Why the frames come from the simulation and not from a screen grab
//!
//! A screen recorder captures what the compositor happened to show, at whatever
//! rate it happened to composite. That is useless for documenting rollback,
//! where the interesting claim is about *which* frames reached the player.
//!
//! This records exactly the frames the simulation produced in
//! [`OutputMode::Present`](rollback_core::OutputMode) -- one per advanced frame,
//! none of the re-simulated ones. So the video is, frame for frame, what the
//! player saw, and a 60 Hz recording of a session that held 60 Hz is proof it
//! held it.
//!
//! # Why ffmpeg rather than an encoder crate
//!
//! Encoding H.264 from Rust means either a large C dependency or a pure-Rust
//! encoder whose output nothing plays. ffmpeg is already how the frames become
//! something watchable, and piping raw RGB into it costs nothing: at 320x224 a
//! frame is 215 KB, and the pipe is the only copy.
//!
//! Recording is opt-in. A session without `--record` never spawns anything.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error(
        "could not start ffmpeg to record '{path}': {source}.\n\
         Recording needs ffmpeg on PATH. Install it, or drop --record."
    )]
    Spawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("ffmpeg stopped accepting frames while recording '{path}': {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("ffmpeg exited with {status} while recording '{path}'")]
    Failed { path: PathBuf, status: String },
}

/// An ffmpeg child process being fed raw RGB frames.
pub struct VideoRecorder {
    child: Child,
    path: PathBuf,
    width: u32,
    height: u32,
    /// Reused between frames so a 60 Hz recording does not allocate 60 times a
    /// second.
    scratch: Vec<u8>,
    pub frames_written: u64,
    /// Frames the simulation reported at a size other than the one ffmpeg was
    /// told to expect. Dropped rather than written, because a short frame would
    /// desynchronise every frame after it.
    pub frames_skipped: u64,
}

impl VideoRecorder {
    /// Start ffmpeg, writing `path`.
    ///
    /// `fps` is the *nominal* rate the video is tagged with. It is the session's
    /// tick rate, not the emulated machine's native rate: the session advances
    /// one frame per tick, so that is what the recording contains.
    pub fn start(path: &Path, width: u32, height: u32, fps: u32) -> Result<Self, RecordError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        let child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                // Input: raw frames on stdin, in the geometry the core reports.
                "-f",
                "rawvideo",
                "-pixel_format",
                "rgb24",
                "-video_size",
                &format!("{width}x{height}"),
                "-framerate",
                &fps.to_string(),
                "-i",
                "-",
                // Output: H.264 that any browser and any player will open.
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                // Visually lossless at this resolution, and small.
                "-crf",
                "18",
                // yuv420p because some players refuse anything else.
                "-pix_fmt",
                "yuv420p",
                // Arcade output is not square-pixel; leave the geometry alone
                // and let the documentation say so rather than guessing an
                // aspect correction the core did not ask for.
                "-vf",
                "scale=iw*3:ih*3:flags=neighbor",
                &path.to_string_lossy(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|source| RecordError::Spawn {
                path: path.to_path_buf(),
                source,
            })?;

        Ok(VideoRecorder {
            child,
            path: path.to_path_buf(),
            width,
            height,
            scratch: Vec::with_capacity((width * height * 3) as usize),
            frames_written: 0,
            frames_skipped: 0,
        })
    }

    /// Write one presented frame.
    ///
    /// `pixels` is XRGB8888, the format the libretro host normalises everything
    /// to. Frames whose geometry does not match what ffmpeg was told are
    /// counted and dropped: writing a short frame would shift every byte after
    /// it and turn the rest of the video into noise.
    pub fn push(&mut self, width: u32, height: u32, pixels: &[u32]) -> Result<(), RecordError> {
        if width != self.width || height != self.height || pixels.len() != (width * height) as usize
        {
            self.frames_skipped += 1;
            return Ok(());
        }

        self.scratch.clear();
        for px in pixels {
            self.scratch.push((px >> 16) as u8);
            self.scratch.push((px >> 8) as u8);
            self.scratch.push(*px as u8);
        }

        let stdin = self.child.stdin.as_mut().expect("stdin was piped");
        stdin
            .write_all(&self.scratch)
            .map_err(|source| RecordError::Write {
                path: self.path.clone(),
                source,
            })?;
        self.frames_written += 1;
        Ok(())
    }

    /// Close the pipe and wait for ffmpeg to finish writing the file.
    pub fn finish(mut self) -> Result<PathBuf, RecordError> {
        drop(self.child.stdin.take());
        let status = self.child.wait().map_err(|source| RecordError::Write {
            path: self.path.clone(),
            source,
        })?;
        if !status.success() {
            return Err(RecordError::Failed {
                path: self.path.clone(),
                status: status.to_string(),
            });
        }
        Ok(self.path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    #[test]
    fn a_recording_produces_a_playable_file() {
        if !have_ffmpeg() {
            eprintln!("ffmpeg not installed; skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("rollback-rec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.mp4");

        let mut rec = VideoRecorder::start(&path, 16, 16, 60).unwrap();
        for f in 0..30u32 {
            let pixels: Vec<u32> = (0..16 * 16).map(|i| (f * 8) << 16 | i as u32).collect();
            rec.push(16, 16, &pixels).unwrap();
        }
        // A frame of the wrong size must be dropped, not written: a short write
        // would corrupt everything after it.
        rec.push(8, 8, &vec![0u32; 64]).unwrap();
        assert_eq!(rec.frames_skipped, 1);
        assert_eq!(rec.frames_written, 30);

        let written = rec.finish().unwrap();
        let size = std::fs::metadata(&written).unwrap().len();
        assert!(size > 0, "ffmpeg produced an empty file");

        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_missing_ffmpeg_is_a_clear_error_not_a_panic() {
        // Not a behaviour test of ffmpeg -- a test that the failure names the
        // dependency, because "No such file or directory (os error 2)" on its
        // own has sent people looking at the wrong thing.
        let err = VideoRecorder::start(Path::new("/dev/null/nope.mp4"), 1, 1, 60);
        if let Err(e) = err {
            let text = e.to_string();
            assert!(text.contains("ffmpeg"), "unhelpful error: {text}");
        }
    }
}
