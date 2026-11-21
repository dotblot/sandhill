//! Space first (`docs/design/clean-footprint-plan.md`, section 11): before
//! any install, its planner compares the disk's free space with what the
//! install needs and refuses before starting, not halfway.

use std::fmt;
use std::path::Path;

use anyhow::Result;
use sandhill_proto::FailureFix;

/// Room kept free besides what the install unpacks.
const MARGIN_MB: u64 = 500;
/// What R's own installers (CRAN's, Homebrew's) need when their size isn't known.
pub(super) const R_INSTALLER_MB: u64 = 1_000;

/// Not enough free space for an install, in plain words.
#[derive(Debug)]
pub(super) struct NoSpace {
    need_mb: u64,
    free_mb: u64,
}

impl fmt::Display for NoSpace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "This needs about {} and the disk has {} free.",
            size(self.need_mb, true),
            size(self.free_mb, false)
        )
    }
}

impl std::error::Error for NoSpace {}

/// "2.4 GB", "600 MB"; a need rounds up, free space down.
fn size(mb: u64, up: bool) -> String {
    if mb < 1_000 {
        return format!("{mb} MB");
    }
    let tenths = if up { mb.div_ceil(100) } else { mb / 100 };
    if tenths % 10 == 0 {
        format!("{} GB", tenths / 10)
    } else {
        format!("{}.{} GB", tenths / 10, tenths % 10)
    }
}

/// What an install that downloads `download_mb` needs free: three times
/// the download once unpacked, and a margin.
pub(super) fn need_mb(download_mb: Option<u64>) -> u64 {
    download_mb.unwrap_or(0).saturating_mul(3) + MARGIN_MB
}

/// Refuses when the disk holding `folder` (or its nearest existing parent)
/// has less than `need_mb` free. A disk that can't be asked isn't refused
/// here; the planners already say when a folder can't be reached.
pub(super) fn check(folder: &Path, need_mb: u64) -> Result<()> {
    match super::own::free_mb(folder) {
        Ok(free_mb) => check_free(free_mb, need_mb),
        Err(_) => Ok(()),
    }
}

pub(super) fn check_free(free_mb: u64, need_mb: u64) -> Result<()> {
    if free_mb < need_mb {
        return Err(NoSpace { need_mb, free_mb }.into());
    }
    Ok(())
}

/// Free Up Space, for a refusal about space.
pub(super) fn fix(error: &anyhow::Error) -> Option<FailureFix> {
    error
        .downcast_ref::<NoSpace>()
        .map(|_| FailureFix::FreeUpSpace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_refusals_say_the_sizes_in_plain_words_and_offer_free_up_space() {
        assert_eq!(need_mb(None), 500);
        assert_eq!(need_mb(Some(710)), 2_630);
        let error = check_free(1_100, 2_400).unwrap_err();
        assert_eq!(
            error.to_string(),
            "This needs about 2.4 GB and the disk has 1.1 GB free."
        );
        assert_eq!(fix(&error), Some(FailureFix::FreeUpSpace));
        assert_eq!(
            check_free(300, 500).unwrap_err().to_string(),
            "This needs about 500 MB and the disk has 300 MB free."
        );
        assert_eq!(
            (size(1_000, true), size(1_190, false)),
            ("1 GB".into(), "1.1 GB".into())
        );
        assert!(check_free(2_400, 2_400).is_ok());
        assert_eq!(fix(&anyhow::anyhow!("Something else.")), None);
    }
}
