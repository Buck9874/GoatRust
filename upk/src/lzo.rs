//! Minimal LZO1X decompressor (the codec UE3 uses for cooked PC packages and TFC chunks).

use anyhow::{Result, bail};

pub fn decompress(src: &[u8], out_len: usize) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(out_len);
    let mut ip = 0usize;
    let get = |i: usize| -> Result<u8> { src.get(i).copied().ok_or_else(|| anyhow::anyhow!("lzo: input overrun")) };

    // copy match from distance
    fn copy_match(out: &mut Vec<u8>, dist: usize, len: usize) -> Result<()> {
        if dist == 0 || dist > out.len() {
            bail!("lzo: bad distance {dist} (have {})", out.len());
        }
        let start = out.len() - dist;
        for k in 0..len {
            let b = out[start + k];
            out.push(b);
        }
        Ok(())
    }
    fn copy_lit(out: &mut Vec<u8>, src: &[u8], ip: &mut usize, n: usize) -> Result<()> {
        let end = *ip + n;
        if end > src.len() {
            bail!("lzo: literal overrun");
        }
        out.extend_from_slice(&src[*ip..end]);
        *ip = end;
        Ok(())
    }
    fn read_len(src: &[u8], ip: &mut usize, base: usize) -> Result<usize> {
        // zero bytes add 255 each, then final byte
        let mut n = 0usize;
        loop {
            let b = *src.get(*ip).ok_or_else(|| anyhow::anyhow!("lzo: len overrun"))?;
            *ip += 1;
            if b == 0 {
                n += 255;
            } else {
                return Ok(n + base + b as usize);
            }
        }
    }

    // state: number of trailing literals copied by the last instruction (0..3), or 4 for "after long literal run"
    let mut state: usize;
    let first = get(ip)? as usize;
    if first > 17 {
        ip += 1;
        let n = first - 17;
        copy_lit(&mut out, src, &mut ip, n)?;
        state = if n < 4 { n } else { 4 };
    } else {
        state = 0;
    }

    loop {
        let t = get(ip)? as usize;
        ip += 1;
        if t < 16 {
            if state == 0 {
                // literal run
                let n = if t == 0 { read_len(src, &mut ip, 15)? + 3 } else { t + 3 };
                copy_lit(&mut out, src, &mut ip, n)?;
                state = 4;
                continue;
            } else if state < 4 {
                // 2-byte match, distance up to 1KB
                let b = get(ip)? as usize;
                ip += 1;
                let dist = (b << 2) + (t >> 2) + 1;
                copy_match(&mut out, dist, 2)?;
                let n = t & 3;
                copy_lit(&mut out, src, &mut ip, n)?;
                state = n;
                continue;
            } else {
                // 3-byte match, distance 2049..3072
                let b = get(ip)? as usize;
                ip += 1;
                let dist = (b << 2) + (t >> 2) + 2049;
                copy_match(&mut out, dist, 3)?;
                let n = t & 3;
                copy_lit(&mut out, src, &mut ip, n)?;
                state = n;
                continue;
            }
        }
        let (len, dist, n);
        if t >= 64 {
            // M2
            let b = get(ip)? as usize;
            ip += 1;
            len = (t >> 5) - 1 + 2;
            dist = (b << 3) + ((t >> 2) & 7) + 1;
            n = t & 3;
        } else if t >= 32 {
            // M3
            len = if t & 31 == 0 { read_len(src, &mut ip, 31)? + 2 } else { (t & 31) + 2 };
            let d0 = get(ip)? as usize;
            let d1 = get(ip + 1)? as usize;
            ip += 2;
            let d = d0 | (d1 << 8);
            dist = (d >> 2) + 1;
            n = d0 & 3;
        } else {
            // M4 (16..31)
            len = if t & 7 == 0 { read_len(src, &mut ip, 7)? + 2 } else { (t & 7) + 2 };
            let d0 = get(ip)? as usize;
            let d1 = get(ip + 1)? as usize;
            ip += 2;
            let d = d0 | (d1 << 8);
            let dd = ((t & 8) << 11) + (d >> 2);
            if dd == 0 {
                // end of stream
                break;
            }
            dist = dd + 16384;
            n = d0 & 3;
        }
        copy_match(&mut out, dist, len)?;
        copy_lit(&mut out, src, &mut ip, n)?;
        state = n;
    }
    if out.len() != out_len {
        bail!("lzo: produced {} bytes, expected {out_len}", out.len());
    }
    Ok(out)
}
