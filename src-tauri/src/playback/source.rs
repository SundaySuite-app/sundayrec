//! The source: ffmpeg decoding a recording from a position to raw f32 PCM on a pipe (B5a).
//!
//! One process per stream, `-ss` before `-i` (input seeking; ffmpeg decodes from the seek point
//! before and drops what lies ahead of the position, so the first sample is the asked one). No
//! filter, no resampling: the file's own rate, mono kept mono and anything else as stereo
//! (the same layout rule as the export's decode, `editor::pcm::pcm_format`). A keep is one
//! stream; a join opens the next keep's stream while the outgoing one runs into the crossfade.
//!
//! The child is killed when the stream is dropped (after a seek, after a join), so no decoder
//! outlives what it was reading for.

use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Stdio};

use sundayrec_core::playback::{PcmOpener, PcmStream};

/// Opens [`FfmpegStream`]s on one file.
#[derive(Debug, Clone)]
pub struct FfmpegOpener {
    pub ffmpeg: PathBuf,
    pub input: PathBuf,
    pub sample_rate: u32,
    /// 1 or 2.
    pub channels: usize,
}

impl FfmpegOpener {
    /// The ffmpeg arguments for a stream from `source_frame`.
    pub fn args(&self, source_frame: u64) -> Vec<String> {
        let mut a: Vec<String> = vec!["-nostdin".into(), "-hide_banner".into()];
        a.extend(["-loglevel".into(), "error".into()]);
        if source_frame > 0 {
            let secs = source_frame as f64 / f64::from(self.sample_rate.max(1));
            a.extend(["-ss".into(), format!("{secs:.6}")]);
        }
        a.extend([
            "-i".into(),
            self.input.to_string_lossy().into_owned(),
            "-map".into(),
            "0:a:0".into(),
            "-vn".into(),
            "-f".into(),
            "f32le".into(),
            "-c:a".into(),
            "pcm_f32le".into(),
            "-ar".into(),
            self.sample_rate.to_string(),
            "-ac".into(),
            self.channels.clamp(1, 2).to_string(),
            "pipe:1".into(),
        ]);
        a
    }
}

impl PcmOpener for FfmpegOpener {
    type Stream = FfmpegStream;

    fn open(&mut self, source_frame: u64) -> io::Result<FfmpegStream> {
        let mut child = crate::util::hidden_std_command(&self.ffmpeg)
            .args(self.args(source_frame))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("ffmpeg gave no stdout"))?;
        Ok(FfmpegStream {
            child,
            stdout,
            channels: self.channels.clamp(1, 2),
            bytes: Vec::new(),
            have: 0,
        })
    }
}

/// One running decode.
pub struct FfmpegStream {
    child: Child,
    stdout: ChildStdout,
    channels: usize,
    bytes: Vec<u8>,
    /// Bytes in `bytes` not yet handed out (a partial frame carried over).
    have: usize,
}

impl PcmStream for FfmpegStream {
    #[allow(clippy::chunks_exact_to_as_chunks)]
    fn read_planar(&mut self, dst: &mut [Vec<f32>], at: usize, frames: usize) -> io::Result<usize> {
        let frame_bytes = self.channels * 4;
        let want = frames * frame_bytes;
        if self.bytes.len() < want {
            self.bytes.resize(want, 0);
        }
        // At least one whole frame, unless the stream ends.
        while self.have < frame_bytes {
            match self
                .stdout
                .read(&mut self.bytes[self.have..want.max(frame_bytes)])
            {
                Ok(0) => {
                    let status = self.child.wait()?;
                    if !status.success() {
                        return Err(io::Error::other(format!("ffmpeg stopped: {status}")));
                    }
                    return Ok(0);
                }
                Ok(n) => self.have += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let n = (self.have / frame_bytes).min(frames);
        for (i, frame) in self.bytes[..n * frame_bytes]
            .chunks_exact(frame_bytes)
            .enumerate()
        {
            for (c, s) in frame.chunks_exact(4).enumerate() {
                if let Some(d) = dst.get_mut(c) {
                    d[at + i] = f32::from_le_bytes([s[0], s[1], s[2], s[3]]);
                }
            }
        }
        // A mono source read into more channels: copy.
        for c in self.channels..dst.len() {
            let (src, rest) = dst.split_at_mut(c);
            rest[0][at..at + n].copy_from_slice(&src[0][at..at + n]);
        }
        let used = n * frame_bytes;
        self.bytes.copy_within(used..self.have, 0);
        self.have -= used;
        Ok(n)
    }
}

impl Drop for FfmpegStream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opener() -> FfmpegOpener {
        FfmpegOpener {
            ffmpeg: "ffmpeg".into(),
            input: "/x/a.mp3".into(),
            sample_rate: 44_100,
            channels: 2,
        }
    }

    #[test]
    fn the_decode_seeks_on_the_input_and_runs_no_filter() {
        let a = opener().args(44_100 * 3 / 2);
        let s = a.join(" ");
        assert!(s.contains("-ss 1.500000 -i /x/a.mp3"), "{s}");
        assert!(
            s.ends_with("-f f32le -c:a pcm_f32le -ar 44100 -ac 2 pipe:1"),
            "{s}"
        );
        assert!(!s.contains("-af") && !s.contains("-filter"), "{s}");
        assert!(!opener().args(0).contains(&"-ss".to_string()));
    }

    /// Real ffmpeg (the fetched sidecar, passed by path, so a homebrew ffmpeg on PATH is never
    /// used): a stream opened at 1.5 s of a known sine starts with the sample at 1.5 s.
    #[test]
    fn a_stream_opened_at_a_position_starts_there_or_skips() {
        let Some(ffmpeg) = crate::media::ffmpeg::tests::fetched_sidecar("ffmpeg") else {
            eprintln!("SKIP: no runnable ffmpeg sidecar in src-tauri/binaries");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("sine.wav");
        let ok = std::process::Command::new(&ffmpeg)
            .args(["-nostdin", "-v", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=250:sample_rate=48000:duration=3")
            .args(["-c:a", "pcm_f32le"])
            .arg(&wav)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let mut o = FfmpegOpener {
            ffmpeg,
            input: wav,
            sample_rate: 48_000,
            channels: 1,
        };
        let mut s = o.open(72_000).unwrap();
        let mut dst = vec![vec![0.0f32; 4800]; 2];
        let mut got = 0;
        while got < 4800 {
            let n = s.read_planar(&mut dst, got, 4800 - got).unwrap();
            assert!(n > 0);
            got += n;
        }
        // lavfi's sine has amplitude 1/8.
        for (i, v) in dst[0].iter().enumerate() {
            let t = (72_000 + i) as f64 / 48_000.0;
            let want = (2.0 * std::f64::consts::PI * 250.0 * t).sin() / 8.0;
            assert!(
                (f64::from(*v) - want).abs() < 2e-3,
                "frame {i}: {v} vs {want}"
            );
        }
        assert_eq!(dst[0], dst[1], "mono fills both channels");
        // And it ends: 3 s − 1.5 s = 72 000 frames in all.
        let mut rest = 0;
        loop {
            let n = s.read_planar(&mut dst, 0, 4800).unwrap();
            if n == 0 {
                break;
            }
            rest += n;
        }
        assert_eq!(got + rest, 72_000);
    }
}
