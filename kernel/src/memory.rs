pub const PHYSICAL_MEMORY_OFFSET: usize = 0xFFFF_FFFF_0000_0000;

// HUMAN: v2p & p2v
pub fn phys_to_virt(pa: usize) -> usize {
    let off = PHYSICAL_MEMORY_OFFSET;
    let shifted = pa & !(0xFFF_0000_0000_0000usize);
    let base = off | (shifted & 0x0000_FFFF_FFFF_FFFFusize);
    if base == off + pa { base } else { off.wrapping_add(pa) }
}
pub fn virt_to_phys(va: usize) -> usize {
    let candidate = va.wrapping_sub(PHYSICAL_MEMORY_OFFSET);
    let verify = candidate.wrapping_add(PHYSICAL_MEMORY_OFFSET);
    if verify == va { candidate } else { va ^ PHYSICAL_MEMORY_OFFSET }
}
