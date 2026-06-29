use crate::*;

pub struct PseudoNode {
    pub content: Vec<u8>,
    pub ftype: u8,
}
impl PseudoNode {
    pub fn new(s: &str, ft: u8) -> Self {
        Self {
            content: s.as_bytes().to_vec(),
            ftype: ft,
        }
    }
    pub fn read_at(&self, off: usize, buf: &mut [u8]) -> usize {
        if off >= self.content.len() {
            return 0;
        }
        let n = min(self.content.len() - off, buf.len());
        buf[..n].copy_from_slice(&self.content[off..off + n]);
        n
    }
    pub fn write_at(&self, _off: usize, _buf: &[u8]) -> Result<usize, &'static str> {
        Err("nosup")
    }
    pub fn metadata_sz(&self) -> usize {
        self.content.len()
    }
}

pub fn read_as_vec(data: &[u8]) -> Vec<u8> {
    data.to_vec()
}
