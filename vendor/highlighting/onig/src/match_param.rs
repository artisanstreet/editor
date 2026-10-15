//! Match Parameters
//!
//! Contains the definition for the `MatchParam` struct. This can be
//! used to control the behavior of searching and matching.

use std::os::raw::{c_uint, c_ulong};

/// Parameters for a Match or Search.
pub struct MatchParam {
    raw: *mut onig_sys::OnigMatchParam,
}

impl MatchParam {
    /// Set the match stack limit
    pub fn set_match_stack_limit(&mut self, limit: u32) {
        unsafe {
            onig_sys::onig_set_match_stack_limit_size_of_match_param(self.raw, limit as c_uint);
        }
    }

    /// Set the retry limit in match
    pub fn set_retry_limit_in_match(&mut self, limit: u32) {
        unsafe {
            onig_sys::onig_set_retry_limit_in_match_of_match_param(self.raw, c_ulong::from(limit));
        }
    }

    /// Set the retry limit across every candidate position in one search.
    /// Returns an error if the linked engine lacks retry-limit support.
    pub fn set_retry_limit_in_search(&mut self, limit: u32) -> Result<(), crate::Error> {
        // SAFETY: self owns this initialized match parameter until Drop, and the
        // setter stores only the integer limit in that parameter.
        let code = unsafe {
            onig_sys::onig_set_retry_limit_in_search_of_match_param(self.raw, c_ulong::from(limit))
        };
        if code == onig_sys::ONIG_NORMAL as i32 {
            Ok(())
        } else {
            Err(crate::Error::from_code(code))
        }
    }

    /// Get the Raw `OnigMatchParam` Pointer
    pub fn as_raw(&self) -> *mut onig_sys::OnigMatchParam {
        self.raw
    }
}

impl Default for MatchParam {
    fn default() -> Self {
        let raw = unsafe {
            let new = onig_sys::onig_new_match_param();
            onig_sys::onig_initialize_match_param(new);
            new
        };
        MatchParam { raw }
    }
}

impl Drop for MatchParam {
    fn drop(&mut self) {
        unsafe {
            onig_sys::onig_free_match_param(self.raw);
        }
    }
}

#[cfg(test)]
mod test {

    use super::*;

    #[test]
    pub fn create_default_match_param() {
        let _mp = MatchParam::default();
    }

    #[test]
    pub fn set_max_stack_size_limit() {
        let mut mp = MatchParam::default();
        mp.set_match_stack_limit(1000);
    }

    #[test]
    pub fn set_retry_limit_in_match() {
        let mut mp = MatchParam::default();
        mp.set_retry_limit_in_match(1000);
    }
}
