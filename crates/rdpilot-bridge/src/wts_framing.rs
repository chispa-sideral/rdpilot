//! Validates documented CHANNEL_PDU_HEADER fragmentation on Windows DVC reads.
//! File handle reads preserve each channel chunk; application framing is separate.
use rdpilot_bridge_protocol::MAX_FRAME_BYTES;
use std::io;

#[derive(Default)]
pub struct PduDecoder {
    remaining: Option<usize>,
    total: usize,
}
impl PduDecoder {
    pub fn push<'a>(&mut self, chunk: &'a [u8]) -> io::Result<&'a [u8]> {
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "invalid WTS CHANNEL_PDU_HEADER");
        if chunk.len() < 8 {
            return Err(bad());
        }
        let length = u32::from_le_bytes(chunk[..4].try_into().unwrap()) as usize;
        let flags = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
        let data = &chunk[8..];
        // SHOW_PROTOCOL may be carried in flags alongside FIRST/LAST.
        if flags & !(0x10 | 3) != 0 || length == 0 || length > MAX_FRAME_BYTES + 4 {
            return Err(bad());
        }
        if flags & 1 != 0 {
            if self.remaining.is_some() {
                return Err(bad());
            }
            self.remaining = Some(length);
            self.total = length;
        }
        let remaining = self.remaining.ok_or_else(bad)?;
        if length != self.total || data.len() > remaining {
            return Err(bad());
        }
        let remaining = remaining - data.len();
        if flags & 2 != 0 {
            if remaining != 0 {
                return Err(bad());
            }
            self.remaining = None;
        } else {
            if remaining == 0 {
                return Err(bad());
            }
            self.remaining = Some(remaining);
        }
        Ok(data)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn chunk(length: u32, flags: u32, data: &[u8]) -> Vec<u8> {
        let mut b = length.to_le_bytes().to_vec();
        b.extend(flags.to_le_bytes());
        b.extend(data);
        b
    }
    #[test]
    fn ordered_fragments_and_bad_lengths() {
        let mut d = PduDecoder::default();
        assert_eq!(d.push(&chunk(6, 1, b"abc")).unwrap(), b"abc");
        assert_eq!(d.push(&chunk(6, 2, b"def")).unwrap(), b"def");
        assert!(d.push(&chunk(6, 2, b"def")).is_err());
        assert!(PduDecoder::default().push(&chunk(4, 3, b"abc")).is_err());
    }
}
