//! One shared-mode call's deadline and cancellation (12.4 design note, "The
//! protocol"): every wait in a call counts against one deadline, and another
//! thread can cancel the call while it waits.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::error::{MnemoError, Result};

/// Cancels a shared-mode call that is waiting for the file's lock, from
/// another thread: an iOS background activity's expiry handler, say. The
/// call releases every lock it holds and returns [`MnemoError::Cancelled`].
///
/// A cancel reaches only a call that is **waiting** for a lock. Once a call
/// holds the lock it runs to its end, so a write is never cut off part-way.
///
/// Clones share one flag. A cancelled token stays cancelled: give the handle
/// a new one for its next calls (`SharedMnemo::set_cancel`).
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<(Mutex<bool>, Condvar)>);

impl CancelToken {
    /// A token that hasn't been cancelled.
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancel every call waiting on this token, now and later.
    pub fn cancel(&self) {
        let (flag, wake) = &*self.0;
        *flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        wake.notify_all();
    }

    /// Whether [`CancelToken::cancel`] has been called.
    pub fn is_cancelled(&self) -> bool {
        *self.0 .0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sleep for `d`, or less if the token is cancelled meanwhile. Returns
    /// whether it was cancelled.
    fn sleep(&self, d: Duration) -> bool {
        let (flag, wake) = &*self.0;
        let guard = flag.lock().unwrap_or_else(|e| e.into_inner());
        if *guard {
            return true;
        }
        let (guard, _) = wake
            .wait_timeout_while(guard, d, |cancelled| !*cancelled)
            .unwrap_or_else(|e| e.into_inner());
        *guard
    }
}

/// One call's deadline, its cancel token, and the backoff between tries.
pub(crate) struct Call {
    started: Instant,
    deadline: Instant,
    cancel: Option<CancelToken>,
    sleep: Duration,
    /// Whether this call has already reopened the sidecar directory once
    /// (RB-007.7).
    pub(crate) sidecar_reopened: bool,
    /// LOCK-2: a writer past the valve's threshold retries every few ms, so
    /// it doesn't sleep through the readers' drain.
    hurried: bool,
}

/// LOCK-2: a hurried writer's sleeps, 1 to 2 ms, jittered.
const HURRIED: (f64, f64) = (1.0, 2.0);

/// The backoff's first and longest sleep. Step 5's bench tunes them (the
/// design note: shorter sleeps while a wait is young).
const FIRST_SLEEP: Duration = Duration::from_millis(1);
const LONGEST_SLEEP: Duration = Duration::from_millis(50);

/// Step 5's alternative shape ("short sleeps while a wait is young"): 1 to
/// 2 ms sleeps for the first `YOUNG_FOR` of a wait, then doubling from
/// `YOUNG_THEN`. Selected for benchmarking only, through
/// `MNEMO_DEV_BACKOFF=young` in a `dev-hooks` build.
const YOUNG_FOR: Duration = Duration::from_millis(15);
const YOUNG_THEN: Duration = Duration::from_millis(2);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Doubling,
    Young,
}

#[cfg(feature = "dev-hooks")]
fn backoff_shape() -> Shape {
    static SHAPE: std::sync::OnceLock<Shape> = std::sync::OnceLock::new();
    *SHAPE.get_or_init(|| match std::env::var("MNEMO_DEV_BACKOFF").as_deref() {
        Ok("young") => Shape::Young,
        _ => Shape::Doubling,
    })
}

#[cfg(not(feature = "dev-hooks"))]
fn backoff_shape() -> Shape {
    Shape::Doubling
}

/// The latest deadline a call gets: a longer one (`Duration::MAX`, say) is
/// clamped to it rather than overflowing the clock.
const LONGEST_DEADLINE: Duration = Duration::from_secs(100 * 365 * 24 * 3600);

impl Call {
    pub(crate) fn new(deadline: Duration, cancel: Option<&CancelToken>) -> Self {
        let now = Instant::now();
        let deadline = now
            .checked_add(deadline.min(LONGEST_DEADLINE))
            .or_else(|| now.checked_add(Duration::from_secs(24 * 3600)))
            .unwrap_or(now);
        Call {
            started: now,
            deadline,
            cancel: cancel.cloned(),
            sleep: FIRST_SLEEP,
            sidecar_reopened: false,
            hurried: false,
        }
    }

    /// How long this call has been waiting.
    pub(crate) fn waited(&self) -> Duration {
        self.started.elapsed()
    }

    /// LOCK-2: from now on, retry every 1 to 2 ms instead of the doubling
    /// backoff (a writer past the valve's threshold).
    pub(crate) fn hurry(&mut self) {
        self.hurried = true;
    }

    /// `Err(Cancelled)` once the call is cancelled.
    pub(crate) fn check(&self) -> Result<()> {
        match &self.cancel {
            Some(c) if c.is_cancelled() => Err(MnemoError::Cancelled),
            _ => Ok(()),
        }
    }

    /// Whether the deadline has passed.
    pub(crate) fn past_deadline(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// Wait before the next try: a jittered sleep that doubles from 1 ms to
    /// 50 ms, cut short by the deadline or a cancel. `Ok(false)` once the
    /// deadline has passed.
    pub(crate) fn back_off(&mut self) -> Result<bool> {
        self.check()?;
        let now = Instant::now();
        if now >= self.deadline {
            return Ok(false);
        }
        let young = backoff_shape() == Shape::Young && now - self.started < YOUNG_FOR;
        let jittered = {
            use rand::Rng;
            // Step 5's alternative: 1 to 2 ms sleeps while the wait is young.
            let (lo, hi) = if self.hurried {
                HURRIED
            } else if young {
                (1.0, 2.0)
            } else {
                let ms = self.sleep.as_secs_f64() * 1000.0;
                (ms / 2.0, ms)
            };
            Duration::from_secs_f64(rand::thread_rng().gen_range(lo..=hi) / 1000.0)
        };
        let d = jittered.min(self.deadline - now);
        let cancelled = match &self.cancel {
            Some(c) => c.sleep(d),
            None => {
                std::thread::sleep(d);
                false
            }
        };
        if cancelled {
            return Err(MnemoError::Cancelled);
        }
        if young {
            self.sleep = YOUNG_THEN;
            return Ok(Instant::now() < self.deadline);
        }
        self.sleep = (self.sleep * 2).min(LONGEST_SLEEP);
        Ok(Instant::now() < self.deadline)
    }
}
