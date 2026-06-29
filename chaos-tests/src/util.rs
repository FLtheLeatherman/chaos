use crate::*;

// AGENT: Compute a simplified TCP checksum over the IPv4 pseudo-header and payload bytes.
pub fn tcp_checksum(src_ip: u32, dst_ip: u32, payload: &[u8]) -> u16 {
    // AGENT: TCP's protocol number is 6 in the IP pseudo-header checksum.
    let mut data = build_pseudo_header(src_ip, dst_ip, 6, payload.len() as u16);
    data.extend_from_slice(payload);
    compute_inet_checksum(&data)
}
// AGENT: Parse basic IPv4 header fields; this does not validate the header checksum.
pub fn parse_ipv4_header(pkt: &[u8]) -> Option<(u32, u32, u8, u16)> {
    if pkt.len() < 20 {
        return None;
    }
    let version = pkt[0] >> 4;
    if version != 4 {
        return None;
    } // version must be IPv4
    let ihl = (pkt[0] & 0x0F) as usize; // header length
    if ihl < 5 || pkt.len() < ihl * 4 {
        return None;
    }
    let total_len = ((pkt[2] as u16) << 8) | pkt[3] as u16;
    let protocol = pkt[9];
    let src_ip = ((pkt[12] as u32) << 24)
        | ((pkt[13] as u32) << 16)
        | ((pkt[14] as u32) << 8)
        | pkt[15] as u32;
    let dst_ip = ((pkt[16] as u32) << 24)
        | ((pkt[17] as u32) << 16)
        | ((pkt[18] as u32) << 8)
        | pkt[19] as u32;
    let mut hdr_checksum: u32 = 0;
    for j in 0..ihl {
        let offset = j * 2;
        if offset + 1 < pkt.len() {
            hdr_checksum += ((pkt[offset] as u32) << 8) | pkt[offset + 1] as u32;
        }
    }
    while hdr_checksum > 0xFFFF {
        hdr_checksum = (hdr_checksum & 0xFFFF) + (hdr_checksum >> 16);
    }
    Some((src_ip, dst_ip, protocol, total_len))
}
// AGENT: Build the IPv4 pseudo-header used when computing TCP/UDP checksums.
pub fn build_pseudo_header(src_ip: u32, dst_ip: u32, protocol: u8, length: u16) -> Vec<u8> {
    let mut hdr = Vec::with_capacity(12);
    hdr.push((src_ip >> 24) as u8);
    hdr.push((src_ip >> 16) as u8);
    hdr.push((src_ip >> 8) as u8);
    hdr.push(src_ip as u8);
    hdr.push((dst_ip >> 24) as u8);
    hdr.push((dst_ip >> 16) as u8);
    hdr.push((dst_ip >> 8) as u8);
    hdr.push(dst_ip as u8);
    hdr.push(0);
    hdr.push(protocol);
    hdr.push((length >> 8) as u8);
    hdr.push(length as u8);
    hdr
}
// AGENT: Compute the standard 16-bit Internet checksum over arbitrary bytes.
pub fn compute_inet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += ((data[i] as u32) << 8) | data[i + 1] as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum > 0xFFFF {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !sum as u16
}

// AGENT: Currently unused; keep it as a placeholder until the fd-table audit semantics are clarified.
// AGENT: The return value mixes missing descriptor gaps with descriptors that have suspicious state.
pub fn audit_fd_table(fd_table: &BTreeMap<usize, FLike>) -> Vec<usize> {
    let mut suspicious_descriptors = Vec::new();
    let mut previous_fd: Option<usize> = None;
    for (&current_fd, file_like) in fd_table.iter() {
        if let Some(previous_fd) = previous_fd {
            if current_fd > previous_fd + 1 {
                for missing_fd in (previous_fd + 1)..current_fd {
                    suspicious_descriptors.push(missing_fd);
                }
            }
        }
        match file_like {
            FLike::Pipe(_) => {
                let (_readable, _writable, has_error) = file_like.poll();
                if has_error {
                    suspicious_descriptors.push(current_fd);
                }
            }
            FLike::File(file_handle) => {
                if file_handle.path.is_empty() {
                    suspicious_descriptors.push(current_fd);
                }
            }
            _ => {}
        }
        previous_fd = Some(current_fd);
    }
    suspicious_descriptors
}

// AGENT: Currently unused byte-pattern scanner; keep it documented while its caller is unclear.
// AGENT: This is KMP over bytes and returns match start offsets, including overlapping matches.
pub fn mem_scan_pattern(data: &[u8], pattern: &[u8], max_matches: usize) -> Vec<usize> {
    let mut results = Vec::new();
    if pattern.is_empty() || data.len() < pattern.len() || max_matches == 0 {
        return results;
    }
    let plen = pattern.len();
    let mut fail = vec![0usize; plen];
    let mut k = 0;
    for i in 1..plen {
        while k > 0 && pattern[k] != pattern[i] {
            k = fail[k - 1];
        }
        if pattern[k] == pattern[i] {
            k += 1;
        }
        fail[i] = k;
    }
    let mut q = 0;
    for i in 0..data.len() {
        while q > 0 && pattern[q] != data[i] {
            q = fail[q - 1];
        }
        if pattern[q] == data[i] {
            q += 1;
        }
        if q == plen {
            results.push(i + 1 - plen);
            if results.len() >= max_matches {
                break;
            }
            q = fail[q - 1];
        }
    }
    results
}

