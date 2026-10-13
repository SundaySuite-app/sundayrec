//! `camera_probe` — lists the cameras and records from them to a raw file, with
//! statistics (C1c, docs/GJENNOMFORING.md §C1c).
//!
//! It exercises exactly the API the recorder will use (`sundayrec_lib::camera`):
//! `list()`, `formats(id)`, `start(id, format, queue)`. Nothing here touches the
//! recorder.
//!
//! ```text
//! cargo run --example camera_probe                    # list; record 10 s from every camera
//! cargo run --example camera_probe -- --list          # list cameras and their formats only
//! cargo run --example camera_probe -- --camera iphone --seconds 5
//! cargo run --example camera_probe -- --camera facetime --format 1920x1080@30 --keep
//! ```
//!
//! Options:
//!
//! - `--list`            list cameras and formats, record nothing
//! - `--camera <text>`   only cameras whose id or name contains `text` (case-insensitive)
//! - `--format WxH@FPS`  the capture format (default: NV12 nearest 1280x720 at 30 fps)
//! - `--seconds <n>`     recording length (default 10)
//! - `--queue <n>`       the bounded queue, in frames (default 8)
//! - `--out-dir <dir>`   where the raw file goes (default: the system temp dir)
//! - `--keep`            keep the raw file (default: delete it after the statistics)
//!
//! macOS asks for camera access the first time a camera is opened; a terminal
//! without it gets no frames and the probe says so. Listing and formats need no
//! access.
//!
//! The raw file holds each frame's planes back to back, rows as the camera
//! delivered them (padding included) — playable with
//! `ffmpeg -f rawvideo -pix_fmt nv12 -s WxH -i file.raw` when the rows carry no
//! padding. Delete-after is the default because 10 s of 1080p NV12 is ~1 GB.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use sundayrec_lib::camera::stats::interval_stats;
use sundayrec_lib::camera::{self, CameraInfo, PixelFormat, VideoFormat};

