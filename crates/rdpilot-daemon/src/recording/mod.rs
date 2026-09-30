//! Session recording: owner-only storage of recordings (manifest, event
//! log, video segments) and their retention.

// The recording service that uses the store comes with the recorder.
#![allow(dead_code)]

pub(crate) mod manifest;
pub(crate) mod store;
