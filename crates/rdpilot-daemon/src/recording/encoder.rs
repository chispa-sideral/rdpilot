//! The video encoder seam and its AV1 implementation (`rav1e`).
//!
//! With `low_latency` and the smallest lookahead, rav1e returns the packet
//! for input frame N only after a few later frames were sent, or when the
//! stream is flushed; a flush ends the stream. The recorder therefore uses
//! one encoder per segment, sends frames, collects whatever packets are
//! ready (matched to their frames by `input_frameno`, which rises in input
//! order) and flushes at segment close.

use rav1e::prelude::{
    ChromaSampling, ColorDescription, ColorPrimaries, Config, Context, EncoderConfig,
    EncoderStatus, FrameParameters, FrameType, FrameTypeOverride, MatrixCoefficients, PixelRange,
    Rational, SceneDetectionSpeed, SpeedSettings, TransferCharacteristics,
};

use super::i420::I420;

/// rav1e speed preset (10 is the fastest).
pub(crate) const SPEED: u8 = 10;
/// Constant quantizer: screen text stays legible at this value.
pub(crate) const QUANTIZER: usize = 130;
/// Threads of the encoder's own pool.
pub(crate) const ENCODER_THREADS: usize = 2;
/// Tiles requested per frame (rav1e may use more to stay within the AV1
/// level limits at the millisecond time base).
pub(crate) const TILES: usize = 1;

/// One encoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Packet {
    /// The number of the frame it encodes, counted from 0 per encoder.
    pub(crate) input_frameno: u64,
    pub(crate) data: Vec<u8>,
    pub(crate) key: bool,
}

/// A video encoder for one segment.
pub(crate) trait VideoEncoder: Send {
    /// Queue one frame; `key` forces a key frame.
    ///
    /// # Errors
    ///
    /// A short, content-free reason.
    fn send(&mut self, frame: &I420, key: bool) -> Result<(), String>;
    /// Every packet that is ready now, in input order.
    ///
    /// # Errors
    ///
    /// A short, content-free reason.
    fn receive(&mut self) -> Result<Vec<Packet>, String>;
    /// End the stream and return every remaining packet.
    ///
    /// # Errors
    ///
    /// A short, content-free reason.
    fn flush(&mut self) -> Result<Vec<Packet>, String>;
    /// The fixed 4-byte header of the Matroska `CodecPrivate`
    /// (AV1CodecConfigurationRecord without its configuration OBUs).
    fn codec_config(&self) -> Vec<u8>;
}

/// Creates one encoder per segment.
pub(crate) trait EncoderFactory: Send + Sync {
    /// An encoder for frames of `width` x `height`.
    ///
    /// # Errors
    ///
    /// A short, content-free reason.
    fn create(&self, width: usize, height: usize) -> Result<Box<dyn VideoEncoder>, String>;
}

/// The AV1 encoder factory with the recording settings.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Rav1eFactory;

impl EncoderFactory for Rav1eFactory {
    fn create(&self, width: usize, height: usize) -> Result<Box<dyn VideoEncoder>, String> {
        let mut enc = EncoderConfig {
            width,
            height,
            bit_depth: 8,
            chroma_sampling: ChromaSampling::Cs420,
            low_latency: true,
            quantizer: QUANTIZER,
            min_quantizer: 0,
            bitrate: 0,
            speed_settings: SpeedSettings::from_preset(SPEED),
            // Key frames only where the recorder asks (each segment start).
            max_key_frame_interval: 100_000,
            min_key_frame_interval: 0,
            time_base: Rational::new(1, 1000),
            pixel_range: PixelRange::Limited,
            tiles: TILES,
            color_description: Some(ColorDescription {
                color_primaries: ColorPrimaries::BT709,
                transfer_characteristics: TransferCharacteristics::BT709,
                matrix_coefficients: MatrixCoefficients::BT709,
            }),
            ..EncoderConfig::default()
        };
        // 0 is rejected; 1 is the shortest lookahead.
        enc.speed_settings.rdo_lookahead_frames = 1;
        enc.speed_settings.scene_detection_mode = SceneDetectionSpeed::None;
        let context: Context<u8> = Config::new()
            .with_encoder_config(enc)
            .with_threads(ENCODER_THREADS)
            .new_context()
            .map_err(|e| format!("encoder configuration rejected: {e:?}"))?;
        Ok(Box::new(Rav1e { context }))
    }
}

