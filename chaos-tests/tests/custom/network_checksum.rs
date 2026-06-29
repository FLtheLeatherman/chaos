use chaos_tests::*;

#[test]
fn tcp_checksum_empty_payload_uses_tcp_protocol_number() {
    assert_eq!(tcp_checksum(0, 0, &[]), 0xfff9);
}

#[test]
fn tcp_checksum_matches_known_vector() {
    let src_ip = 0x0a00_0001;
    let dst_ip = 0x0a00_0002;

    assert_eq!(tcp_checksum(src_ip, dst_ip, b"hello"), 0xa81f);
}

#[test]
fn tcp_checksum_matches_pseudo_header_plus_inet_checksum() {
    let src_ip = 0x0a00_0001;
    let dst_ip = 0x0a00_0002;
    let payload = [0x12, 0x34, 0x56, 0x78, 0x9a];

    let mut data = build_pseudo_header(src_ip, dst_ip, 6, payload.len() as u16);
    data.extend_from_slice(&payload);

    assert_eq!(tcp_checksum(src_ip, dst_ip, &payload), compute_inet_checksum(&data));
}
