pub fn make_table_crc16(poly: u16) -> [u16; 256] {
    let mut table = [0u16; 256];
    for i in 0..256 {
        let mut value = i as u16;
        for _ in 0..8 {
            value = if (value & 1) == 1 {
                (value >> 1) ^ poly
            } else {
                value >> 1
            }
        }
        table[i] = value;
    }
    table
}

pub fn make_table_crc32(poly: u32) -> [u32; 256] {
    let mut table = [0u32; 256];
    for i in 0..256 {
        let mut value = i as u32;
        for _ in 0..8 {
            value = if (value & 1) == 1 {
                (value >> 1) ^ poly
            } else {
                value >> 1
            }
        }
        table[i] = value;
    }
    table
}

pub fn make_table_crc64(poly: u64) -> [u64; 256] {
    let mut table = [0u64; 256];
    for i in 0..256 {
        let mut value = i as u64;
        for _ in 0..8 {
            value = if (value & 1) == 1 {
                (value >> 1) ^ poly
            } else {
                value >> 1
            }
        }
        table[i] = value;
    }
    table
}
