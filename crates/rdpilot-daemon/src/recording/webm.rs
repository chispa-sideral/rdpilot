//! A single-track WebM (Matroska) writer for one self-contained AV1 video
//! segment.
//!
//! Layout: EBML header (DocType `webm`); one Segment with SeekHead (Info,
//! Tracks, Cues), Info (1 ms timestamps, Duration), Tracks (one `V_AV1`
//! track whose `CodecPrivate` is the AV1 configuration record followed by
//! the sequence header OBU, BT.709 limited-range colour), Clusters of
//! SimpleBlocks with millisecond timestamps relative to the segment start,
//! and Cues. The last frame is held back and written as a BlockGroup whose
//! BlockDuration reaches the segment's end, so a still display keeps its
//! last frame on screen.
//!
//! The file is written as `NNNNNN.webm.part`; [`SegmentWriter::finish`]
//! patches the sizes and positions, flushes it to disk and renames it to
//! `NNNNNN.webm`. A `.part` file is never read as a segment.

use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::encoder::sequence_header_obu;
use super::store;

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POSITION: u32 = 0x53AC;
const INFO: u32 = 0x1549_A966;
const TRACKS: u32 = 0x1654_AE6B;
const CLUSTER: u32 = 0x1F43_B675;
const CUES: u32 = 0x1C53_BB6B;

/// Largest relative block timestamp; a new Cluster starts before it.
const MAX_RELATIVE: u64 = 32_767;

fn id_bytes(id: u32) -> Vec<u8> {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count();
    bytes[skip..].to_vec()
}

/// A minimal EBML size.
fn size_bytes(n: u64) -> Vec<u8> {
    for len in 1..=8_u32 {
        let max = (1_u64 << (7 * len)) - 1;
        if n < max {
            let marked = n | (1_u64 << (7 * len));
            return marked.to_be_bytes()[(8 - len as usize)..].to_vec();
        }
    }
    fixed_size(n).to_vec()
}

/// An 8-byte EBML size (patchable).
fn fixed_size(n: u64) -> [u8; 8] {
    let mut out = n.to_be_bytes();
    out[0] = 0x01;
    out
}

fn element(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = id_bytes(id);
    out.extend(size_bytes(payload.len() as u64));
    out.extend_from_slice(payload);
    out
}

fn uint(id: u32, value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let skip = bytes.iter().take_while(|b| **b == 0).count().min(7);
    element(id, &bytes[skip..])
}

fn text(id: u32, value: &str) -> Vec<u8> {
    element(id, value.as_bytes())
}

fn ebml_header() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend(uint(0x4286, 1)); // EBMLVersion
    body.extend(uint(0x42F7, 1)); // EBMLReadVersion
    body.extend(uint(0x42F2, 4)); // EBMLMaxIDLength
    body.extend(uint(0x42F3, 8)); // EBMLMaxSizeLength
    body.extend(text(0x4282, "webm")); // DocType
    body.extend(uint(0x4287, 4)); // DocTypeVersion
    body.extend(uint(0x4285, 2)); // DocTypeReadVersion
    element(EBML, &body)
}

fn tracks(width: u32, height: u32, codec_private: &[u8]) -> Vec<u8> {
    let mut colour = Vec::new();
    colour.extend(uint(0x55B1, 1)); // MatrixCoefficients: BT.709
    colour.extend(uint(0x55B9, 1)); // Range: broadcast (limited)
    colour.extend(uint(0x55BA, 1)); // TransferCharacteristics: BT.709
    colour.extend(uint(0x55BB, 1)); // Primaries: BT.709
    let mut video = Vec::new();
    video.extend(uint(0xB0, u64::from(width))); // PixelWidth
    video.extend(uint(0xBA, u64::from(height))); // PixelHeight
    video.extend(element(0x55B0, &colour)); // Colour
    let mut entry = Vec::new();
    entry.extend(uint(0xD7, 1)); // TrackNumber
    entry.extend(uint(0x73C5, 1)); // TrackUID
    entry.extend(uint(0x83, 1)); // TrackType: video
    entry.extend(uint(0x9C, 0)); // FlagLacing
    entry.extend(text(0x86, "V_AV1")); // CodecID
    entry.extend(element(0x63A2, codec_private)); // CodecPrivate
    entry.extend(element(0xE0, &video)); // Video
    element(TRACKS, &element(0xAE, &entry))
}

fn block_payload(relative: u64, key: bool, data: &[u8]) -> Vec<u8> {
    let rel = i16::try_from(relative).unwrap_or(i16::MAX);
    let mut out = vec![0x81]; // track 1
    out.extend_from_slice(&rel.to_be_bytes());
    out.push(if key { 0x80 } else { 0x00 });
    out.extend_from_slice(data);
    out
}

