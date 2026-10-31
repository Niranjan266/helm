//! Minimal Ogg/Opus file writer (RFC 3533 pages, RFC 7845 Opus mapping), pure Rust.
//!
//! One logical stream, one packet per page for audio (simple and fine for recordings of a few
//! minutes; ~27 bytes of overhead per 20 ms packet). Plays in VLC, ffmpeg, browsers.

use std::io::{self, Write};

/// CRC-32 used by Ogg: polynomial 0x04C11DB7, MSB first, initial value 0, no final XOR.
pub fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut r = (i as u32) << 24;
            for _ in 0..8 {
                r = if r & 0x8000_0000 != 0 {
                    (r << 1) ^ 0x04C1_1DB7
                } else {
                    r << 1
                };
            }
            *e = r;
        }
        t
    });
    let mut crc = 0u32;
    for &b in data {
        crc = (crc << 8) ^ table[((crc >> 24) as u8 ^ b) as usize];
    }
    crc
}

/// Build one Ogg page holding exactly `packet` (which must be < 255 * 255 bytes).
pub fn page(serial: u32, sequence: u32, granule: u64, header_type: u8, packet: &[u8]) -> Vec<u8> {
    let mut lacing = Vec::with_capacity(packet.len() / 255 + 1);
    let mut left = packet.len();
    loop {
        if left >= 255 {
            lacing.push(255u8);
            left -= 255;
        } else {
            lacing.push(left as u8);
            break;
        }
    }
    assert!(lacing.len() <= 255, "packet too large for one Ogg page");
    let mut p = Vec::with_capacity(27 + lacing.len() + packet.len());
    p.extend_from_slice(b"OggS");
    p.push(0); // version
    p.push(header_type);
    p.extend_from_slice(&granule.to_le_bytes());
    p.extend_from_slice(&serial.to_le_bytes());
    p.extend_from_slice(&sequence.to_le_bytes());
    p.extend_from_slice(&[0, 0, 0, 0]); // CRC placeholder
    p.push(lacing.len() as u8);
    p.extend_from_slice(&lacing);
    p.extend_from_slice(packet);
    let crc = crc32(&p);
    p[22..26].copy_from_slice(&crc.to_le_bytes());
    p
}

/// `OpusHead` identification header.
pub fn opus_head(channels: u8, pre_skip: u16, input_rate: u32) -> Vec<u8> {
    let mut h = Vec::with_capacity(19);
    h.extend_from_slice(b"OpusHead");
    h.push(1); // version
    h.push(channels);
    h.extend_from_slice(&pre_skip.to_le_bytes());
    h.extend_from_slice(&input_rate.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes()); // output gain
    h.push(0); // mapping family 0 (mono / stereo)
    h
}

/// `OpusTags` comment header.
pub fn opus_tags(vendor: &str) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend_from_slice(b"OpusTags");
    t.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    t.extend_from_slice(vendor.as_bytes());
    t.extend_from_slice(&0u32.to_le_bytes()); // no user comments
    t
}

/// Streams Opus packets into an Ogg/Opus file.
pub struct OggOpusWriter<W: Write> {
    out: W,
    serial: u32,
    sequence: u32,
    granule: u64,
    pre_skip: u16,
    /// Granule (48 kHz samples) per packet.
    packet_samples_48k: u64,
    pending: Option<Vec<u8>>,
}

impl<W: Write> OggOpusWriter<W> {
    /// Write the two header pages. `frame_samples` is per channel at `sample_rate`.
    pub fn new(
        mut out: W,
        serial: u32,
        channels: u8,
        sample_rate: u32,
        frame_samples: usize,
        pre_skip_48k: u16,
    ) -> io::Result<Self> {
        out.write_all(&page(
            serial,
            0,
            0,
            0x02,
            &opus_head(channels, pre_skip_48k, sample_rate),
        ))?;
        out.write_all(&page(serial, 1, 0, 0x00, &opus_tags("helm-win-audio")))?;
        Ok(OggOpusWriter {
            out,
            serial,
            sequence: 2,
            granule: 0,
            pre_skip: pre_skip_48k,
            packet_samples_48k: frame_samples as u64 * 48_000 / sample_rate as u64,
            pending: None,
        })
    }

