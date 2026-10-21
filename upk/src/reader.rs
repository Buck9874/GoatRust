use anyhow::{Result, bail};

/// Little-endian cursor over a byte slice.
#[derive(Clone)]
pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self { data, pos }
    }
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            bail!("read past end: pos {} + {} > {}", self.pos, n, self.data.len());
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn f16(&mut self) -> Result<f32> {
        Ok(half::f16::from_bits(self.u16()?).to_f32())
    }
    pub fn vec3(&mut self) -> Result<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }
    pub fn guid(&mut self) -> Result<[u8; 16]> {
        Ok(self.bytes(16)?.try_into().unwrap())
    }
    /// UE3 FString: i32 length (negative = UTF-16), includes terminator.
    pub fn fstring(&mut self) -> Result<String> {
        let n = self.i32()?;
        if n == 0 {
            return Ok(String::new());
        }
        if n > 0 {
            if n > 1 << 20 {
                bail!("fstring too long: {n}");
            }
            let b = self.bytes(n as usize)?;
            let b = b.strip_suffix(&[0]).unwrap_or(b);
            Ok(b.iter().map(|&c| c as char).collect())
        } else {
            let n = (-n) as usize;
            if n > 1 << 20 {
                bail!("fstring too long: {n}");
            }
            let mut v = Vec::with_capacity(n);
            for _ in 0..n {
                v.push(self.u16()?);
            }
            if v.last() == Some(&0) {
                v.pop();
            }
            Ok(String::from_utf16_lossy(&v))
        }
    }
    pub fn array<T>(&mut self, mut f: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let n = self.i32()?;
        if n < 0 || n as usize > self.remaining().max(16) * 4 + 16 {
            bail!("bad array count {n} at {}", self.pos - 4);
        }
        let mut v = Vec::with_capacity(n as usize);
        for _ in 0..n {
            v.push(f(self)?);
        }
        Ok(v)
    }
}
