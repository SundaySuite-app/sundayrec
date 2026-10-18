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
