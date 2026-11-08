//! FMT-9b: the fp16 vector column.
//!
//! A v9 record doesn't carry its embedding. The vector lives in a separate
//! column of fixed-size entries, `dims` IEEE half-precision values plus the
//! `f32` norm of the rounded vector (for cosine and L2), packed into
//! `vector` pages behind the standard 16-byte page header. At 384 dims an
//! entry is 772 bytes and a page holds 10.
//!
//! The FMT-9a measurement picked fp16 over int8 (PLANNER-A, 2026-11-07):
//! recall@10 against float rerank moves by 0.0000 at 10K and −0.0005 at
//! 100K, inside the 0.005 gate, where int8 lost 0.0115 and 0.0165.
//! `get()` and `recall()` on a v9 file return the fp16-rounded vector (an
//! error around 1e-3 relative) unless the file was created with
//! `store_float_vectors`.

use std::sync::OnceLock;

use crate::error::{MnemoError, Result};
use crate::format::PAYLOAD;

/// Bytes of the standard v9 page header (kind, scope, codec, reserved,
/// n_slots u16, live_bytes u16, generation u64).
pub(crate) const PAGE_HEADER: usize = 16;
/// `kind` byte of a vector page.
pub(crate) const PAGE_KIND_VECTOR: u8 = 2;

/// Bytes per column entry.
pub(crate) fn entry_len(dims: usize) -> usize {
    dims * 2 + 4
}

/// Entries per vector page (0 when one entry doesn't fit a page).
pub(crate) fn per_page(dims: usize) -> usize {
    (PAYLOAD - PAGE_HEADER) / entry_len(dims)
}

/// Round an `f32` to the nearest IEEE half (ties to even). Overflow goes to
/// infinity, NaN stays NaN.
pub(crate) fn f32_to_f16(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32;
    let man = b & 0x007f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x0200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x0080_0000;
        let shift = (14 - e) as u32;
        let half = m >> shift;
        let rem = m & ((1u32 << shift) - 1);
        let halfway = 1u32 << (shift - 1);
        let mut h = half;
        if rem > halfway || (rem == halfway && (half & 1) == 1) {
            h += 1;
        }
        return sign | h as u16;
    }
    let mut h = ((e as u32) << 10) | (man >> 13);
    let rem = man & 0x1fff;
    // A carry out of the mantissa correctly bumps the exponent (or reaches
    // infinity).
    if rem > 0x1000 || (rem == 0x1000 && (h & 1) == 1) {
        h += 1;
    }
    sign | h as u16
}

/// Widen an IEEE half to `f32` (exact).
pub(crate) fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h & 0x8000) as u32) << 16;
    let exp = ((h >> 10) & 0x1f) as u32;
    let man = (h & 0x03ff) as u32;
    let bits = if exp == 0 {
        if man == 0 {
            sign
        } else {
            let mut e = 113u32;
            let mut m = man;
            while m & 0x0400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x03ff) << 13)
        }
    } else if exp == 0x1f {
        sign | 0x7f80_0000 | (man << 13)
    } else {
        sign | ((exp + 112) << 23) | (man << 13)
    };
    f32::from_bits(bits)
}

/// The 65,536-entry half → float table the scoring kernels index (256 KB,
/// built on first use).
fn table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| (0..=u16::MAX).map(f16_to_f32).collect())
}

/// Quantize a vector: its fp16 values and the norm of the rounded vector.
pub(crate) fn quantize(v: &[f32]) -> (Vec<u16>, f32) {
    let h: Vec<u16> = v.iter().map(|&x| f32_to_f16(x)).collect();
    let t = table();
    let norm = h
        .iter()
        .map(|&x| {
            let f = t[x as usize];
            f * f
        })
        .sum::<f32>()
        .sqrt();
    (h, norm)
}

/// Widen a stored entry into a float vector.
pub(crate) fn dequantize(h: &[u16]) -> Vec<f32> {
    let t = table();
    h.iter().map(|&x| t[x as usize]).collect()
}

/// Widen an fp16 value for the scoring kernels, branch-free so the loop
/// vectorizes: the magnitude bits shifted into place as an `f32`, times
/// 2^112, re-biases normals and subnormals exactly. Infinity and NaN don't
/// survive (they come out as 65536 × 1.m); embeddings never hold them, and
/// [`dequantize`] (what `get()` returns) is exact for every value.
#[inline(always)]
fn widen(h: u16) -> f32 {
    let h = h as u32;
    let mag = f32::from_bits((h & 0x7fff) << 13) * f32::from_bits(0x7780_0000);
    f32::from_bits(mag.to_bits() | ((h & 0x8000) << 16))
}

