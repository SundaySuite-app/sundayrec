//! A camera that comes and goes on command, for the tests of the reconnect
//! (here and in `preview`'s end-to-end test). It is a whole fake platform:
//! a device list, sessions that push frames into the queue they were given
//! while the "physical" camera is attached, and the change signal.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use super::super::queue::FrameQueue;
use super::super::{CameraError, Frame, PixelFormat, VideoFormat};
use super::{lock, BackendCounters, BackendHandle, CameraPlatform};
use crate::audio::device_watch::CameraChanges;

pub(crate) struct FakeWorld {
    /// The stable id of THE camera.
    pub id: String,
    /// The cable is in and the camera works. When false its sessions deliver
    /// nothing (an AVCaptureSession whose device vanished just goes quiet).
    pub attached: AtomicBool,
    /// The camera is attached and listed but delivers nothing (a wedged driver).
    pub frozen: AtomicBool,
    /// Other cameras the list shows.
    pub others: Mutex<Vec<String>>,
    pub changes: Arc<CameraChanges>,
    /// Sessions opened so far, and how many are running now.
    pub opens: AtomicU32,
    pub live_sessions: Arc<AtomicU32>,
    /// The next this-many opens fail ("listed but not ready").
    pub fail_opens: AtomicU32,
    /// The formats an open was asked for (their `Display`).
    pub opened_formats: Mutex<Vec<String>>,
    /// The next open of the ORIGINAL format says it is not offered.
    pub original_format_gone: AtomicBool,
    /// The ids every session was opened for.
    pub opened_ids: Mutex<Vec<String>>,
    /// Time wound forward on the supervisor's and the health's clock, so a
    /// 75 s outage takes no 75 s ([`FakeWorld::advance`]).
    skew: Mutex<Duration>,
    /// How many times the device list has been read (a look of the supervisor
    /// that a test can wait for).
    pub list_reads: AtomicU64,
    pub fps: f64,
    pub size: (u32, u32),
    /// Frames the fake sessions pushed, over all sessions.
    pub pushed: AtomicU64,
    me: Weak<FakeWorld>,
}