struct Held {
    timestamp: u64,
    key: bool,
    data: Vec<u8>,
    /// Timestamp of the frame before it, which it depends on.
    previous: Option<u64>,
}

/// One frame as a SimpleBlock, or as a BlockGroup when it has a duration.
struct Block {
    key: bool,
    data: Vec<u8>,
    duration: Option<u64>,
    /// Relative timestamp of the frame it depends on (negative).
    reference: Option<i64>,
}

impl Block {
    fn encode(self, relative: u64) -> Vec<u8> {
        let Some(duration) = self.duration else {
            return element(0xA3, &block_payload(relative, self.key, &self.data));
            // SimpleBlock
        };
        let mut group = element(0xA1, &block_payload(relative, false, &self.data)); // Block
        group.extend(uint(0x9B, duration)); // BlockDuration
        if let Some(reference) = self.reference {
            // ReferenceBlock: without it a BlockGroup counts as a key frame.
            group.extend(element(0xFB, &reference.to_be_bytes()));
        }
        element(0xA0, &group) // BlockGroup
    }
}

struct Cluster {
    timestamp: u64,
    body: Vec<u8>,
}

/// Writes one segment file.
pub(crate) struct SegmentWriter {
    file: File,
    part: PathBuf,
    done: PathBuf,
    width: u32,
    height: u32,
    codec_config: Vec<u8>,
    /// File offset of the Segment's data (0 until the header is written).
    data_start: u64,
    size_at: u64,
    duration_at: u64,
    cues_position_at: u64,
    written: u64,
    cluster: Option<Cluster>,
    held: Option<Held>,
    cues: Vec<(u64, u64)>,
    frames: u64,
    last_timestamp: u64,
}

