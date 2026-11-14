use std::fmt;

pub mod bundle;
pub mod check;
pub mod dist;
pub mod local_tool;
pub mod sources;
pub mod vm;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElfValidationError(pub String);

impl fmt::Display for ElfValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ElfValidationError {}

pub fn verify_x86_64_elf(
    bytes: &[u8],
    require_static: bool,
    runtime_version: &str,
) -> Result<(), ElfValidationError> {
    if bytes.len() < 64 || &bytes[..4] != b"\x7fELF" {
        return Err(ElfValidationError("invalid or truncated ELF header".into()));
    }
    if bytes[4] != 2 || bytes[5] != 1 || bytes[6] != 1 {
        return Err(ElfValidationError(
            "ELF must be 64-bit, little-endian, version 1".into(),
        ));
    }
    if !matches!(read_u16(bytes, 16)?, 2 | 3) {
        return Err(ElfValidationError(
            "ELF type must be executable or position-independent executable".into(),
        ));
    }
    if read_u16(bytes, 18)? != 0x3e {
        return Err(ElfValidationError("ELF machine is not x86_64".into()));
    }
    if read_u16(bytes, 52)? != 64 {
        return Err(ElfValidationError("unexpected ELF64 header size".into()));
    }
    if runtime_version.is_empty() || !contains_bytes(bytes, runtime_version.as_bytes()) {
        return Err(ElfValidationError(
            "ELF does not contain the expected runtime version".into(),
        ));
    }

    let phoff = usize::try_from(read_u64(bytes, 32)?)
        .map_err(|_| ElfValidationError("ELF program-header offset overflows usize".into()))?;
    let phentsize = usize::from(read_u16(bytes, 54)?);
    let phnum = usize::from(read_u16(bytes, 56)?);
    if phentsize < 56 {
        return Err(ElfValidationError(
            "ELF program-header entries are smaller than ELF64".into(),
        ));
    }
    let table_len = phentsize
        .checked_mul(phnum)
        .and_then(|len| phoff.checked_add(len))
        .ok_or_else(|| ElfValidationError("ELF program-header table overflows".into()))?;
    if table_len > bytes.len() {
        return Err(ElfValidationError(
            "ELF program-header table is truncated".into(),
        ));
    }

    let mut has_load_segment = false;
    for index in 0..phnum {
        let offset = phoff + index * phentsize;
        match read_u32(bytes, offset)? {
            1 => has_load_segment = true,
            2 if require_static => {
                return Err(ElfValidationError(
                    "static ELF must not contain a PT_DYNAMIC segment".into(),
                ));
            }
            3 if require_static => {
                return Err(ElfValidationError(
                    "static ELF must not contain a PT_INTERP segment".into(),
                ));
            }
            _ => {}
        }
    }
    if !has_load_segment {
        return Err(ElfValidationError("ELF has no PT_LOAD segment".into()));
    }
    Ok(())
}

pub fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ElfValidationError> {
    let slice = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| ElfValidationError("truncated ELF field".into()))?;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ElfValidationError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ElfValidationError("truncated ELF field".into()))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ElfValidationError> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| ElfValidationError("truncated ELF field".into()))?;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

#[cfg(test)]
mod tests {
    use super::verify_x86_64_elf;

    fn fake_elf(program_type: u32) -> Vec<u8> {
        let mut bytes = vec![0; 64 + 56];
        bytes[..4].copy_from_slice(b"\x7fELF");
        bytes[4] = 2;
        bytes[5] = 1;
        bytes[6] = 1;
        bytes[16..18].copy_from_slice(&2u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&0x3eu16.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&1u16.to_le_bytes());
        bytes[64..68].copy_from_slice(&program_type.to_le_bytes());
        bytes.extend_from_slice(rustaller::RUNTIME_VERSION.as_bytes());
        bytes
    }

    #[test]
    fn accepts_static_x86_64_elf_with_matching_runtime_marker() {
        assert!(verify_x86_64_elf(&fake_elf(1), true, rustaller::RUNTIME_VERSION).is_ok());
    }

    #[test]
    fn rejects_dynamic_interpreter_and_wrong_architecture() {
        assert!(verify_x86_64_elf(&fake_elf(2), true, rustaller::RUNTIME_VERSION).is_err());
        assert!(verify_x86_64_elf(&fake_elf(3), true, rustaller::RUNTIME_VERSION).is_err());
        let mut wrong_arch = fake_elf(1);
        wrong_arch[18..20].copy_from_slice(&0x3eu16.wrapping_sub(1).to_le_bytes());
        assert!(verify_x86_64_elf(&wrong_arch, true, rustaller::RUNTIME_VERSION).is_err());
    }

    #[test]
    fn rejects_truncation_missing_load_segment_and_version_mismatch() {
        assert!(verify_x86_64_elf(b"\x7fELF", true, rustaller::RUNTIME_VERSION).is_err());
        assert!(verify_x86_64_elf(&fake_elf(4), true, rustaller::RUNTIME_VERSION).is_err());
        assert!(verify_x86_64_elf(&fake_elf(1), true, "some-other-version").is_err());
    }
}
