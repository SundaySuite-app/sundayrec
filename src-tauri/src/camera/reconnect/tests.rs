//! The reconnect, against a camera that comes and goes on command
//! ([`super::testkit::FakeWorld`]). These prove WHAT happens — the same queue,
//! the same id, no restart of anything else — not how fast: every wait is a
//! generous deadline, so a loaded CI runner cannot fail them for being slow.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::testkit::FakeWorld;
use crate::camera::{start_with, Capture};

fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let t = Instant::now();
    while !ready() {
        assert!(
            t.elapsed() < Duration::from_secs(30),
            "timed out waiting for: {what}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn open(world: &Arc<FakeWorld>) -> Capture {
    start_with(world.clone(), &world.id, &world.format(), 30).expect("the camera opens")
}

/// Frames pushed by the world so far.
fn pushed(w: &FakeWorld) -> u64 {
    w.pushed.load(Ordering::SeqCst)
}

/// Wait for a few more frames than there are now.
fn wait_for_frames(w: &FakeWorld, what: &str) {
    let before = pushed(w);
    wait_for(what, || pushed(w) >= before + 5);
}

#[test]
fn a_camera_that_vanishes_and_returns_is_reopened_on_the_same_id_into_the_same_queue() {
    let world = FakeWorld::new("cam-A", 64, 48, 60.0);
    let cap = open(&world);
    let queue = Arc::clone(cap.frames());
    wait_for_frames(&world, "frames at the start");
    assert_eq!(cap.health().lost_for, None);

    world.unplug(true);
    wait_for("the outage is noticed", || cap.health().lost_for.is_some());
    assert_eq!(cap.health().outages, 1);
    wait_for("the dead session is closed", || {
        world.live_sessions.load(Ordering::SeqCst) == 0
    });
    assert_eq!(cap.describe(), "", "no session while the camera is lost");

    world.replug(true);
    wait_for("the camera is opened again", || {
        world.live_sessions.load(Ordering::SeqCst) == 1 && cap.health().lost_for.is_none()
    });
    assert!(Arc::ptr_eq(&queue, cap.frames()), "the same queue");
    let before = queue.stats().pushed;
    wait_for("frames flow into the old queue again", || {
        queue.stats().pushed >= before + 5
    });

    // Both sessions opened the SAME id, on the SAME format.
    assert_eq!(*world.opened_ids.lock().unwrap(), ["cam-A", "cam-A"]);
    let formats = world.opened_formats.lock().unwrap().clone();
    assert_eq!(formats[0], formats[1]);
    assert_eq!(world.opens.load(Ordering::SeqCst), 2);

    // The totals survive the session that ended: one skipped buffer per session.
    let stats = cap.stop();
    assert_eq!(stats.skipped, 2, "{stats:?}");
    assert_eq!(world.live_sessions.load(Ordering::SeqCst), 0);
    assert!(queue.is_closed());
}

#[test]
fn an_outage_is_noticed_and_mended_without_any_signal_from_the_os() {
    // No listener (or one that failed to install): the frames stopping and the
    // device list are enough.
    let world = FakeWorld::new("cam-A", 64, 48, 60.0);
    let cap = open(&world);
    wait_for_frames(&world, "frames at the start");
    world.unplug(false);
    wait_for("the outage is noticed", || cap.health().lost_for.is_some());
    world.replug(false);
    wait_for("the camera is opened again", || {
        world.live_sessions.load(Ordering::SeqCst) == 1 && cap.health().lost_for.is_none()
    });
    wait_for_frames(&world, "frames again");
    cap.stop();
}

#[test]
fn another_camera_is_never_taken_for_the_one_that_left() {
    let world = FakeWorld::new("cam-A", 64, 48, 60.0);
    let cap = open(&world);
    wait_for_frames(&world, "frames at the start");
    world.unplug(true);
    // A different camera appears in its place, and the OS says so.
    world.others.lock().unwrap().push("cam-B".into());
    world.changes.poke();
    wait_for("the outage is noticed", || cap.health().lost_for.is_some());
    // Give the supervisor plenty of looks: it must not open anything.
    for _ in 0..4 {
        world.changes.poke();
        std::thread::sleep(Duration::from_millis(300));
    }
    assert_eq!(
        world.opens.load(Ordering::SeqCst),
        1,
        "no session was opened"
    );
    assert_eq!(world.live_sessions.load(Ordering::SeqCst), 0);
    assert!(cap.health().lost_for.is_some());
    // The right one comes back.
    world.replug(true);
    wait_for("the right camera is opened again", || {
        world.live_sessions.load(Ordering::SeqCst) == 1
    });
    assert_eq!(*world.opened_ids.lock().unwrap(), ["cam-A", "cam-A"]);
    cap.stop();
}

#[test]
fn a_camera_that_is_listed_but_not_ready_is_tried_again_until_it_opens() {
    let world = FakeWorld::new("cam-A", 64, 48, 60.0);
    let cap = open(&world);
    wait_for_frames(&world, "frames at the start");
    world.unplug(true);
    wait_for("the outage is noticed", || cap.health().lost_for.is_some());
    wait_for("the dead session is closed", || {
        world.live_sessions.load(Ordering::SeqCst) == 0
    });
    world.fail_opens.store(3, Ordering::SeqCst);
    world.replug(true);
    wait_for("the camera finally opens", || {
        world.live_sessions.load(Ordering::SeqCst) == 1 && cap.health().lost_for.is_none()
    });
    assert_eq!(world.opens.load(Ordering::SeqCst), 1 + 3 + 1);
    cap.stop();
}

#[test]
fn a_camera_that_is_listed_but_silent_gets_a_fresh_session() {
    let world = FakeWorld::new("cam-A", 64, 48, 60.0);
    let cap = open(&world);
    wait_for_frames(&world, "frames at the start");
    // A wedged driver: attached, listed, no frames.
    world.frozen.store(true, Ordering::SeqCst);
    wait_for("the stall is noticed", || cap.health().outages == 1);
    world.frozen.store(false, Ordering::SeqCst);
    wait_for("a new session delivers", || {
        world.opens.load(Ordering::SeqCst) == 2 && cap.health().lost_for.is_none()
    });
    wait_for_frames(&world, "frames again");
    cap.stop();
}

#[test]
fn a_format_that_is_no_longer_offered_falls_to_the_same_picture_at_the_nearest_rate() {
    let world = FakeWorld::new("cam-A", 64, 48, 60.0);
    let cap = open(&world);
    wait_for_frames(&world, "frames at the start");
    world.unplug(true);
    wait_for("the outage is noticed", || cap.health().lost_for.is_some());
    wait_for("the dead session is closed", || {
        world.live_sessions.load(Ordering::SeqCst) == 0
    });
    world.original_format_gone.store(true, Ordering::SeqCst);
    world.replug(true);
    wait_for("the camera is opened on the other format", || {
        world.live_sessions.load(Ordering::SeqCst) == 1
    });
    let formats = world.opened_formats.lock().unwrap().clone();
    assert!(formats.last().unwrap().contains("29.97"), "{formats:?}");
    // The picture is the same size and layout.
    assert!(formats.last().unwrap().contains("64x48"), "{formats:?}");
    cap.stop();
}

#[test]
fn stopping_during_an_outage_leaves_nothing_behind() {
    let world = FakeWorld::new("cam-A", 64, 48, 60.0);
    let cap = open(&world);
    wait_for_frames(&world, "frames at the start");
    world.unplug(true);
    wait_for("the outage is noticed", || cap.health().lost_for.is_some());
    cap.stop();
    // The supervisor ends on its own: the world is no longer held by it.
    world.replug(true);
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(
        world.opens.load(Ordering::SeqCst),
        1,
        "no session after the stop"
    );
    assert_eq!(world.live_sessions.load(Ordering::SeqCst), 0);
    wait_for("the supervisor has let go", || {
        Arc::strong_count(&world) == 1
    });
}

#[test]
fn stopping_a_healthy_capture_stops_its_session_once() {
    let world = FakeWorld::new("cam-A", 64, 48, 60.0);
    let cap = open(&world);
    wait_for_frames(&world, "frames at the start");
    let stats = cap.stop();
    assert_eq!(world.live_sessions.load(Ordering::SeqCst), 0);
    assert_eq!(stats.skipped, 1);
    wait_for("the supervisor has let go", || {
        Arc::strong_count(&world) == 1
    });
}

/// THE proof on a real camera (C5a, RIG-DAY (c5a)). Needs a person with a cable
/// (or a Continuity iPhone) and camera permission for the terminal, so it is
/// never part of `cargo test`. From the owner's own terminal:
///
/// ```text
/// SUNDAYREC_REAL_CAMERA=iPhone cargo test -p sundayrec --lib \
///     real_camera_comes_back_on_the_same_id -- --ignored --nocapture
/// ```
///
/// (`SUNDAYREC_REAL_CAMERA` is a part of the camera's name or its id;
/// `SUNDAYREC_OUTAGE_WINDOW_SECS` is how long the test waits, default 120.)
/// It opens the camera with the recorder's own `open_camera`, prints what it
/// sees every second, and passes once the camera has dropped out and come back
/// on the SAME id, with frames flowing into the SAME queue again.
#[test]
#[ignore = "needs a real camera, a person to pull its cable, and camera permission"]
fn real_camera_comes_back_on_the_same_id() {
    use crate::recorder::native_capture::video::open_camera;
    use sundayrec_core::video_conform::Fps;

    let name = std::env::var("SUNDAYREC_REAL_CAMERA").expect("set SUNDAYREC_REAL_CAMERA");
    let window = std::env::var("SUNDAYREC_OUTAGE_WINDOW_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(120);
    let cams = crate::camera::list().expect("camera list");
    let cam = crate::recorder::native_capture::video::find_camera(&cams, &name)
        .unwrap_or_else(|| panic!("no camera matching {name}: {cams:?}"));
    println!("camera: {} — id {}", cam.report_line(), cam.id);
    let (capture, spec) = open_camera(&name, 1920, 1080, Fps::new(30, 1)).expect("opens");
    println!(
        "session: {} ({}x{})",
        capture.describe(),
        spec.width,
        spec.height
    );
    let queue = Arc::clone(capture.frames());
    let drain = {
        let queue = Arc::clone(&queue);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s2 = Arc::clone(&stop);
        let join = std::thread::spawn(move || {
            while !s2.load(Ordering::Relaxed) {
                drop(queue.pop_timeout(Duration::from_millis(50)));
            }
        });
        (stop, join)
    };

    println!(">>> Frames flow now. In the next {window} s: switch the camera OFF (pull the USB cable / unlock-and-move the iPhone away),");
    println!(">>> wait for LOST, then switch it back ON. The test ends by itself.");
    let t0 = Instant::now();
    let (mut last_frames, mut last_t) = (queue.stats().pushed, Instant::now());
    let (mut lost_seen, mut back_seen) = (false, false);
    let mut frames_after_back = 0u64;
    while t0.elapsed() < Duration::from_secs(window) {
        std::thread::sleep(Duration::from_secs(1));
        let frames = queue.stats().pushed;
        let fps = (frames - last_frames) as f64 / last_t.elapsed().as_secs_f64();
        (last_frames, last_t) = (frames, Instant::now());
        let h = capture.health();
        let listed: Vec<String> = crate::camera::list()
            .map(|l| l.into_iter().map(|c| c.id).collect())
            .unwrap_or_default();
        let state = match h.lost_for {
            Some(d) => format!("LOST for {:.1}s", d.as_secs_f64()),
            None => "live".into(),
        };
        println!(
            "[{:5.1}s] {state:<16} {fps:5.1} fps  outages={} session={:?}  our id listed: {}  (list: {listed:?})",
            t0.elapsed().as_secs_f64(),
            h.outages,
            capture.describe(),
            listed.contains(&cam.id)
        );
        if h.lost_for.is_some() {
            lost_seen = true;
        }
        if lost_seen && h.lost_for.is_none() && fps > 5.0 {
            back_seen = true;
        }
        if back_seen {
            frames_after_back += (fps) as u64;
            if frames_after_back >= 90 {
                break;
            }
        }
    }
    drain.0.store(true, Ordering::Relaxed);
    let _ = drain.1.join();
    let health = capture.health();
    let stats = capture.stop();
    println!("final: {health:?} {stats:?}");
    assert!(
        lost_seen,
        "the camera was never seen to drop out — did you pull the cable?"
    );
    assert!(
        back_seen,
        "the camera dropped out but did not come back on id {}",
        cam.id
    );
    assert!(health.outages >= 1);
    println!(
        "RESULT: PASS — the camera came back on the same id ({})",
        cam.id
    );
}

// ── M3R #3: a camera that is listed but says nothing ─────────────────────────

/// An HDMI card whose source is off, a phone that locked: listed, opens, and
/// delivers nothing. It used to be "lost" for ≤ 5 s at a time (the clock was
/// reset by every open, not by a picture), so the 60 s alert never came and the
/// message flickered. Time is the world's wound-forward clock, not a wait.
#[test]
fn a_listed_camera_that_delivers_nothing_is_lost_for_as_long_as_it_is_silent() {
    let world = FakeWorld::new("cam-A", 64, 48, 60.0);
    let cap = open(&world);
    wait_for_frames(&world, "frames at the start");
    assert_eq!(cap.health().lost_for, None);

    world.frozen.store(true, Ordering::SeqCst);
    let mut longest = Duration::ZERO;
    let mut shortest_after_noticed: Option<Duration> = None;
    let mut alert = sundayrec_core::notify::CameraOutageWatch::default();
    let mut alerted = false;
    for _ in 0..120 {
        world.advance(Duration::from_secs(1));
        let lost = cap.health().lost_for;
        if let Some(d) = lost {
            longest = longest.max(d);
            // Once noticed it never flips back to "fine" while it is silent.
            shortest_after_noticed = Some(shortest_after_noticed.map_or(d, |s| s.min(d)));
        } else {
            assert!(
                shortest_after_noticed.is_none(),
                "the message blinked: lost, then fine, with no picture in between"
            );
        }
        if alert.observe(lost) {
            alerted = true;
            break;
        }
    }
    assert!(longest >= Duration::from_secs(60), "lost for {longest:?}");
    assert!(alerted, "the 60 s alert never came");
    assert!(
        world.opens.load(Ordering::SeqCst) >= 3,
        "it kept trying to open the camera"
    );
    assert_eq!(cap.health().outages, 1, "one outage, not one per open");

    // The signal comes back: the first picture ends the outage.
    world.frozen.store(false, Ordering::SeqCst);
    wait_for("the first picture ends the outage", || {
        world.advance(Duration::from_millis(500));
        cap.health().lost_for.is_none()
    });
    let _ = cap.stop();
}

// ── M3R #4: the camera is out when a fragment starts ─────────────────────────

/// A fragment boundary while the camera is out and nobody watches: the old
/// capture is let go of, the next fragment's camera cannot open. It used to
/// have no camera at all for the rest of the recording, and the message said
/// everything was fine. Now it is a capture that is lost from the start, says
/// so, and opens the SAME id when it is back.
#[test]
fn a_fragment_that_starts_while_the_camera_is_out_connects_when_it_is_back() {
    let world = FakeWorld::new("cam-A", 64, 48, 60.0);
    let cap = open(&world);
    wait_for_frames(&world, "frames at the start");
    world.unplug(true);
    wait_for("the outage is noticed", || cap.health().lost_for.is_some());
    // The fragment ends: the unwatched camera is let go of, as `stop_camera` does.
    crate::camera::note_source_stopped(cap.health().lost_for);
    let _ = cap.stop();
    assert_eq!(world.live_sessions.load(Ordering::SeqCst), 0);

    // The next fragment: the camera is not there.
    let next = crate::camera::start_lost_with(world.clone(), &world.id, &world.format(), 30);
    let health = next.health();
    assert!(health.lost_for.is_some(), "it says the camera is out");
    assert_eq!(health.outages, 1);
    assert_eq!(next.describe(), "", "no session yet");

    world.replug(true);
    wait_for("the same id is opened", || {
        world.live_sessions.load(Ordering::SeqCst) == 1
    });
    let queue = Arc::clone(next.frames());
    wait_for("pictures arrive in the new fragment's queue", || {
        queue.stats().pushed >= 5
    });
    wait_for("the outage is over", || next.health().lost_for.is_none());
    assert_eq!(
        world.opened_ids.lock().unwrap().last().map(String::as_str),
        Some("cam-A")
    );
    let _ = next.stop();
    assert_eq!(world.live_sessions.load(Ordering::SeqCst), 0);
}

#[test]
fn a_camera_that_has_run_is_found_again_by_name_when_it_is_no_longer_listed() {
    use crate::camera::{CameraInfo, CameraKind};
    let info = CameraInfo {
        id: "m3r-test-cam-id".into(),
        name: "M3R Test Cam".into(),
        model_id: None,
        kind: CameraKind::Camera,
    };
    let other = CameraInfo {
        id: "m3r-test-other-id".into(),
        name: "M3R Test Other".into(),
        model_id: None,
        kind: CameraKind::Camera,
    };
    crate::camera::remember_seen_for_test(&[info.clone(), other]);
    // Listed but never started: not a camera the recording could have run on.
    assert!(crate::camera::remembered_for("M3R Test Cam").is_none());
    crate::camera::remember_started_for_test(&info.id, &FakeWorld::new("x", 64, 48, 30.0).format());
    let found = crate::camera::remembered_for("m3r test cam").expect("remembered");
    assert_eq!(found.id, "m3r-test-cam-id");
    assert!(crate::camera::remembered_for("M3R Test Other").is_none());
    assert!(crate::camera::remembered_for("Something Else").is_none());
}