    fn flush_pending(&mut self, last: bool) -> io::Result<()> {
        if let Some(p) = self.pending.take() {
            self.granule += self.packet_samples_48k;
            let mut g = self.granule;
            if last {
                g = g.max(self.pre_skip as u64);
            }
            let page = page(self.serial, self.sequence, g, if last { 0x04 } else { 0 }, &p);
            self.sequence += 1;
            self.out.write_all(&page)?;
        }
        Ok(())
    }

    /// Append one packet. (Pages are written one packet behind so the last one can carry the
    /// end-of-stream flag.)
    pub fn write_packet(&mut self, packet: &[u8]) -> io::Result<()> {
        self.flush_pending(false)?;
        self.pending = Some(packet.to_vec());
        Ok(())
    }

    /// Write the final page (EOS) and return the writer.
    pub fn finish(mut self) -> io::Result<W> {
        self.flush_pending(true)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-by-bit reference CRC (independent of the table version).
    fn crc_slow(data: &[u8]) -> u32 {
        let mut crc = 0u32;
        for &b in data {
            crc ^= (b as u32) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04C1_1DB7
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    #[test]
    fn crc_matches_reference() {
        for data in [&b""[..], b"OggS", b"123456789", &[0xFFu8; 300][..]] {
            assert_eq!(crc32(data), crc_slow(data));
        }
        assert_eq!(crc32(b""), 0);
        // CRC-32 with this polynomial, init 0 and no reflection/xor: check value 0x89A1897F.
        assert_eq!(crc32(b"123456789"), 0x89A1_897F);
    }

    #[test]
    fn page_layout_and_crc() {
        let p = page(7, 3, 960, 0, &[1u8; 600]);
        assert_eq!(&p[..4], b"OggS");
        assert_eq!(p[26], 3); // 255 + 255 + 90
        assert_eq!(&p[27..30], &[255, 255, 90]);
        assert_eq!(p.len(), 27 + 3 + 600);
        let stored = u32::from_le_bytes([p[22], p[23], p[24], p[25]]);
        let mut z = p.clone();
        z[22..26].fill(0);
        assert_eq!(crc32(&z), stored);
        // A 255-byte packet needs a terminating 0 lacing value.
        let p = page(7, 3, 0, 0, &[0u8; 255]);
        assert_eq!(&p[26..29], &[2, 255, 0]);
    }

    #[test]
    fn writer_granules_and_eos() {
        let w = OggOpusWriter::new(Vec::new(), 1, 2, 48_000, 960, 120).unwrap();
        let mut w = w;
        for _ in 0..3 {
            w.write_packet(&[0xFC, 0xFF, 0xFE]).unwrap();
        }
        let bytes = w.finish().unwrap();
        // Walk the pages.
        let mut off = 0;
        let mut pages = Vec::new();
        while off < bytes.len() {
            assert_eq!(&bytes[off..off + 4], b"OggS");
            let segs = bytes[off + 26] as usize;
            let body: usize = bytes[off + 27..off + 27 + segs].iter().map(|&b| b as usize).sum();
            let granule = u64::from_le_bytes(bytes[off + 6..off + 14].try_into().unwrap());
            let start = off + 27 + segs;
            pages.push((bytes[off + 5], granule, bytes[start..start + body.min(8)].to_vec()));
            off += 27 + segs + body;
        }
        assert_eq!(pages.len(), 5);
        assert_eq!(pages[0].0, 0x02);
        assert_eq!(&pages[0].2, b"OpusHead");
        assert_eq!(&pages[1].2, b"OpusTags");
        assert_eq!(
            pages.iter().map(|p| p.1).collect::<Vec<_>>(),
            vec![0, 0, 960, 1920, 2880]
        );
        assert_eq!(pages[4].0, 0x04);
    }
}
