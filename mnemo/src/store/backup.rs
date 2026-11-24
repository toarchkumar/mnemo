//! v0.21.1 (G13): [`Mnemo::backup_to`], a consistent, checked copy of the
//! brain, still encrypted, under a dated name in a backup directory, with
//! the oldest dated copies removed past a count.

use super::*;

/// A backup's name is `mnemo-YYYY-MM-DD.mnemo`.
const PREFIX: &str = "mnemo-";
const SUFFIX: &str = ".mnemo";

impl Mnemo {
    /// Copy the file this handle has open, as it is on disk (still
    /// encrypted, with the brain's key), to `<dir>/mnemo-YYYY-MM-DD.mnemo`,
    /// check the copy, then keep only the newest `opts.keep` dated copies in
    /// `dir`.
    ///
    /// **Consistency comes from the handle's lock.** A plain handle holds the
    /// file's lock for its life: a read-only one (`open_read_only_waiting`,
    /// as `mnemo backup` opens it) the shared lock, so readers run alongside
    /// and writers wait until the handle closes. The copy is read from the
    /// handle's own locked descriptor, never by a second open or a plain
    /// copy. A read-write handle flushes first, so the copy has its writes.
    /// A shared-mode handle's state may be used only inside one of its calls.
    ///
    /// The copy goes to a temp file in `dir` (mode 600), is synced, then
    /// opened read-only with this handle's data key and checked as
    /// [`Mnemo::verify_report`] does (every live record decrypts and
    /// decodes). Only then is it renamed into place, replacing a copy from
    /// the same date, and `dir` synced; so a copy that fails its check is
    /// removed and an earlier copy of that date is kept. Then the dated
    /// copies past `keep` are removed, oldest first; the new copy always
    /// stays, and nothing else in `dir` is touched. `dir` is created (mode
    /// 700) if it's missing.
    ///
    /// Refuses a `dir` inside a git working tree (it or a parent holds
    /// `.git`), where a copy could be committed; the brain's own directory;
    /// `keep` 0; and a `date` that isn't `YYYY-MM-DD`.
    pub fn backup_to(
        &mut self,
        dir: impl AsRef<Path>,
        opts: &BackupOptions,
    ) -> Result<BackupReport> {
        let refuse = |why: String| MnemoError::Invalid(format!("backup refused: {why}"));
        if opts.keep == 0 {
            return Err(refuse(
                "keep must be at least 1 (the new copy counts as one)".into(),
            ));
        }
        let date = match &opts.date {
            Some(d) if is_date(d) => d.clone(),
            Some(d) => return Err(refuse(format!("the date {d:?} isn't YYYY-MM-DD"))),
            None => crate::memory::date_of(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs() as i64),
            ),
        };
        if !self.pager.has_file() {
            return Err(MnemoError::Invalid(
                "backup: a shared-mode handle can back up only inside one of its calls".into(),
            ));
        }

