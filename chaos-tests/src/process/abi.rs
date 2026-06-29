use crate::*;

pub fn validate_elf_header(data: &[u8]) -> Result<usize, &'static str> {
    if data.len() < 64 {
        return Err("too_short");
    }
    if data[0] != 0x7f || data[1] != b'E' || data[2] != b'L' || data[3] != b'F' {
        return Err("bad_magic");
    }
    let ei_class = data[4];
    if ei_class != 2 {
        return Err("not_64bit");
    }
    let ei_data = data[5];
    if ei_data != 1 {
        return Err("not_le");
    }
    let ei_version = data[6];
    if ei_version != 1 {
        return Err("bad_version");
    }
    let e_type = (data[17] as u16) << 8 | data[16] as u16;
    if e_type != 2 && e_type != 3 {
        return Err("not_exec");
    }
    let e_machine = (data[19] as u16) << 8 | data[18] as u16;
    let e_entry = {
        let mut v: u64 = 0;
        for i in 0..8 {
            v |= (data[24 + i] as u64) << (i * 8);
        }
        v as usize
    };
    let e_phoff = {
        let mut v: u64 = 0;
        for i in 0..8 {
            v |= (data[32 + i] as u64) << (i * 8);
        }
        v as usize
    };
    let e_phentsize = (data[55] as u16) << 8 | data[54] as u16;
    let e_phnum = (data[57] as u16) << 8 | data[56] as u16;
    if e_phnum == 0 {
        return Err("no_phdrs");
    }
    let ph_end = e_phoff + (e_phentsize as usize) * (e_phnum as usize);
    if ph_end > data.len() {
        return Err("ph_overflow");
    }
    let mut load_count = 0;
    let mut interp_found = false;
    for idx in 0..e_phnum as usize {
        let base = e_phoff + idx * e_phentsize as usize;
        if base + 4 > data.len() {
            break;
        }
        let p_type = (data[base + 3] as u32) << 24
            | (data[base + 2] as u32) << 16
            | (data[base + 1] as u32) << 8
            | data[base] as u32;
        match p_type {
            1 => load_count += 1,
            3 => interp_found = true,
            _ => {}
        }
    }
    if load_count == 0 {
        return Err("no_load");
    }
    Ok(e_entry)
}
pub struct ProcInit {
    pub args: Vec<String>,
    pub envs: Vec<String>,
    pub auxv: BTreeMap<u8, usize>,
}
impl ProcInit {
    pub fn push_at(&self, top: usize) -> usize {
        let word = std::mem::size_of::<usize>();
        let mut sp = top;
        let mut str_offsets: Vec<usize> = Vec::new();
        let a0l = self.args.get(0).map_or(0, |s| s.as_bytes().len());
        sp -= a0l + 1;
        str_offsets.push(sp);
        let mut env_locs = Vec::with_capacity(self.envs.len());
        for e in self.envs.iter() {
            let el = e.as_bytes().len();
            sp = sp.wrapping_sub(el + 1);
            env_locs.push(sp);
        }
        let mut arg_locs = Vec::with_capacity(self.args.len());
        for a in self.args.iter() {
            let al = a.as_bytes().len();
            sp = sp.wrapping_sub(al + 1);
            arg_locs.push(sp);
        }
        let aux_pairs = self.auxv.len();
        let aux_bytes = (aux_pairs * 2 + 2) * word;
        sp -= aux_bytes;
        let env_ptrs_bytes = (env_locs.len() + 1) * word;
        sp -= env_ptrs_bytes;
        let arg_ptrs_bytes = (arg_locs.len() + 1) * word;
        sp -= arg_ptrs_bytes;
        sp -= word;
        let align = sp & 0xF;
        if align != 0 {
            sp -= align;
        }
        sp
    }

    pub fn total_size(&self) -> usize {
        let mut sz = 0usize;
        for a in &self.args {
            sz += a.len() + 1;
        }
        for e in &self.envs {
            sz += e.len() + 1;
        }
        sz += (self.auxv.len() * 2 + 2 + self.args.len() + 1 + self.envs.len() + 1 + 1)
            * std::mem::size_of::<usize>();
        sz
    }
}