impl SegmentWriter {
    /// Create `segments_dir/NNNNNN.webm.part` for frames of `width` x
    /// `height`. `codec_config` is the encoder's 4-byte configuration record.
    pub(crate) fn create(
        segments_dir: &Path,
        number: u32,
        width: u32,
        height: u32,
        codec_config: Vec<u8>,
    ) -> io::Result<Self> {
        let done = segments_dir.join(store::segment_file(number));
        let mut part = done.as_os_str().to_owned();
        part.push(".part");
        let part = PathBuf::from(part);
        let file = store::open_private_file(&part, true, false)?;
        Ok(SegmentWriter {
            file,
            part,
            done,
            width,
            height,
            codec_config,
            data_start: 0,
            size_at: 0,
            duration_at: 0,
            cues_position_at: 0,
            written: 0,
            cluster: None,
            held: None,
            cues: Vec::new(),
            frames: 0,
            last_timestamp: 0,
        })
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    fn write_header(&mut self, first_packet: &[u8]) -> io::Result<()> {
        let mut codec_private = self.codec_config.clone();
        if let Some(obu) = sequence_header_obu(first_packet) {
            codec_private.extend_from_slice(obu);
        }
        self.put(&ebml_header())?;
        let mut head = id_bytes(SEGMENT);
        self.size_at = self.written + head.len() as u64;
        head.extend_from_slice(&fixed_size(0));
        self.put(&head)?;
        self.data_start = self.written;

        // SeekHead with fixed-size positions: Info, Tracks, then Cues.
        let seek = |id: u32| -> Vec<u8> {
            let mut body = element(SEEK_ID, &id_bytes(id));
            body.extend(id_bytes(SEEK_POSITION));
            body.extend(size_bytes(8));
            body.extend_from_slice(&[0_u8; 8]);
            element(SEEK, &body)
        };
        let entries = [seek(INFO), seek(TRACKS), seek(CUES)];
        let body: Vec<u8> = entries.concat();
        let seek_head = element(SEEK_HEAD, &body);
        let body_start = seek_head.len() - body.len();
        // Offset of each SeekPosition payload inside `seek_head`.
        let mut position_at = Vec::new();
        let mut at = body_start;
        for entry in &entries {
            position_at.push(at + entry.len() - 8);
            at += entry.len();
        }
        let head_at = self.written;
        let info_pos = self.written + seek_head.len() as u64 - self.data_start;
        let mut info_body = Vec::new();
        info_body.extend(uint(0x2A_D7B1, 1_000_000)); // TimestampScale: 1 ms
        info_body.extend(text(0x4D80, "rdpilot")); // MuxingApp
        info_body.extend(text(0x5741, "rdpilot")); // WritingApp
        let duration_rel = info_body.len();
        info_body.extend(id_bytes(0x4489)); // Duration (float, patched)
        info_body.extend(size_bytes(8));
        info_body.extend_from_slice(&0_f64.to_be_bytes());
        let info = element(INFO, &info_body);
        let tracks_pos = info_pos + info.len() as u64;
        let tracks = tracks(self.width, self.height, &codec_private);

        let mut seek_head = seek_head;
        seek_head[position_at[0]..position_at[0] + 8].copy_from_slice(&info_pos.to_be_bytes());
        seek_head[position_at[1]..position_at[1] + 8].copy_from_slice(&tracks_pos.to_be_bytes());
        self.cues_position_at = head_at + position_at[2] as u64;
        self.put(&seek_head)?;
        let info_start = self.written;
        self.duration_at = info_start + (info.len() - info_body.len() + duration_rel) as u64 + 3;
        self.put(&info)?;
        self.put(&tracks)?;
        Ok(())
    }

    fn flush_cluster(&mut self) -> io::Result<()> {
        if let Some(cluster) = self.cluster.take() {
            let position = self.written - self.data_start;
            if self.cues.is_empty() {
                self.cues.push((cluster.timestamp, position));
            }
            let mut body = uint(0xE7, cluster.timestamp); // Timestamp
            body.extend(cluster.body);
            self.put(&element(CLUSTER, &body))?;
        }
        Ok(())
    }

    fn emit(&mut self, timestamp: u64, block: Block) -> io::Result<()> {
        let fits = self
            .cluster
            .as_ref()
            .is_some_and(|c| timestamp - c.timestamp <= MAX_RELATIVE);
        if !fits {
            self.flush_cluster()?;
            self.cluster = Some(Cluster {
                timestamp,
                body: Vec::new(),
            });
        }
        if let Some(cluster) = &mut self.cluster {
            cluster
                .body
                .extend(block.encode(timestamp - cluster.timestamp));
        }
        Ok(())
    }

    /// Add one frame at `timestamp` ms after the segment start (not before
    /// the previous frame). The first frame must be the key frame.
    pub(crate) fn write_frame(&mut self, timestamp: u64, data: &[u8], key: bool) -> io::Result<()> {
        if self.data_start == 0 {
            self.write_header(data)?;
        }
        let timestamp = timestamp.max(self.last_timestamp);
        if let Some(held) = self.held.take() {
            self.emit(
                held.timestamp,
                Block {
                    key: held.key,
                    data: held.data,
                    duration: None,
                    reference: None,
                },
            )?;
        }
        self.held = Some(Held {
            timestamp,
            key,
            data: data.to_vec(),
            previous: (self.frames > 0).then_some(self.last_timestamp),
        });
        self.last_timestamp = timestamp;
        self.frames += 1;
        Ok(())
    }

    /// Frames written so far.
    pub(crate) fn frames(&self) -> u64 {
        self.frames
    }

    /// Bytes written to the file plus bytes still buffered.
    pub(crate) fn bytes(&self) -> u64 {
        self.written
            + self.cluster.as_ref().map_or(0, |c| c.body.len() as u64)
            + self.held.as_ref().map_or(0, |h| h.data.len() as u64)
    }

    /// Finish the segment at `end` ms after its start: write the held frame
    /// with its duration, the Cues, patch sizes and positions, flush to disk
    /// and rename the file into place. Returns the file's size.
    pub(crate) fn finish(mut self, end: u64) -> io::Result<u64> {
        if let Some(held) = self.held.take() {
            let duration = end.saturating_sub(held.timestamp).max(1);
            let reference = if held.key {
                None
            } else {
                let previous = held.previous.unwrap_or(held.timestamp);
                Some(-i64::try_from(held.timestamp - previous).unwrap_or(0).max(1))
            };
            self.emit(
                held.timestamp,
                Block {
                    key: held.key,
                    data: held.data,
                    duration: Some(duration),
                    reference,
                },
            )?;
        }
        self.flush_cluster()?;
        if self.data_start == 0 {
            // No frame at all: nothing playable to keep.
            drop(self.file);
            fs::remove_file(&self.part)?;
            return Ok(0);
        }
        let cues_pos = self.written - self.data_start;
        let mut points = Vec::new();
        for (time, position) in &self.cues {
            let mut positions = uint(0xF7, 1); // CueTrack
            positions.extend(uint(0xF1, *position)); // CueClusterPosition
            let mut point = uint(0xB3, *time); // CueTime
            point.extend(element(0xB7, &positions)); // CueTrackPositions
            points.extend(element(0xBB, &point)); // CuePoint
        }
        self.put(&element(CUES, &points))?;
        let total = self.written;
        let duration = end.max(self.last_timestamp + 1) as f64;
        self.file.seek(SeekFrom::Start(self.size_at))?;
        self.file.write_all(&fixed_size(total - self.data_start))?;
        self.file.seek(SeekFrom::Start(self.duration_at))?;
        self.file.write_all(&duration.to_be_bytes())?;
        self.file.seek(SeekFrom::Start(self.cues_position_at))?;
        self.file.write_all(&cues_pos.to_be_bytes())?;
        self.file.sync_all()?;
        drop(self.file);
        fs::rename(&self.part, &self.done)?;
        Ok(total)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use matroska_demuxer::{Frame, MatroskaFile, TrackType};

    use super::*;
    use crate::recording::store::tests::temp_root;

    /// What a segment file holds, read back with an independent demuxer.
    #[derive(Debug)]
    pub(crate) struct Parsed {
        pub(crate) doc_type: String,
        pub(crate) codec: String,
        pub(crate) codec_private: Vec<u8>,
        pub(crate) width: u64,
        pub(crate) height: u64,
        pub(crate) duration_ms: f64,
        /// (timestamp, keyframe flag as the file says, bytes)
        pub(crate) frames: Vec<(u64, Option<bool>, Vec<u8>)>,
        pub(crate) has_cues: bool,
    }

    pub(crate) fn parse(path: &Path) -> Parsed {
        let bytes = fs::read(path).unwrap();
        let has_cues = bytes.windows(4).any(|w| w == CUES.to_be_bytes().as_slice());
        let mut mkv = MatroskaFile::open(File::open(path).unwrap()).unwrap();
        let track = &mkv.tracks()[0];
        assert_eq!(track.track_type(), TrackType::Video);
        let video = track.video().unwrap();
        let parsed = Parsed {
            doc_type: mkv.ebml_header().doc_type().to_owned(),
            codec: track.codec_id().to_owned(),
            codec_private: track.codec_private().unwrap_or_default().to_vec(),
            width: video.pixel_width().get(),
            height: video.pixel_height().get(),
            duration_ms: mkv.info().duration().unwrap_or(0.0),
            frames: Vec::new(),
            has_cues,
        };
        let mut frames = Vec::new();
        let mut frame = Frame::default();
        while mkv.next_frame(&mut frame).unwrap() {
            frames.push((frame.timestamp, frame.is_keyframe, frame.data.clone()));
        }
        Parsed { frames, ..parsed }
    }

    #[test]
    fn a_segment_parses_with_timestamps_duration_and_cues() {
        let root = temp_root("webm");
        fs::create_dir_all(&root).unwrap();
        let mut w = SegmentWriter::create(&root, 1, 320, 180, vec![0x81, 0, 0x0c, 0]).unwrap();
        // A fake key frame: temporal delimiter + a sequence header OBU.
        let key = [0x12, 0x00, 0x0a, 0x02, 0xaa, 0xbb, 0x01];
        w.write_frame(0, &key, true).unwrap();
        w.write_frame(250, b"second", false).unwrap();
        w.write_frame(40_000, b"third", false).unwrap(); // new cluster
        assert_eq!(w.frames(), 3);
        assert!(!root.join("000001.webm").exists());
        let bytes = w.finish(60_000).unwrap();
        assert!(!root.join("000001.webm.part").exists());
        let path = root.join("000001.webm");
        assert_eq!(fs::metadata(&path).unwrap().len(), bytes);
        let p = parse(&path);
        assert_eq!(p.doc_type, "webm");
        assert_eq!(p.codec, "V_AV1");
        assert_eq!(p.codec_private, [0x81, 0, 0x0c, 0, 0x0a, 0x02, 0xaa, 0xbb]);
        assert_eq!((p.width, p.height), (320, 180));
        assert!((p.duration_ms - 60_000.0).abs() < 1.0);
        let times: Vec<u64> = p.frames.iter().map(|f| f.0).collect();
        assert_eq!(times, [0, 250, 40_000]);
        assert_eq!(p.frames[0].1, Some(true));
        assert_eq!(p.frames[1].1, Some(false));
        assert_eq!(p.frames[2].2, b"third");
        assert!(p.has_cues);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_empty_segment_leaves_no_file() {
        let root = temp_root("webm-empty");
        fs::create_dir_all(&root).unwrap();
        let w = SegmentWriter::create(&root, 2, 8, 8, vec![0x81, 0, 0, 0]).unwrap();
        assert_eq!(w.finish(100).unwrap(), 0);
        assert!(fs::read_dir(&root).unwrap().next().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ebml_sizes_are_minimal_and_valid() {
        assert_eq!(size_bytes(0), [0x80]);
        assert_eq!(size_bytes(126), [0xFE]);
        assert_eq!(size_bytes(127), [0x40, 0x7F]);
        assert_eq!(size_bytes(16_382), [0x7F, 0xFE]);
        assert_eq!(fixed_size(5), [1, 0, 0, 0, 0, 0, 0, 5]);
        assert_eq!(id_bytes(0xA3), [0xA3]);
        assert_eq!(id_bytes(SEGMENT), [0x18, 0x53, 0x80, 0x67]);
        assert_eq!(uint(0xD7, 0), [0xD7, 0x81, 0x00]);
    }
}