struct Rav1e {
    context: Context<u8>,
}

impl Rav1e {
    fn drain(&mut self, flushing: bool) -> Result<Vec<Packet>, String> {
        let mut out = Vec::new();
        loop {
            match self.context.receive_packet() {
                Ok(packet) => out.push(Packet {
                    input_frameno: packet.input_frameno,
                    key: packet.frame_type == FrameType::KEY,
                    data: packet.data,
                }),
                Err(EncoderStatus::Encoded) => continue,
                Err(EncoderStatus::NeedMoreData) if !flushing => return Ok(out),
                Err(EncoderStatus::LimitReached) if flushing => return Ok(out),
                Err(e) => return Err(format!("encoder failed: {e:?}")),
            }
        }
    }
}

impl VideoEncoder for Rav1e {
    fn send(&mut self, frame: &I420, key: bool) -> Result<(), String> {
        let mut f = self.context.new_frame();
        let cw = frame.chroma_width();
        f.planes[0].copy_from_raw_u8(&frame.y, frame.width, 1);
        f.planes[1].copy_from_raw_u8(&frame.u, cw, 1);
        f.planes[2].copy_from_raw_u8(&frame.v, cw, 1);
        let params = FrameParameters {
            frame_type_override: if key {
                FrameTypeOverride::Key
            } else {
                FrameTypeOverride::No
            },
            ..FrameParameters::default()
        };
        self.context
            .send_frame((f, params))
            .map_err(|e| format!("encoder refused a frame: {e:?}"))
    }

    fn receive(&mut self) -> Result<Vec<Packet>, String> {
        self.drain(false)
    }

    fn flush(&mut self) -> Result<Vec<Packet>, String> {
        self.context.flush();
        self.drain(true)
    }

    fn codec_config(&self) -> Vec<u8> {
        self.context.container_sequence_header()
    }
}

