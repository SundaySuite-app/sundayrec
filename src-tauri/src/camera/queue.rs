//! The bounded frame queue between a camera callback and its consumer (C1c).
//!
//! The one rule of a camera callback (docs/VIDEOMOTOR.md §C1): **it puts the
//! frame in a bounded queue and returns.** It never waits for the consumer,
//! never allocates a growing buffer, never does I/O. A slow consumer costs the
//! OLDEST frames — they are dropped, and counted — not the camera's thread, and
//! not unbounded memory.
//!
//! [`FrameQueue::push`] therefore holds its lock for a `VecDeque` push/pop and
//! nothing else, and drops an evicted frame only after the lock is released (a
//! frame's `Drop` may release a pool buffer back to the driver).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

/// A bounded, drop-oldest, multi-producer queue.
pub struct FrameQueue<T> {
    items: Mutex<VecDeque<T>>,
    ready: Condvar,
    capacity: usize,
    pushed: AtomicU64,
    dropped: AtomicU64,
    closed: AtomicBool,
}

/// The counters of a [`FrameQueue`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueStats {
    /// Frames offered by the callback.
    pub pushed: u64,
    /// Frames dropped because the queue was full (the oldest each time).
    pub dropped: u64,
}

impl<T> FrameQueue<T> {
    /// A queue holding at most `capacity` frames (at least one).
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            items: Mutex::new(VecDeque::with_capacity(capacity)),
            ready: Condvar::new(),
            capacity,
            pushed: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<T>> {
        // A consumer that panicked mid-pop must not wedge the camera thread.
        self.items.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Put a frame in the queue. **Never blocks on the consumer**: when the
    /// queue is full the oldest frame is dropped (and counted) to make room.
    /// After [`close`](Self::close) the frame is discarded.
    pub fn push(&self, item: T) {
        self.pushed.fetch_add(1, Ordering::Relaxed);
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let evicted = {
            let mut q = self.lock();
            let evicted = if q.len() >= self.capacity {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                q.pop_front()
            } else {
                None
            };
            q.push_back(item);
            evicted
        };
        self.ready.notify_one();
        drop(evicted);
    }

    /// The oldest frame, if any.
    pub fn try_pop(&self) -> Option<T> {
        self.lock().pop_front()
    }

    /// The oldest frame, waiting up to `timeout` for one. `None` on timeout,
    /// and immediately once the queue is closed and empty.
    pub fn pop_timeout(&self, timeout: Duration) -> Option<T> {
        let deadline = Instant::now() + timeout;
        let mut q = self.lock();
        loop {
            if let Some(item) = q.pop_front() {
                return Some(item);
            }
            if self.closed.load(Ordering::Acquire) {
                return None;
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            if left.is_zero() {
                return None;
            }
            q = self
                .ready
                .wait_timeout(q, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// No more frames will be accepted; waiting consumers wake up.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.ready.notify_all();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn stats(&self) -> QueueStats {
        QueueStats {
            pushed: self.pushed.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn keeps_order_and_counts_nothing_dropped_under_capacity() {
        let q = FrameQueue::new(4);
        for i in 0..4 {
            q.push(i);
        }
        assert_eq!(q.len(), 4);
        assert_eq!(
            (0..4).map(|_| q.try_pop().unwrap()).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert_eq!(
            q.stats(),
            QueueStats {
                pushed: 4,
                dropped: 0
            }
        );
        assert!(q.try_pop().is_none());
    }

    #[test]
    fn a_full_queue_drops_the_oldest_and_counts_it() {
        let q = FrameQueue::new(3);
        for i in 0..10 {
            q.push(i);
        }
        assert_eq!(q.len(), 3);
        assert_eq!(
            (0..3).map(|_| q.try_pop().unwrap()).collect::<Vec<_>>(),
            [7, 8, 9]
        );
        assert_eq!(
            q.stats(),
            QueueStats {
                pushed: 10,
                dropped: 7
            }
        );
    }

    #[test]
    fn zero_capacity_is_one() {
        let q = FrameQueue::new(0);
        q.push(1);
        q.push(2);
        assert_eq!(q.capacity(), 1);
        assert_eq!(q.try_pop(), Some(2));
    }

    #[test]
    fn push_never_waits_for_a_consumer_that_never_comes() {
        // The callback's whole contract: a million pushes with nobody popping
        // finish at once, memory stays at `capacity` frames.
        let q = FrameQueue::new(8);
        let start = Instant::now();
        for i in 0..1_000_000u32 {
            q.push(i);
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(q.len(), 8);
        assert_eq!(q.stats().dropped, 1_000_000 - 8);
    }

    #[test]
    fn push_does_not_block_while_the_consumer_is_waiting_for_a_frame() {
        let q = Arc::new(FrameQueue::new(2));
        let consumer = {
            let q = Arc::clone(&q);
            std::thread::spawn(move || q.pop_timeout(Duration::from_secs(5)))
        };
        std::thread::sleep(Duration::from_millis(50));
        q.push(42);
        assert_eq!(consumer.join().unwrap(), Some(42));
    }

    #[test]
    fn pop_times_out_on_an_empty_queue() {
        let q: FrameQueue<u8> = FrameQueue::new(2);
        let start = Instant::now();
        assert!(q.pop_timeout(Duration::from_millis(30)).is_none());
        assert!(start.elapsed() >= Duration::from_millis(25));
    }

    #[test]
    fn close_wakes_a_waiting_consumer_and_discards_later_pushes() {
        let q = Arc::new(FrameQueue::<u8>::new(2));
        let consumer = {
            let q = Arc::clone(&q);
            std::thread::spawn(move || q.pop_timeout(Duration::from_secs(30)))
        };
        std::thread::sleep(Duration::from_millis(50));
        q.close();
        assert_eq!(consumer.join().unwrap(), None);
        q.push(1);
        assert!(q.is_empty());
        assert!(q.is_closed());
    }

    #[test]
    fn frames_queued_before_close_can_still_be_drained() {
        let q = FrameQueue::new(4);
        q.push(1);
        q.push(2);
        q.close();
        assert_eq!(q.pop_timeout(Duration::from_secs(1)), Some(1));
        assert_eq!(q.pop_timeout(Duration::from_secs(1)), Some(2));
        assert_eq!(q.pop_timeout(Duration::from_secs(1)), None);
    }

    #[test]
    fn an_evicted_frame_is_dropped_outside_the_lock() {
        // A frame whose Drop touches the queue would deadlock if the eviction
        // were dropped under the lock.
        struct Probe(Arc<FrameQueue<Probe>>);
        impl Drop for Probe {
            fn drop(&mut self) {
                let _ = self.0.len();
            }
        }
        let q = Arc::new(FrameQueue::new(1));
        q.push(Probe(Arc::clone(&q)));
        q.push(Probe(Arc::clone(&q)));
        assert_eq!(q.stats().dropped, 1);
        // Break the Arc cycle so the test does not leak.
        while q.try_pop().is_some() {}
    }
}