struct Args {
    list_only: bool,
    camera: Option<String>,
    format: Option<(u32, u32, f64)>,
    seconds: u64,
    queue: usize,
    out_dir: PathBuf,
    keep: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        list_only: false,
        camera: None,
        format: None,
        seconds: 10,
        queue: 8,
        out_dir: std::env::temp_dir(),
        keep: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--list" => a.list_only = true,
            "--keep" => a.keep = true,
            "--camera" => a.camera = Some(value("--camera")?),
            "--seconds" => {
                a.seconds = value("--seconds")?.parse().map_err(|_| "bad --seconds")?;
            }
            "--queue" => a.queue = value("--queue")?.parse().map_err(|_| "bad --queue")?,
            "--out-dir" => a.out_dir = PathBuf::from(value("--out-dir")?),
            "--format" => {
                let v = value("--format")?;
                let bad = || format!("--format wants WxH@FPS, got {v:?}");
                let (size, fps) = v.split_once('@').ok_or_else(bad)?;
                let (w, h) = size.split_once('x').ok_or_else(bad)?;
                a.format = Some((
                    w.parse().map_err(|_| bad())?,
                    h.parse().map_err(|_| bad())?,
                    fps.parse().map_err(|_| bad())?,
                ));
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(a)
}

/// The format to record when the user named none: 30 fps (or the nearest at or
/// below), NV12 over the packed formats over MJPEG, nearest 1280x720.
fn default_format(formats: &[VideoFormat]) -> Option<&VideoFormat> {
    let rank = |p: PixelFormat| match p {
        PixelFormat::Nv12 => 0,
        PixelFormat::Uyvy => 1,
        PixelFormat::Yuy2 => 2,
        PixelFormat::Mjpeg => 3,
        PixelFormat::Other => 9,
    };
    formats
        .iter()
        .filter(|f| f.pixel_format != PixelFormat::Other)
        .min_by(|a, b| {
            let key = |f: &VideoFormat| {
                let fps_gap = if f.fps <= 30.5 {
                    30.0 - f.fps
                } else {
                    1000.0 + f.fps
                };
                let px_gap =
                    ((f.width as i64 * f.height as i64) - 1280 * 720).unsigned_abs() as f64;
                (
                    rank(f.pixel_format),
                    (fps_gap * 100.0) as i64,
                    px_gap as i64,
                )
            };
            key(a).cmp(&key(b))
        })
}

fn print_formats(formats: &[VideoFormat]) {
    for f in formats {
        println!("      {f}");
    }
}

fn record(cam: &CameraInfo, format: &VideoFormat, a: &Args) {
    println!(
        "\n  recording {} s from {} as {format}",
        a.seconds, cam.name
    );
    let started = Instant::now();
    let cap = match camera::start(&cam.id, format, a.queue) {
        Ok(c) => c,
        Err(e) => {
            println!(
                "  START FAILED after {:.1} s: {e}",
                started.elapsed().as_secs_f64()
            );
            if matches!(e, camera::CameraError::NotAuthorized(_)) {
                println!(
                    "  (no camera access for this terminal: allow it in System Settings → Privacy \
                     & Security → Camera, then run again. Nothing was recorded.)"
                );
            }
            return;
        }
    };
    println!("  backend: {}", cap.describe());

    let path = a.out_dir.join(format!(
        "camera_probe_{}x{}_{}.raw",
        format.width, format.height, format.pixel_format
    ));
    let mut out = match File::create(&path) {
        Ok(f) => BufWriter::with_capacity(4 << 20, f),
        Err(e) => {
            println!("  cannot create {}: {e}", path.display());
            return;
        }
    };

    let begin = Instant::now();
    let deadline = begin + Duration::from_secs(a.seconds);
    let mut first_frame_after = None;
    let mut host_ns: Vec<u64> = Vec::new();
    let mut bytes_written = 0u64;
    let mut sizes: Vec<(u32, u32, PixelFormat)> = Vec::new();
    while Instant::now() < deadline {
        let Some(frame) = cap.frames().pop_timeout(Duration::from_millis(200)) else {
            continue;
        };
        first_frame_after.get_or_insert_with(|| begin.elapsed());
        host_ns.push(frame.host_time_ns);
        let shape = (frame.width, frame.height, frame.pixel_format);
        if !sizes.contains(&shape) {
            sizes.push(shape);
        }
        let _ = frame.with_planes(|planes| {
            for p in planes {
                if out.write_all(p).is_ok() {
                    bytes_written += p.len() as u64;
                }
            }
        });
    }
    let stats = cap.stop();
    let _ = out.flush();
    drop(out);

    println!("  ── statistics ──");
    match first_frame_after {
        Some(t) => println!("  first frame after   {:.0} ms", t.as_secs_f64() * 1000.0),
        None => println!("  first frame after   NEVER (no frames in {} s)", a.seconds),
    }
    println!(
        "  frames              {} received (callback delivered {})",
        host_ns.len(),
        stats.delivered
    );
    println!("  frame shapes        {sizes:?}");
    match interval_stats(&host_ns, format.fps) {
        Some(s) => {
            println!(
                "  measured fps        {:.2}  (asked {:.2})",
                s.fps, format.fps
            );
            println!(
                "  interval            mean {:.2} ms, min {:.2}, p99 {:.2}, max {:.2}",
                s.mean_ms, s.min_ms, s.p99_ms, s.max_ms
            );
            println!("  jitter (std dev)    {:.2} ms", s.jitter_ms);
            println!(
                "  lost (camera gaps)  {}  (intervals of 1.5+ frame periods)",
                s.lost
            );
            if s.non_monotonic > 0 {
                println!(
                    "  NON-MONOTONIC       {} timestamps went backwards",
                    s.non_monotonic
                );
            }
        }
        None => println!("  (too few frames for interval statistics)"),
    }
    println!(
        "  dropped             queue {} · OS {} · skipped {}",
        stats.queue_dropped, stats.dropped_by_os, stats.skipped
    );
    println!(
        "  raw bytes           {bytes_written}  ({})",
        path.display()
    );
    if a.keep {
        println!("  kept {}", path.display());
    } else {
        let _ = std::fs::remove_file(&path);
    }
}

fn main() {
    let a = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("camera_probe: {e}\n(see the header of examples/camera_probe.rs)");
            std::process::exit(2);
        }
    };

    let cameras = match camera::list() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("camera_probe: cannot list cameras: {e}");
            std::process::exit(1);
        }
    };
    println!("{} camera(s):", cameras.len());
    if cameras.is_empty() {
        return;
    }

    let wanted = a.camera.as_ref().map(|s| s.to_lowercase());
    for cam in &cameras {
        println!(
            "\n- {}  [{}]{}\n    id {}",
            cam.name,
            cam.kind.label(),
            cam.model_id
                .as_ref()
                .map(|m| format!("  model {m}"))
                .unwrap_or_default(),
            cam.id
        );
        let selected = wanted.as_ref().is_none_or(|w| {
            cam.name.to_lowercase().contains(w) || cam.id.to_lowercase().contains(w)
        });
        let formats = match camera::formats(&cam.id) {
            Ok(f) => f,
            Err(e) => {
                println!("    formats: {e}");
                continue;
            }
        };
        println!("    {} format(s)", formats.len());
        if a.list_only || !selected {
            if a.list_only {
                print_formats(&formats);
            }
            continue;
        }
        // The summary for a recording run: the distinct sizes at their best fps.
        let mut sizes: Vec<(u32, u32)> = formats.iter().map(|f| (f.width, f.height)).collect();
        sizes.sort_unstable();
        sizes.dedup();
        println!("    sizes: {sizes:?}");

        let chosen = match a.format {
            Some((w, h, fps)) => formats
                .iter()
                .filter(|f| f.width == w && f.height == h && (f.fps - fps).abs() < 0.5)
                .min_by_key(|f| match f.pixel_format {
                    PixelFormat::Nv12 => 0,
                    PixelFormat::Uyvy => 1,
                    PixelFormat::Yuy2 => 2,
                    PixelFormat::Mjpeg => 3,
                    PixelFormat::Other => 9,
                }),
            None => default_format(&formats),
        };
        match chosen {
            Some(f) => record(cam, f, &a),
            None => println!("    no matching format to record"),
        }
    }
}
