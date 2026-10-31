//! H.264 Annex-B byte-stream helpers (ITU-T H.264 Annex B), pure Rust.
//!
//! Media Foundation H.264 encoders emit Annex-B: each NAL unit is preceded by a
//! `00 00 01` or `00 00 00 01` start code. These helpers split such buffers into NAL units,
//! classify them, and make sure keyframes carry SPS/PPS so a decoder can join at any IDR.

/// NAL unit types we care about (H.264 Table 7-1).
pub mod nal_type {
    /// Coded slice of a non-IDR picture.
    pub const SLICE: u8 = 1;
    /// Coded slice of an IDR picture (keyframe).
    pub const IDR: u8 = 5;
    /// Supplemental enhancement information.
    pub const SEI: u8 = 6;
    /// Sequence parameter set.
    pub const SPS: u8 = 7;
    /// Picture parameter set.
    pub const PPS: u8 = 8;
    /// Access unit delimiter.
    pub const AUD: u8 = 9;
}

/// One NAL unit inside an Annex-B buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NalUnit<'a> {
    /// Offset of the start code within the buffer.
    pub start: usize,
    /// NAL bytes after the start code (header byte first), trailing zero bytes removed.
    pub data: &'a [u8],
}

impl NalUnit<'_> {
    /// `nal_unit_type` (low 5 bits of the header byte), or 0 for an empty unit.
    pub fn nal_type(&self) -> u8 {
        self.data.first().map_or(0, |b| b & 0x1f)
    }

    /// For slice NALs (types 1 and 5): true when `first_mb_in_slice == 0`, i.e. this slice
    /// starts a new picture. `first_mb_in_slice` is the first ue(v) field of the slice header;
    /// it is 0 exactly when the first payload bit is 1.
    pub fn is_first_slice_of_picture(&self) -> bool {
        matches!(self.nal_type(), nal_type::SLICE | nal_type::IDR)
            && self.data.get(1).is_some_and(|b| b & 0x80 != 0)
    }

    /// For slice NALs: `slice_type % 5` (see [`slice_type`]), parsed from the slice header.
    pub fn slice_type(&self) -> Option<u8> {
        if !matches!(self.nal_type(), nal_type::SLICE | nal_type::IDR) {
            return None;
        }
        let rbsp = rbsp_prefix(self.data.get(1..)?, 16);
        let mut r = BitReader { data: &rbsp, pos: 0 };
        let _first_mb = r.ue()?;
        Some((r.ue()? % 5) as u8)
    }
}

/// `slice_type` values modulo 5 (H.264 Table 7-6).
pub mod slice_type {
    pub const P: u8 = 0;
    pub const B: u8 = 1;
    pub const I: u8 = 2;
    pub const SP: u8 = 3;
    pub const SI: u8 = 4;
}

/// Reads Exp-Golomb codes from RBSP bytes (emulation-prevention bytes already removed).
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl BitReader<'_> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos / 8)?;
        let b = (byte >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        Some(b as u32)
    }

    /// ue(v)
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        let mut v = 0u32;
        for _ in 0..zeros {
            v = (v << 1) | self.bit()?;
        }
        Some((1u32 << zeros) - 1 + v)
    }
}