/// `Σ q_i · v_i` with `v` in fp16: eight lanes per step, so the compiler
/// can keep two NEON (or SSE) registers of partial sums.
pub(crate) fn dot_f16(q: &[f32], h: &[u16]) -> f32 {
    let n = q.len().min(h.len());
    let (q, h) = (&q[..n], &h[..n]);
    let mut acc = [0f32; 8];
    let mut qc = q.chunks_exact(8);
    let mut hc = h.chunks_exact(8);
    for (qa, ha) in (&mut qc).zip(&mut hc) {
        let qa: &[f32; 8] = qa.try_into().expect("chunk of 8");
        let ha: &[u16; 8] = ha.try_into().expect("chunk of 8");
        for ((a, &x), &y) in acc.iter_mut().zip(qa).zip(ha) {
            *a += x * widen(y);
        }
    }
    let mut sum = ((acc[0] + acc[4]) + (acc[1] + acc[5])) + ((acc[2] + acc[6]) + (acc[3] + acc[7]));
    for (&x, &y) in qc.remainder().iter().zip(hc.remainder()) {
        sum += x * widen(y);
    }
    sum
}

/// The recall similarity of a float query against an fp16 entry, matching
/// [`crate::memory::similarity`]'s three metrics. `q_norm` is `|q|`.
pub(crate) fn similarity_f16(
    metric: crate::memory::Metric,
    q: &[f32],
    q_norm: f32,
    h: &[u16],
    v_norm: f32,
) -> f32 {
    use crate::memory::Metric;
    let dot = dot_f16(q, h);
    match metric {
        Metric::Dot => dot,
        Metric::Cosine => {
            if q_norm == 0.0 || v_norm == 0.0 {
                0.0
            } else {
                dot / (q_norm * v_norm)
            }
        }
        Metric::L2 => {
            let d2 = (q_norm * q_norm + v_norm * v_norm - 2.0 * dot).max(0.0);
            1.0 / (1.0 + d2.sqrt())
        }
    }
}

/// Encode a vector page: the header, then `entries` in slot order.
pub(crate) fn encode_page(dims: usize, gen: u64, entries: &[(&[u16], f32)]) -> Vec<u8> {
    let el = entry_len(dims);
    let mut out = Vec::with_capacity(PAGE_HEADER + entries.len() * el);
    out.push(PAGE_KIND_VECTOR);
    out.push(0); // scope
    out.push(0); // codec
    out.push(0); // reserved
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&((entries.len() * el) as u16).to_le_bytes());
    out.extend_from_slice(&gen.to_le_bytes());
    for (h, norm) in entries {
        for x in h.iter() {
            out.extend_from_slice(&x.to_le_bytes());
        }
        out.extend_from_slice(&norm.to_le_bytes());
    }
    out
}

/// Decode slot `slot` of a vector page into `out` (`dims` values);
/// returns the entry's norm.
pub(crate) fn decode_entry_into(
    page: &[u8],
    dims: usize,
    slot: usize,
    out: &mut [u16],
) -> Result<f32> {
    let bad = || MnemoError::Invalid("malformed vector page".into());
    if page.len() < PAGE_HEADER || page[0] != PAGE_KIND_VECTOR || out.len() != dims {
        return Err(bad());
    }
    let n = u16::from_le_bytes([page[4], page[5]]) as usize;
    if slot >= n {
        return Err(bad());
    }
    let el = entry_len(dims);
    let o = PAGE_HEADER + slot * el;
    let e = page.get(o..o + el).ok_or_else(bad)?;
    for (dst, c) in out.iter_mut().zip(e[..dims * 2].chunks_exact(2)) {
        *dst = u16::from_le_bytes([c[0], c[1]]);
    }
    Ok(f32::from_le_bytes([
        e[dims * 2],
        e[dims * 2 + 1],
        e[dims * 2 + 2],
        e[dims * 2 + 3],
    ]))
}

