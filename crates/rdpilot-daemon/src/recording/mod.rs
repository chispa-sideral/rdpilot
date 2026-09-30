//! Session recording: owner-only storage of recordings (manifest, event
//! log, video segments) and their retention, and the recorder that encodes
//! captured frames into AV1/WebM segments.

// The recording service that uses the store comes with the recorder.
#![allow(dead_code)]

pub(crate) mod capture;
pub(crate) mod encoder;
pub(crate) mod i420;
pub(crate) mod log;
pub(crate) mod manifest;
pub(crate) mod recorder;
pub(crate) mod store;
pub(crate) mod webm;