        // Where the copy goes.
        let dir = absolute(dir.as_ref().to_path_buf());
        let mut probe = dir.as_path();
        while std::fs::symlink_metadata(probe).is_err() {
            probe = match probe.parent() {
                Some(p) => p,
                None => break,
            };
        }
        let resolved = std::fs::canonicalize(probe)?;
        if let Some(top) = resolved
            .ancestors()
            .find(|a| std::fs::symlink_metadata(a.join(".git")).is_ok())
        {
            return Err(refuse(format!(
                "{} is inside a git working tree ({}), where a copy could be committed; \
                 choose a directory outside it",
                dir.display(),
                top.display()
            )));
        }
        if dir.exists() {
            if !dir.is_dir() {
                return Err(refuse(format!("{} isn't a directory", dir.display())));
            }
            let brain_dir = std::fs::canonicalize(&self.path)?
                .parent()
                .map(Path::to_path_buf);
            if brain_dir.as_deref() == Some(std::fs::canonicalize(&dir)?.as_path()) {
                return Err(refuse(format!(
                    "{} is the brain's own directory; choose another, off this disk if you can",
                    dir.display()
                )));
            }
        } else {
            let mut b = std::fs::DirBuilder::new();
            b.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                b.mode(0o700);
            }
            b.create(&dir)?;
        }

        // A read-write handle's writes go to the file first.
        if !self.read_only {
            self.flush()?;
        }

        // The copy, to a new temp file in `dir`.
        let dest = dir.join(format!("{PREFIX}{date}{SUFFIX}"));
        let (tmp, mut out) = {
            let mut attempt = 0;
            loop {
                let tmp = temp_sibling(&dest, "backup");
                match owner_only(OpenOptions::new().write(true).create_new(true)).open(&tmp) {
                    Ok(out) => break (tmp, out),
                    Err(e)
                        if e.kind() == std::io::ErrorKind::AlreadyExists
                            && attempt + 1 < TEMP_NAME_ATTEMPTS =>
                    {
                        attempt += 1;
                    }
                    Err(e) => return Err(MnemoError::Io(e)),
                }
            }
        };
        let copied = (|| -> Result<u64> {
            let src = self.pager.file_mut();
            src.seek(SeekFrom::Start(0))?;
            let mut buf = vec![0u8; 1 << 20];
            let mut bytes = 0u64;
            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                bytes += n as u64;
            }
            out.sync_all()?;
            crate::bump_fsync();
            Ok(bytes)
        })();
        // Closed before the check and the rename (Windows renames no file
        // this process has open).
        drop(out);
        let checked = copied.and_then(|bytes| {
            let file = open_with_lock(&tmp, false, Create::No)?;
            let mut copy =
                Mnemo::read_only_from_locked(tmp.clone(), file, &[], Some(self.pager.dek()))?;
            let records = copy.verify_report()?.records;
            copy.close()?;
            Ok((bytes, records))
        });
        let (bytes, records) = match checked {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(MnemoError::Invalid(format!(
                    "backup failed: the copy didn't check out ({e}); it was removed, and \
                     nothing in {} changed",
                    dir.display()
                )));
            }
        };
        if let Err(e) = std::fs::rename(&tmp, &dest) {
            let _ = std::fs::remove_file(&tmp);
            return Err(MnemoError::Io(e));
        }
        #[cfg(unix)]
        {
            File::open(&dir)?.sync_all()?;
            crate::bump_fsync();
        }

        // The rotation: dated copies only, newest first; the new one stays.
        let mut others: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if is_backup_name(&name) && entry.file_type()?.is_file() && dir.join(&name) != dest {
                others.push(name);
            }
        }
        others.sort_unstable_by(|a, b| b.cmp(a));
        let mut kept = vec![dest.clone()];
        let mut removed = Vec::new();
        for (i, name) in others.into_iter().enumerate() {
            let p = dir.join(name);
            if i + 1 < opts.keep {
                kept.push(p);
            } else {
                std::fs::remove_file(&p).map_err(|e| {
                    MnemoError::Invalid(format!(
                        "the backup {} is written and checked, but removing the old copy {} \
                         failed: {e}",
                        dest.display(),
                        p.display()
                    ))
                })?;
                removed.push(p);
            }
        }
        kept.sort_unstable_by(|a, b| b.cmp(a));
        Ok(BackupReport {
            path: dest,
            bytes,
            records,
            kept,
            removed,
        })
    }
}

/// `YYYY-MM-DD`, with a month 01 to 12 and a day 01 to 31.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(0);
    (1..=12).contains(&num(5..7)) && (1..=31).contains(&num(8..10))
}

/// A backup's name exactly: `mnemo-YYYY-MM-DD.mnemo`.
fn is_backup_name(name: &str) -> bool {
    name.strip_prefix(PREFIX)
        .and_then(|r| r.strip_suffix(SUFFIX))
        .is_some_and(is_date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_names() {
        assert!(is_date("2026-11-24"));
        for bad in [
            "2026-13-01",
            "2026-00-10",
            "2026-11-32",
            "2026-1-24",
            "26-11-24x",
            "",
        ] {
            assert!(!is_date(bad), "{bad}");
        }
        assert!(is_backup_name("mnemo-2026-11-24.mnemo"));
        for bad in [
            "mnemo-2026-11-24.mnemo.tmp",
            "project.mnemo",
            "mnemo-latest.mnemo",
            "xmnemo-2026-11-24.mnemo",
            "mnemo-2026-11-24.mnemo.backup-1-0-00000000.tmp",
        ] {
            assert!(!is_backup_name(bad), "{bad}");
        }
    }
}
