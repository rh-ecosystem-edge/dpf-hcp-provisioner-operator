//! Go `hash/crc32` with the IEEE polynomial.

const IEEE: u32 = 0xedb88320;

const fn make_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut j = 0;
        while j < 8 {
            if crc & 1 == 1 {
                crc = (crc >> 1) ^ IEEE;
            } else {
                crc >>= 1;
            }
            j += 1;
        }
        t[i] = crc;
        i += 1;
    }
    t
}

static TABLE: [u32; 256] = make_table();

/// Go `crc32.Update(crc, crc32.IEEETable, p)`.
pub fn update(crc: u32, p: &[u8]) -> u32 {
    let mut crc = !crc;
    for &b in p {
        crc = TABLE[((crc as u8) ^ b) as usize] ^ (crc >> 8);
    }
    !crc
}

/// Go `crc32.ChecksumIEEE`.
pub fn checksum_ieee(p: &[u8]) -> u32 {
    update(0, p)
}
