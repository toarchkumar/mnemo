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
/// Clones share one flag. A cancelled token stays cancelled, so make a new
/// one for the next call.
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
pub(crate) struct Call<'a> {
    deadline: Instant,
    cancel: Option<&'a CancelToken>,
    sleep: Duration,
}

/// The backoff's first and longest sleep. Step 5's bench tunes them (the
/// design note: shorter sleeps while a wait is young).
const FIRST_SLEEP: Duration = Duration::from_millis(1);
const LONGEST_SLEEP: Duration = Duration::from_millis(50);

impl<'a> Call<'a> {
    pub(crate) fn new(deadline: Duration, cancel: Option<&'a CancelToken>) -> Self {
        Call {
            deadline: Instant::now() + deadline,
            cancel,
            sleep: FIRST_SLEEP,
        }
    }

    /// `Err(Cancelled)` once the call is cancelled.
    pub(crate) fn check(&self) -> Result<()> {
        match self.cancel {
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
        let jittered = {
            use rand::Rng;
            let ms = self.sleep.as_secs_f64() * 1000.0;
            Duration::from_secs_f64(rand::thread_rng().gen_range(ms / 2.0..=ms) / 1000.0)
        };
        let d = jittered.min(self.deadline - now);
        let cancelled = match self.cancel {
            Some(c) => c.sleep(d),
            None => {
                std::thread::sleep(d);
                false
            }
        };
        if cancelled {
            return Err(MnemoError::Cancelled);
        }
        self.sleep = (self.sleep * 2).min(LONGEST_SLEEP);
        Ok(Instant::now() < self.deadline)
    }
}