/// Decode slot `slot` of a vector page.
pub(crate) fn decode_entry(page: &[u8], dims: usize, slot: usize) -> Result<(Vec<u16>, f32)> {
    let bad = || MnemoError::Invalid("malformed vector page".into());
    if page.len() < PAGE_HEADER || page[0] != PAGE_KIND_VECTOR {
        return Err(bad());
    }
    let n = u16::from_le_bytes([page[4], page[5]]) as usize;
    if slot >= n {
        return Err(bad());
    }
    let el = entry_len(dims);
    let o = PAGE_HEADER + slot * el;
    let e = page.get(o..o + el).ok_or_else(bad)?;
    let h: Vec<u16> = e[..dims * 2]
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let norm = f32::from_le_bytes([
        e[dims * 2],
        e[dims * 2 + 1],
        e[dims * 2 + 2],
        e[dims * 2 + 3],
    ]);
    Ok((h, norm))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_round_trip_is_exact_for_every_half() {
        for h in 0..=u16::MAX {
            let f = f16_to_f32(h);
            if f.is_nan() {
                assert!(f16_to_f32(f32_to_f16(f)).is_nan());
                continue;
            }
            assert_eq!(f32_to_f16(f), h, "half {h:#06x} -> {f} -> back");
        }
    }

    #[test]
    fn f32_to_f16_rounds_to_nearest_even() {
        assert_eq!(f32_to_f16(1.0), 0x3c00);
        assert_eq!(f32_to_f16(-2.0), 0xc000);
        assert_eq!(f32_to_f16(65504.0), 0x7bff);
        assert_eq!(f32_to_f16(1e9), 0x7c00);
        assert_eq!(f32_to_f16(0.0), 0);
        assert_eq!(f32_to_f16(-0.0), 0x8000);
        // 1 + 2^-11 is halfway between 1 and the next half: ties to even (1).
        assert_eq!(f32_to_f16(1.0 + 2f32.powi(-11)), 0x3c00);
        // Just past halfway rounds up.
        assert_eq!(f32_to_f16(1.0 + 2f32.powi(-11) + 2f32.powi(-20)), 0x3c01);
        // Smallest subnormal, and half of it (ties to even: 0).
        assert_eq!(f32_to_f16(2f32.powi(-24)), 1);
        assert_eq!(f32_to_f16(2f32.powi(-25)), 0);
        // Relative error for typical embedding values stays under 2^-11.
        for i in 1..1000 {
            let x = i as f32 * 0.000_37 - 0.18;
            let y = f16_to_f32(f32_to_f16(x));
            assert!((x - y).abs() <= x.abs() * 2f32.powi(-11) + 2f32.powi(-25));
        }
    }

    #[test]
    fn fast_widen_is_exact_for_every_finite_half() {
        for h in 0..=u16::MAX {
            let f = f16_to_f32(h);
            if f.is_finite() {
                assert_eq!(widen(h).to_bits(), f.to_bits(), "half {h:#06x}");
            }
        }
        let q: Vec<f32> = (0..19).map(|i| i as f32 * 0.1 - 0.7).collect();
        let (h, _) = quantize(&q);
        let slow: f32 = q.iter().zip(dequantize(&h)).map(|(a, b)| a * b).sum();
        assert!((dot_f16(&q, &h) - slow).abs() < 1e-5);
    }

    #[test]
    fn page_round_trip_and_similarity() {
        let dims = 384;
        assert_eq!(entry_len(dims), 772);
        assert_eq!(per_page(dims), 10);
        let vs: Vec<Vec<f32>> = (0..10)
            .map(|k| {
                (0..dims)
                    .map(|i| ((i * 7 + k * 13) % 17) as f32 / 17.0 - 0.5)
                    .collect()
            })
            .collect();
        let qs: Vec<(Vec<u16>, f32)> = vs.iter().map(|v| quantize(v)).collect();
        let refs: Vec<(&[u16], f32)> = qs.iter().map(|(h, n)| (h.as_slice(), *n)).collect();
        let page = encode_page(dims, 42, &refs);
        assert!(page.len() <= PAYLOAD);
        for (slot, (h, n)) in qs.iter().enumerate() {
            let (h2, n2) = decode_entry(&page, dims, slot).unwrap();
            assert_eq!(&h2, h);
            assert_eq!(n2, *n);
        }
        assert!(decode_entry(&page, dims, 10).is_err());
        // fp16 similarity tracks the float one closely.
        let q = &vs[3];
        let qn = q.iter().map(|x| x * x).sum::<f32>().sqrt();
        for (v, (h, n)) in vs.iter().zip(&qs) {
            for metric in [
                crate::memory::Metric::Cosine,
                crate::memory::Metric::Dot,
                crate::memory::Metric::L2,
            ] {
                let exact = crate::memory::similarity(metric, q, v);
                let approx = similarity_f16(metric, q, qn, h, *n);
                let tol = 2e-3 * exact.abs().max(1.0);
                assert!(
                    (exact - approx).abs() < tol,
                    "{metric:?}: {exact} vs {approx}"
                );
            }
        }
        assert_eq!(dequantize(&qs[0].0).len(), dims);
    }
}