/// The first sequence header OBU (header, size field and payload) in an
/// AV1 temporal unit, if any.
pub(crate) fn sequence_header_obu(data: &[u8]) -> Option<&[u8]> {
    let mut pos = 0;
    while pos < data.len() {
        let header = data[pos];
        let obu_type = (header >> 3) & 0x0f;
        let extension = usize::from((header >> 2) & 1);
        let has_size = (header >> 1) & 1 == 1;
        if !has_size {
            return None;
        }
        let mut cursor = pos + 1 + extension;
        let mut size: usize = 0;
        let mut shift = 0;
        loop {
            let byte = *data.get(cursor)?;
            cursor += 1;
            size |= usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 56 {
                return None;
            }
        }
        let end = cursor.checked_add(size)?;
        if end > data.len() {
            return None;
        }
        if obu_type == 1 {
            return Some(&data[pos..end]);
        }
        pos = end;
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;
    use crate::recording::i420::from_rgba;

    /// A scripted encoder: returns packets `lag` frames late, optionally
    /// sleeps per send, fails from a given send on, or reorders packets.
    #[derive(Clone, Default)]
    pub(crate) struct FakeFactory {
        pub(crate) lag: usize,
        pub(crate) delay: Duration,
        pub(crate) fail_at_send: Option<usize>,
        pub(crate) out_of_order: bool,
        pub(crate) packet_bytes: usize,
        /// Total sends across all encoders.
        pub(crate) sends: Arc<AtomicUsize>,
        /// Encoders created.
        pub(crate) created: Arc<AtomicUsize>,
        /// Sizes encoders were created for.
        pub(crate) sizes: Arc<Mutex<Vec<(usize, usize)>>>,
    }

    struct Fake {
        cfg: FakeFactory,
        queued: VecDeque<(u64, bool)>,
        next: u64,
    }

    impl EncoderFactory for FakeFactory {
        fn create(&self, width: usize, height: usize) -> Result<Box<dyn VideoEncoder>, String> {
            self.created.fetch_add(1, Ordering::SeqCst);
            self.sizes.lock().unwrap().push((width, height));
            Ok(Box::new(Fake {
                cfg: self.clone(),
                queued: VecDeque::new(),
                next: 0,
            }))
        }
    }

    impl Fake {
        fn packet(&self, no: u64, key: bool) -> Packet {
            let mut data = no.to_be_bytes().to_vec();
            data.resize(self.cfg.packet_bytes.max(8), 0xab);
            Packet {
                input_frameno: no,
                data,
                key,
            }
        }
    }

    impl VideoEncoder for Fake {
        fn send(&mut self, _frame: &I420, key: bool) -> Result<(), String> {
            let n = self.cfg.sends.fetch_add(1, Ordering::SeqCst);
            if self.cfg.fail_at_send.is_some_and(|f| n >= f) {
                return Err("fake encoder failure".into());
            }
            if !self.cfg.delay.is_zero() {
                std::thread::sleep(self.cfg.delay);
            }
            self.queued.push_back((self.next, key));
            self.next += 1;
            Ok(())
        }

        fn receive(&mut self) -> Result<Vec<Packet>, String> {
            let mut out = Vec::new();
            while self.queued.len() > self.cfg.lag {
                let (no, key) = self.queued.pop_front().unwrap_or_default();
                out.push(self.packet(no, key));
            }
            if self.cfg.out_of_order && out.len() >= 2 {
                out.swap(0, 1);
            }
            Ok(out)
        }

        fn flush(&mut self) -> Result<Vec<Packet>, String> {
            let rest: Vec<(u64, bool)> = self.queued.drain(..).collect();
            let mut out: Vec<Packet> = rest
                .into_iter()
                .map(|(no, key)| self.packet(no, key))
                .collect();
            if self.cfg.out_of_order && out.len() >= 2 {
                out.swap(0, 1);
            }
            Ok(out)
        }

        fn codec_config(&self) -> Vec<u8> {
            vec![0x81, 0x00, 0x0c, 0x00]
        }
    }

    pub(crate) fn frame(w: usize, h: usize, seed: u8) -> I420 {
        let rgba: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                let v = u8::try_from((i * 7 + usize::from(seed) * 31) % 251).unwrap_or(0);
                [v, v.wrapping_add(seed), 255 - v, 255]
            })
            .collect();
        from_rgba(&rgba, w, h)
    }

    /// The real encoder returns packets late, in input order, and a flush
    /// returns every frame still pending.
    #[test]
    fn rav1e_returns_every_frame_in_input_order_after_a_flush() {
        for count in [1_u64, 3, 10] {
            let mut enc = Rav1eFactory.create(64, 48).unwrap();
            let mut packets = Vec::new();
            for i in 0..count {
                enc.send(&frame(64, 48, u8::try_from(i).unwrap()), i == 0)
                    .unwrap();
                packets.extend(enc.receive().unwrap());
            }
            assert!(
                (packets.len() as u64) < count || count > 4,
                "packets come late"
            );
            packets.extend(enc.flush().unwrap());
            let numbers: Vec<u64> = packets.iter().map(|p| p.input_frameno).collect();
            assert_eq!(numbers, (0..count).collect::<Vec<_>>());
            assert!(packets[0].key);
            let header = sequence_header_obu(&packets[0].data).expect("key frame carries it");
            assert_eq!((header[0] >> 3) & 0x0f, 1);
            assert_eq!(enc.codec_config().len(), 4);
        }
    }

    #[test]
    fn obu_scanner_finds_the_sequence_header_and_rejects_garbage() {
        // Temporal delimiter (type 2, size 0), then a type 1 OBU of 2 bytes.
        let tu = [0x12, 0x00, 0x0a, 0x02, 0xaa, 0xbb, 0x32, 0x00];
        assert_eq!(sequence_header_obu(&tu), Some(&tu[2..6]));
        assert_eq!(sequence_header_obu(&[0x12, 0x00]), None);
        assert_eq!(sequence_header_obu(&[0x0a, 0x05, 0x00]), None);
        assert_eq!(sequence_header_obu(&[0x08]), None);
    }
}
