//! FMT-9 (b): the key-slot run.
//!
//! Phase 13.2 lets more than one key-encryption key unlock a file (a
//! passphrase and a device key, say, or one per team member). Each slot
//! wraps the same DEK under its own KEK. Slot 0 stays in the header
//! (`salt`, `dek_nonce`, `wrapped_dek`), so passphrase and raw-key opens are
//! unchanged; the others live in this run, which is encrypted under the DEK
//! like any other page run, written copy-on-write, and flipped by the
//! WAL-logged header frame (the header seal covers its pointer).
//!
//! 9a defines the run and persists it; nothing adds slots yet. The `format`
//! byte lets 13.2 extend the slot shape without a v10.

use serde::{Deserialize, Serialize};

use crate::crypto::{NONCE_LEN, SALT_LEN};
use crate::format::WRAPPED_DEK_LEN;

/// Current key-slot run format.
pub(crate) const KEY_SLOT_FORMAT: u8 = 1;

/// One extra key slot.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeySlot {
    /// Caller-chosen label ("laptop", "recovery", …).
    pub label: String,
    /// Role (13.2): 0 = owner. Reserved values for later roles.
    pub role: u8,
    /// Argon2id parameters for this slot's KEK.
    pub m_cost: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt: [u8; SALT_LEN],
    pub dek_nonce: [u8; NONCE_LEN],
    /// The DEK wrapped under this slot's KEK. Stored as a `Vec` because
    /// serde doesn't derive for 48-byte arrays; always `WRAPPED_DEK_LEN`.
    pub wrapped_dek: Vec<u8>,
}

/// The persisted run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeySlotRun {
    pub format: u8,
    pub slots: Vec<KeySlot>,
}

impl Default for KeySlotRun {
    fn default() -> Self {
        KeySlotRun {
            format: KEY_SLOT_FORMAT,
            slots: Vec::new(),
        }
    }
}

impl KeySlotRun {
    /// Check the shape after decoding (a hostile run must not panic later).
    pub fn validate(&self) -> crate::Result<()> {
        if self.format != KEY_SLOT_FORMAT {
            return Err(crate::MnemoError::Invalid(format!(
                "key-slot run format {} is newer than this build understands",
                self.format
            )));
        }
        if self
            .slots
            .iter()
            .any(|s| s.wrapped_dek.len() != WRAPPED_DEK_LEN)
        {
            return Err(crate::MnemoError::Invalid(
                "key-slot run: wrapped DEK has the wrong length".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_slot_run_round_trips() {
        let run = KeySlotRun {
            format: KEY_SLOT_FORMAT,
            slots: vec![KeySlot {
                label: "recovery".into(),
                role: 0,
                m_cost: 8,
                t_cost: 1,
                p_cost: 1,
                salt: [1; SALT_LEN],
                dek_nonce: [2; NONCE_LEN],
                wrapped_dek: vec![3; WRAPPED_DEK_LEN],
            }],
        };
        let bytes = rmp_serde::to_vec(&run).unwrap();
        let back: KeySlotRun = rmp_serde::from_slice(&bytes).unwrap();
        back.validate().unwrap();
        assert_eq!(back, run);
        let mut bad = run.clone();
        bad.format = 2;
        assert!(bad.validate().is_err());
        let mut bad = run;
        bad.slots[0].wrapped_dek.pop();
        assert!(bad.validate().is_err());
    }
}