impl FakeWorld {
    pub(crate) fn new(id: &str, width: u32, height: u32, fps: f64) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            me: me.clone(),
            id: id.to_owned(),
            attached: AtomicBool::new(true),
            frozen: AtomicBool::new(false),
            others: Mutex::new(Vec::new()),
            changes: Arc::new(CameraChanges::new()),
            opens: AtomicU32::new(0),
            live_sessions: Arc::new(AtomicU32::new(0)),
            fail_opens: AtomicU32::new(0),
            opened_formats: Mutex::new(Vec::new()),
            original_format_gone: AtomicBool::new(false),
            skew: Mutex::new(Duration::ZERO),
            list_reads: AtomicU64::new(0),
            opened_ids: Mutex::new(Vec::new()),
            fps,
            size: (width, height),
            pushed: AtomicU64::new(0),
        })
    }

    pub(crate) fn format(&self) -> VideoFormat {
        VideoFormat::synthetic(
            self.size.0,
            self.size.1,
            self.fps,
            PixelFormat::Nv12,
            "420v",
        )
    }

    /// Wind the clock forward by `by` and wake the supervisor so it looks at
    /// once; returns when it has read the device list (a look on the new time).
    pub(crate) fn advance(&self, by: Duration) {
        *lock(&self.skew) += by;
        let reads = self.list_reads.load(Ordering::SeqCst);
        self.changes.poke();
        let t = std::time::Instant::now();
        while self.list_reads.load(Ordering::SeqCst) == reads
            && t.elapsed() < Duration::from_secs(10)
        {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Pull the cable. `signal`: the OS tells us (otherwise only the missing
    /// frames and the device list give it away).
    pub(crate) fn unplug(&self, signal: bool) {
        self.attached.store(false, Ordering::SeqCst);
        if signal {
            self.changes.poke();
        }
    }

    pub(crate) fn replug(&self, signal: bool) {
        self.attached.store(true, Ordering::SeqCst);
        if signal {
            self.changes.poke();
        }
    }
}

struct FakeSession {
    world: Arc<FakeWorld>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    mode: &'static str,
}

impl BackendHandle for FakeSession {
    fn describe(&self) -> String {
        self.mode.to_owned()
    }
    fn counters(&self) -> BackendCounters {
        BackendCounters {
            dropped_by_os: 0,
            skipped: 1,
        }
    }
    fn stop(mut self: Box<Self>) -> BackendCounters {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        self.world.live_sessions.fetch_sub(1, Ordering::SeqCst);
        BackendCounters {
            dropped_by_os: 0,
            skipped: 1,
        }
    }
}

impl CameraPlatform for FakeWorld {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now() + *lock(&self.skew)
    }

    fn camera_ids(&self) -> Result<Vec<String>, CameraError> {
        self.list_reads.fetch_add(1, Ordering::SeqCst);
        let mut ids = lock(&self.others).clone();
        if self.attached.load(Ordering::SeqCst) {
            ids.push(self.id.clone());
        }
        Ok(ids)
    }

    fn open(
        &self,
        id: &str,
        format: &VideoFormat,
        queue: Arc<FrameQueue<Frame>>,
    ) -> Result<Box<dyn BackendHandle>, CameraError> {
        let world = self.me.upgrade().expect("the world is alive");
        lock(&world.opened_formats).push(format.to_string());
        lock(&world.opened_ids).push(id.to_owned());
        world.opens.fetch_add(1, Ordering::SeqCst);
        if world.fail_opens.load(Ordering::SeqCst) > 0 {
            world.fail_opens.fetch_sub(1, Ordering::SeqCst);
            return Err(CameraError::Backend("not ready yet".into()));
        }
        if id != world.id || !world.attached.load(Ordering::SeqCst) {
            return Err(CameraError::DeviceNotFound(id.to_owned()));
        }
        if format.fps == world.fps && world.original_format_gone.load(Ordering::SeqCst) {
            return Err(CameraError::FormatNotFound(format.to_string()));
        }
        world.live_sessions.fetch_add(1, Ordering::SeqCst);
        let stop = Arc::new(AtomicBool::new(false));
        let (w, h, fps) = (world.size.0, world.size.1, format.fps);
        let join = {
            let (world, stop) = (Arc::clone(&world), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("fake-camera".into())
                .spawn(move || {
                    let period = Duration::from_secs_f64(1.0 / fps.max(1.0));
                    let luma = (w * h) as usize;
                    let mut n = 0u64;
                    while !stop.load(Ordering::SeqCst) {
                        std::thread::sleep(period);
                        if !world.attached.load(Ordering::SeqCst)
                            || world.frozen.load(Ordering::SeqCst)
                        {
                            continue;
                        }
                        let mut bytes = vec![16 + (n % 200) as u8; luma];
                        bytes.resize(luma * 3 / 2, 128);
                        queue.push(Frame::from_bytes(
                            crate::camera::host_now_ns(),
                            w,
                            h,
                            PixelFormat::Nv12,
                            false,
                            bytes,
                        ));
                        world.pushed.fetch_add(1, Ordering::SeqCst);
                        n += 1;
                    }
                })
                .expect("spawn fake camera")
        };
        Ok(Box::new(FakeSession {
            world,
            stop,
            join: Some(join),
            mode: "fake session",
        }))
    }

    fn same_picture_as(&self, _id: &str, wanted: &VideoFormat) -> Result<VideoFormat, CameraError> {
        // The camera now offers the same picture at 29.97.
        Ok(VideoFormat::synthetic(
            wanted.width,
            wanted.height,
            29.97,
            wanted.pixel_format,
            &wanted.native_code,
        ))
    }

    fn changes(&self) -> Arc<CameraChanges> {
        Arc::clone(&self.changes)
    }
}
