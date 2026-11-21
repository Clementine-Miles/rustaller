//! Bounded prefix checks for a Linux x86-64 bzImage with the EFI stub initrd
//! media device-path ABI. Firmware LoadImage still validates the entire PE.
pub fn valid_efi_stub(header: &[u8]) -> bool {
    fn word(data: &[u8], offset: usize) -> Option<u16> {
        Some(u16::from_le_bytes(
            data.get(offset..offset + 2)?.try_into().ok()?,
        ))
    }
    if header.get(..2) != Some(b"MZ")
        || header.get(0x202..0x206) != Some(b"HdrS")
        || word(header, 0x1fe) != Some(0xaa55)
        || word(header, 0x236).is_none_or(|flags| flags & 1 == 0)
    {
        return false;
    }
    let Some(offset) = header
        .get(0x3c..0x40)
        .and_then(|v| v.try_into().ok())
        .map(u32::from_le_bytes)
    else {
        return false;
    };
    let Some(pe) = header.get(offset as usize..) else {
        return false;
    };
    // PE32+ optional header starts 24 bytes from the PE signature.
    pe.get(..4) == Some(b"PE\0\0")
        && word(pe, 4) == Some(0x8664)
        && word(pe, 20).is_some_and(|n| n >= 70)
        && word(pe, 24) == Some(0x20b)
        && word(pe, 24 + 68) == Some(10)
        && word(pe, 24 + 44)
            .is_some_and(|major| matches!(major, 1 | 2) || (major >= 3 && major & 1 != 0))
}

pub fn valid_bzimage(b: &[u8]) -> bool {
    b.len() >= 0x264
        && b[0x1fe..0x200] == [0x55, 0xaa]
        && &b[0x202..0x206] == b"HdrS"
        && u16::from_le_bytes([b[0x206], b[0x207]]) >= 0x20c
        && b[0x211] & 1 != 0
        && b[0x234] != 0
        && b[0x236] & 1 != 0
}