/// Remove `emulation_prevention_three_byte`s (`00 00 03` → `00 00`) from at most `max` bytes.
fn rbsp_prefix(nal_payload: &[u8], max: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(max.min(nal_payload.len()));
    let mut zeros = 0;
    for &b in nal_payload.iter().take(max) {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// Find the next `00 00 01` at or after `from`. Returns its position.
fn find_three_byte_start(buf: &[u8], from: usize) -> Option<usize> {
    if buf.len() < 3 {
        return None;
    }
    let mut i = from;
    while i + 2 < buf.len() {
        // Fast skip: a start code needs buf[i+2] == 1 and buf[i+1] == 0.
        if buf[i + 2] > 1 {
            i += 3;
            continue;
        }
        if buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 1 {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Iterator over the NAL units of an Annex-B buffer. Bytes before the first start code are
/// ignored.
pub struct NalIter<'a> {
    buf: &'a [u8],
    /// Position of the next `00 00 01`, if any.
    next: Option<usize>,
}

impl<'a> Iterator for NalIter<'a> {
    type Item = NalUnit<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let sc = self.next?;
        let payload_start = sc + 3;
        // A 4-byte start code is a zero byte followed by a 3-byte one.
        let start = if sc > 0 && self.buf[sc - 1] == 0 { sc - 1 } else { sc };
        let following = find_three_byte_start(self.buf, payload_start);
        self.next = following;
        let mut end = following.unwrap_or(self.buf.len());
        // NAL units never end in 0x00 (rbsp_trailing_bits), so trailing zeros belong to the
        // next start code (or are trailing_zero_8bits padding).
        while end > payload_start && self.buf[end - 1] == 0 {
            end -= 1;
        }
        Some(NalUnit { start, data: &self.buf[payload_start..end] })
    }
}

/// Split an Annex-B buffer into NAL units.
pub fn nal_units(buf: &[u8]) -> NalIter<'_> {
    NalIter { buf, next: find_three_byte_start(buf, 0) }
}

/// True if any NAL unit in `buf` has type `t`.
pub fn contains_nal_type(buf: &[u8], t: u8) -> bool {
    nal_units(buf).any(|n| n.nal_type() == t)
}

/// True if the access unit contains an IDR slice.
pub fn is_keyframe(buf: &[u8]) -> bool {
    contains_nal_type(buf, nal_type::IDR)
}

/// Counts of the interesting NAL types in a whole elementary stream.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StreamSummary {
    pub nal_units: usize,
    pub sps: usize,
    pub pps: usize,
    pub idr_slices: usize,
    pub non_idr_slices: usize,
    /// Pictures = slices with `first_mb_in_slice == 0`.
    pub pictures: usize,
    pub idr_pictures: usize,
    pub sei: usize,
    pub aud: usize,
    /// Slices by type (P / B / I, SP and SI counted as P and I).
    pub p_slices: usize,
    pub b_slices: usize,
    pub i_slices: usize,
    /// Type of the very first NAL unit (should be SPS or AUD for a joinable stream).
    pub first_nal_type: Option<u8>,
}

/// Summarise an Annex-B elementary stream (e.g. a `.h264` file).
pub fn summarize(buf: &[u8]) -> StreamSummary {
    let mut s = StreamSummary::default();
    for n in nal_units(buf) {
        s.nal_units += 1;
        if s.first_nal_type.is_none() {
            s.first_nal_type = Some(n.nal_type());
        }
        match n.nal_type() {
            nal_type::SPS => s.sps += 1,
            nal_type::PPS => s.pps += 1,
            nal_type::SEI => s.sei += 1,
            nal_type::AUD => s.aud += 1,
            nal_type::IDR => {
                s.idr_slices += 1;
                if n.is_first_slice_of_picture() {
                    s.idr_pictures += 1;
                }
            }
            nal_type::SLICE => s.non_idr_slices += 1,
            _ => {}
        }
        if n.is_first_slice_of_picture() {
            s.pictures += 1;
        }
        match n.slice_type() {
            Some(slice_type::P | slice_type::SP) => s.p_slices += 1,
            Some(slice_type::B) => s.b_slices += 1,
            Some(slice_type::I | slice_type::SI) => s.i_slices += 1,
            _ => {}
        }
    }
    s
}

/// Parse `profile_idc`, `level_idc` from an SPS NAL (header byte first).
pub fn sps_profile_level(sps: &[u8]) -> Option<(u8, u8)> {
    if sps.first().map(|b| b & 0x1f) != Some(nal_type::SPS) || sps.len() < 4 {
        return None;
    }
    Some((sps[1], sps[3]))
}

/// Remembers the latest SPS/PPS seen in the stream (or supplied by the encoder's media type)
/// and inserts them in front of keyframes that lack them, so every IDR is self-contained.
#[derive(Debug, Default, Clone)]
pub struct ParameterSets {
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl ParameterSets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record every SPS / PPS found in `buf` (an access unit or an `MF_MT_MPEG_SEQUENCE_HEADER`).
    pub fn learn(&mut self, buf: &[u8]) {
        for n in nal_units(buf) {
            match n.nal_type() {
                nal_type::SPS => self.sps = Some(n.data.to_vec()),
                nal_type::PPS => self.pps = Some(n.data.to_vec()),
                _ => {}
            }
        }
    }

    pub fn has_both(&self) -> bool {
        self.sps.is_some() && self.pps.is_some()
    }

    pub fn sps(&self) -> Option<&[u8]> {
        self.sps.as_deref()
    }

    /// Learn from `au`, then return it unchanged unless it is a keyframe missing SPS or PPS, in
    /// which case a copy with the cached parameter sets prepended is returned.
    pub fn fix_access_unit(&mut self, au: Vec<u8>) -> Vec<u8> {
        let mut has_sps = false;
        let mut has_pps = false;
        let mut has_idr = false;
        for n in nal_units(&au) {
            match n.nal_type() {
                nal_type::SPS => has_sps = true,
                nal_type::PPS => has_pps = true,
                nal_type::IDR => has_idr = true,
                _ => {}
            }
        }
        if has_sps || has_pps {
            self.learn(&au);
        }
        if !has_idr || (has_sps && has_pps) {
            return au;
        }
        let mut out = Vec::with_capacity(au.len() + 64);
        if !has_sps {
            if let Some(sps) = &self.sps {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(sps);
            }
        }
        if !has_pps {
            if let Some(pps) = &self.pps {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(pps);
            }
        }
        out.extend_from_slice(&au);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPS (Main profile 77, level 4.0), PPS, IDR slice with first_mb == 0, P slice.
    const SPS: [u8; 5] = [0x67, 77, 0x40, 40, 0xAC];
    const PPS: [u8; 3] = [0x68, 0xEE, 0x3C];
    const IDR: [u8; 4] = [0x65, 0x88, 0x84, 0x21];
    const P: [u8; 3] = [0x41, 0x9A, 0x02];

    fn au(parts: &[(&[u8], bool)]) -> Vec<u8> {
        let mut v = Vec::new();
        for (nal, four) in parts {
            if *four {
                v.extend_from_slice(&[0, 0, 0, 1]);
            } else {
                v.extend_from_slice(&[0, 0, 1]);
            }
            v.extend_from_slice(nal);
        }
        v
    }

    #[test]
    fn splits_three_and_four_byte_start_codes() {
        let buf = au(&[(&SPS, true), (&PPS, false), (&IDR, true)]);
        let nals: Vec<_> = nal_units(&buf).collect();
        assert_eq!(nals.len(), 3);
        assert_eq!(nals[0].data, &SPS);
        assert_eq!(nals[0].start, 0);
        assert_eq!(nals[1].data, &PPS);
        assert_eq!(nals[2].data, &IDR);
        assert_eq!(nals.iter().map(|n| n.nal_type()).collect::<Vec<_>>(), vec![7, 8, 5]);
    }

    #[test]
    fn trailing_zeros_are_not_part_of_the_nal() {
        let mut buf = au(&[(&P, true)]);
        buf.extend_from_slice(&[0, 0]);
        let n = nal_units(&buf).next().unwrap();
        assert_eq!(n.data, &P);
    }

    #[test]
    fn leading_garbage_and_no_start_code() {
        assert_eq!(nal_units(&[1, 2, 3, 4]).count(), 0);
        assert_eq!(nal_units(&[]).count(), 0);
        let mut buf = vec![0xFF, 0xFE];
        buf.extend(au(&[(&P, false)]));
        assert_eq!(nal_units(&buf).count(), 1);
    }

    #[test]
    fn emulation_prevention_bytes_do_not_split() {
        // 00 00 03 01 inside a NAL must not be taken as a start code.
        let nal = [0x41, 0x00, 0x00, 0x03, 0x01, 0x7F];
        let buf = au(&[(&nal, true)]);
        let nals: Vec<_> = nal_units(&buf).collect();
        assert_eq!(nals.len(), 1);
        assert_eq!(nals[0].data, &nal);
    }

    #[test]
    fn keyframe_detection() {
        assert!(is_keyframe(&au(&[(&SPS, true), (&PPS, true), (&IDR, true)])));
        assert!(!is_keyframe(&au(&[(&P, true)])));
        assert!(contains_nal_type(&au(&[(&SPS, true)]), nal_type::SPS));
    }

    #[test]
    fn first_slice_detection() {
        let first = NalUnit { start: 0, data: &[0x41, 0x80] };
        let later = NalUnit { start: 0, data: &[0x41, 0x40] };
        let sps = NalUnit { start: 0, data: &SPS };
        assert!(first.is_first_slice_of_picture());
        assert!(!later.is_first_slice_of_picture());
        assert!(!sps.is_first_slice_of_picture());
    }

    #[test]
    fn summary_counts_pictures() {
        let mut stream = au(&[(&SPS, true), (&PPS, true), (&IDR, true)]);
        for _ in 0..4 {
            stream.extend(au(&[(&P, true)]));
        }
        let s = summarize(&stream);
        assert_eq!(s.sps, 1);
        assert_eq!(s.pps, 1);
        assert_eq!(s.idr_pictures, 1);
        assert_eq!(s.non_idr_slices, 4);
        assert_eq!(s.pictures, 5);
        assert_eq!(s.first_nal_type, Some(nal_type::SPS));
    }

    #[test]
    fn slice_types_from_header() {
        // first_mb_in_slice = 0 ("1"), slice_type ue: 7 (I, all-I) = "0001000", 5 (P) = "00110",
        // 6 (B) = "00111", 2 (I) = "011".
        let i_all = NalUnit { start: 0, data: &[0x65, 0b1000_1000, 0x80] };
        let p = NalUnit { start: 0, data: &[0x41, 0b1001_1010] };
        let b = NalUnit { start: 0, data: &[0x01, 0b1001_1110] };
        let i = NalUnit { start: 0, data: &[0x41, 0b1011_1000] };
        assert_eq!(i_all.slice_type(), Some(slice_type::I));
        assert_eq!(p.slice_type(), Some(slice_type::P));
        assert_eq!(b.slice_type(), Some(slice_type::B));
        assert_eq!(i.slice_type(), Some(slice_type::I));
        assert_eq!(NalUnit { start: 0, data: &SPS }.slice_type(), None);
    }

    #[test]
    fn rbsp_strips_emulation_prevention() {
        assert_eq!(rbsp_prefix(&[0, 0, 3, 1, 0, 0, 3, 0], 16), vec![0, 0, 1, 0, 0, 0]);
        assert_eq!(rbsp_prefix(&[1, 2, 3], 2), vec![1, 2]);
    }

    #[test]
    fn profile_level_from_sps() {
        assert_eq!(sps_profile_level(&SPS), Some((77, 40)));
        assert_eq!(sps_profile_level(&PPS), None);
    }

    #[test]
    fn parameter_sets_are_prepended_to_bare_keyframes() {
        let mut ps = ParameterSets::new();
        let first = au(&[(&SPS, true), (&PPS, true), (&IDR, true)]);
        assert_eq!(ps.fix_access_unit(first.clone()), first);
        assert!(ps.has_both());

        let p = au(&[(&P, true)]);
        assert_eq!(ps.fix_access_unit(p.clone()), p, "non-key frames untouched");

        let bare_idr = au(&[(&IDR, true)]);
        let fixed = ps.fix_access_unit(bare_idr);
        let types: Vec<_> = nal_units(&fixed).map(|n| n.nal_type()).collect();
        assert_eq!(types, vec![7, 8, 5]);
    }

    #[test]
    fn learns_from_sequence_header() {
        let mut ps = ParameterSets::new();
        ps.learn(&au(&[(&SPS, true), (&PPS, true)]));
        assert_eq!(ps.sps(), Some(&SPS[..]));
        let fixed = ps.fix_access_unit(au(&[(&IDR, false)]));
        assert!(contains_nal_type(&fixed, nal_type::PPS));
    }
}