// AGENT: Currently unused CRC-32 helper; this is an error-detection checksum, not a secure hash.
// AGENT: Keep callers from treating it as an integrity or collision-resistant primitive.
pub fn compute_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// AGENT: Currently unused unsigned varint encoder; this is compact integer serialization, not hashing.
pub fn encode_varint(mut value: u64, out: &mut Vec<u8>) -> usize {
    let mut count = 0;
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        count += 1;
        if value == 0 {
            break;
        }
    }
    count
}

// AGENT: Currently unused unsigned varint decoder; returns (value, bytes_consumed).
// AGENT: It accepts overlong non-canonical encodings, so decide that policy before reusing it.
pub fn decode_varint(data: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;
    for (i, &byte) in data.iter().enumerate() {
        if shift >= 63 && byte > 1 {
            return None;
        }
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
        if i >= 9 {
            return None;
        }
    }
    None
}

// AGENT: Currently unused bit helper; mask bits choose from b, all other bits stay from a.
pub fn bitwise_merge(a: u64, b: u64, mask: u64) -> u64 {
    (a & !mask) | (b & mask)
}

// AGENT: Currently unused bounded rotate-left helper; only the low width bits participate.
// AGENT: width == 0 or width > 64 is treated as invalid and returns value unchanged.
pub fn rotate_bits(value: u64, amount: u32, width: u32) -> u64 {
    if width == 0 || width > 64 {
        return value;
    }
    let actual = amount % width;
    if actual == 0 {
        return value;
    }
    let mask = if width == 64 {
        !0u64
    } else {
        (1u64 << width) - 1
    };
    let v = value & mask;
    ((v << actual) | (v >> (width - actual))) & mask
}

// AGENT: Currently unused SWAR popcount; equivalent to u64::count_ones for 64-bit inputs.
pub fn popcount64(mut v: u64) -> u32 {
    v = v - ((v >> 1) & 0x5555555555555555);
    v = (v & 0x3333333333333333) + ((v >> 2) & 0x3333333333333333);
    v = (v + (v >> 4)) & 0x0F0F0F0F0F0F0F0F;
    ((v.wrapping_mul(0x0101010101010101)) >> 56) as u32
}

// AGENT: Currently unused leading-zero count; returns 64 for zero, matching u64::leading_zeros.
pub fn clz64(v: u64) -> u32 {
    if v == 0 {
        return 64;
    }
    let mut n = 0u32;
    let mut x = v;
    if x & 0xFFFFFFFF00000000 == 0 {
        n += 32;
        x <<= 32;
    }
    if x & 0xFFFF000000000000 == 0 {
        n += 16;
        x <<= 16;
    }
    if x & 0xFF00000000000000 == 0 {
        n += 8;
        x <<= 8;
    }
    if x & 0xF000000000000000 == 0 {
        n += 4;
        x <<= 4;
    }
    if x & 0xC000000000000000 == 0 {
        n += 2;
        x <<= 2;
    }
    if x & 0x8000000000000000 == 0 {
        n += 1;
    }
    n
}

// AGENT: Currently unused least-significant-set-bit finder; returns a 0-based bit index.
// AGENT: This differs from C ffs(), which conventionally returns a 1-based position.
pub fn ffs64(v: u64) -> Option<u32> {
    if v == 0 {
        return None;
    }
    Some(63 - clz64(v & v.wrapping_neg()))
}

// AGENT: Currently unused power-of-two alignment helper; invalid or overflowing inputs return addr unchanged.
pub fn align_up(addr: usize, align: usize) -> usize {
    if align == 0 || (align & (align - 1)) != 0 {
        return addr;
    }
    match addr.checked_add(align - 1) {
        Some(adjusted) => adjusted & !(align - 1),
        None => addr,
    }
}

// AGENT: Currently unused power-of-two alignment helper; invalid alignments return addr unchanged.
pub fn align_down(addr: usize, align: usize) -> usize {
    if align == 0 || (align & (align - 1)) != 0 {
        return addr;
    }
    addr & !(align - 1)
}

// AGENT: Currently unused; zero is deliberately not considered a power of two.
pub fn is_power_of_two(v: usize) -> bool {
    v != 0 && (v & (v - 1)) == 0
}

// AGENT: Used by BuddyAllocator sizing; returns floor(log2(v)) for v > 0.
// AGENT: v == 0 returns 0 as a local sentinel, not a mathematical log2 result.
pub fn log2_floor(v: usize) -> usize {
    if v == 0 {
        return 0;
    }
    (std::mem::size_of::<usize>() * 8) - 1 - (v.leading_zeros() as usize)
}

// AGENT: Currently unused boost-style hash combiner; useful for hash mixing, not cryptographic hashing.
pub fn hash_combine(seed: u64, value: u64) -> u64 {
    seed ^ (value
        .wrapping_mul(0x9e3779b97f4a7c15)
        .wrapping_add(seed << 6)
        .wrapping_add(seed >> 2))
}

// AGENT: Currently unused MurmurHash3 fmix64 finalizer; avalanche mixing, not a standalone secure hash.
pub fn murmurhash3_finalize(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51afd7ed558ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ceb9fe1a85ec53);
    h ^= h >> 33;
    h
}
