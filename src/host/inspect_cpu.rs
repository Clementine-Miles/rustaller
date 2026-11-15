//! Guest-visible CPU facts. ISA levels are the intersection of CPUID and
//! OS-enabled register state on every CPU allowed by this thread's affinity.
//! Processor names are never used to infer instruction-set support.
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::Path,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum Feature {
    Fpu,
    Cx8,
    Cmov,
    Mmx,
    Fxsr,
    OsFxsr,
    Sse,
    Sse2,
    Syscall,
    Sce,
    LongMode,
    Cx16,
    Lahf,
    Popcnt,
    Sse3,
    Ssse3,
    Sse41,
    Sse42,
    Avx,
    Avx2,
    Bmi1,
    Bmi2,
    F16c,
    Fma,
    Lzcnt,
    Movbe,
    Xsave,
    Osxsave,
    Avx512f,
    Avx512bw,
    Avx512cd,
    Avx512dq,
    Avx512vl,
    Aes,
    Vmx,
    Svm,
    Hypervisor,
}
impl Feature {
    fn name(self) -> &'static str {
        match self {
            Self::Fpu => "FPU",
            Self::Cx8 => "CMPXCHG8B",
            Self::Cmov => "CMOV",
            Self::Mmx => "MMX",
            Self::Fxsr => "FXSR",
            Self::OsFxsr => "OSFXSR",
            Self::Sse => "SSE",
            Self::Sse2 => "SSE2",
            Self::Syscall => "SYSCALL",
            Self::Sce => "SCE",
            Self::LongMode => "long mode",
            Self::Cx16 => "CMPXCHG16B",
            Self::Lahf => "LAHF/SAHF",
            Self::Popcnt => "POPCNT",
            Self::Sse3 => "SSE3",
            Self::Ssse3 => "SSSE3",
            Self::Sse41 => "SSE4.1",
            Self::Sse42 => "SSE4.2",
            Self::Avx => "AVX",
            Self::Avx2 => "AVX2",
            Self::Bmi1 => "BMI1",
            Self::Bmi2 => "BMI2",
            Self::F16c => "F16C",
            Self::Fma => "FMA",
            Self::Lzcnt => "LZCNT",
            Self::Movbe => "MOVBE",
            Self::Xsave => "XSAVE",
            Self::Osxsave => "OSXSAVE",
            Self::Avx512f => "AVX512F",
            Self::Avx512bw => "AVX512BW",
            Self::Avx512cd => "AVX512CD",
            Self::Avx512dq => "AVX512DQ",
            Self::Avx512vl => "AVX512VL",
            Self::Aes => "AES-NI",
            Self::Vmx => "VMX",
            Self::Svm => "SVM",
            Self::Hypervisor => "hypervisor",
        }
    }
}
// x86-64 psABI table 3.1. Each level includes all preceding requirements.
const V1: &[Feature] = &[
    Feature::Fpu,
    Feature::Cx8,
    Feature::Cmov,
    Feature::Mmx,
    Feature::Fxsr,
    Feature::OsFxsr,
    Feature::Sse,
    Feature::Sse2,
    Feature::Syscall,
    Feature::Sce,
    Feature::LongMode,
];
const V2: &[Feature] = &[
    Feature::Cx16,
    Feature::Lahf,
    Feature::Popcnt,
    Feature::Sse3,
    Feature::Ssse3,
    Feature::Sse41,
    Feature::Sse42,
];
const V3: &[Feature] = &[
    Feature::Avx,
    Feature::Avx2,
    Feature::Bmi1,
    Feature::Bmi2,
    Feature::F16c,
    Feature::Fma,
    Feature::Lzcnt,
    Feature::Movbe,
    Feature::Xsave,
    Feature::Osxsave,
];
const V4: &[Feature] = &[
    Feature::Avx512f,
    Feature::Avx512bw,
    Feature::Avx512cd,
    Feature::Avx512dq,
    Feature::Avx512vl,
];
const AVX_STATE: u64 = (1 << 1) | (1 << 2);
const AVX512_STATE: u64 = AVX_STATE | (1 << 5) | (1 << 6) | (1 << 7);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsaLevel {
    V1,
    V2,
    V3,
    V4,
}
impl IsaLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::V1 => "x86-64-v1",
            Self::V2 => "x86-64-v2",
            Self::V3 => "x86-64-v3",
            Self::V4 => "x86-64-v4",
        }
    }
}
#[derive(Clone, Debug, Default)]
struct Capabilities {
    hardware: BTreeSet<Feature>,
    xcr0: u64,
}
impl Capabilities {
    fn has(&self, feature: Feature) -> bool {
        self.hardware.contains(&feature)
    }
    fn usable(&self, feature: Feature) -> bool {
        self.has(feature)
            && match feature {
                Feature::Avx | Feature::Avx2 | Feature::Fma | Feature::F16c => {
                    self.has(Feature::Avx)
                        && self.has(Feature::Xsave)
                        && self.has(Feature::Osxsave)
                        && self.xcr0 & AVX_STATE == AVX_STATE
                }
                Feature::Avx512f
                | Feature::Avx512bw
                | Feature::Avx512cd
                | Feature::Avx512dq
                | Feature::Avx512vl => {
                    self.usable(Feature::Avx) && self.xcr0 & AVX512_STATE == AVX512_STATE
                }
                _ => true,
            }
    }
    fn missing(&self, through: IsaLevel) -> Vec<&'static str> {
        let level = match through {
            IsaLevel::V1 => 1,
            IsaLevel::V2 => 2,
            IsaLevel::V3 => 3,
            IsaLevel::V4 => 4,
        };
        let mut missing: Vec<_> = [V1, V2, V3, V4]
            .iter()
            .take(level)
            .flat_map(|features| features.iter())
            .filter(|f| !self.has(**f))
            .map(|f| f.name())
            .collect();
        if level >= 3 && self.xcr0 & AVX_STATE != AVX_STATE {
            missing.push("OS-enabled XMM/YMM state");
        }
        if level >= 4 && self.xcr0 & AVX512_STATE != AVX512_STATE {
            missing.push("OS-enabled opmask/ZMM state");
        }
        missing
    }
    fn level(&self) -> Option<IsaLevel> {
        [IsaLevel::V4, IsaLevel::V3, IsaLevel::V2, IsaLevel::V1]
            .into_iter()
            .find(|level| self.missing(*level).is_empty())
    }
    fn intersect(&mut self, other: Self) {
        self.hardware
            .retain(|feature| other.hardware.contains(feature));
        self.xcr0 &= other.xcr0;
    }
}

#[derive(Clone, Debug, Default)]
pub struct CpuFacts {
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub logical_cpus: Option<usize>,
    pub sockets: Option<usize>,
    pub cores: Option<usize>,
    pub threads_per_core: Option<(usize, usize)>,
    pub frequency_mhz: Option<(f64, f64)>,
    pub caches: Vec<String>,
    pub schedulable_cpus: Option<usize>,
    pub isa_level: Option<IsaLevel>,
    capabilities: Option<Capabilities>,
    advertised: BTreeSet<String>,
    advertised_known: bool,
}
impl CpuFacts {
    pub fn hypervisor_present(&self) -> Option<bool> {
        self.capabilities
            .as_ref()
            .map(|caps| caps.has(Feature::Hypervisor))
            .or_else(|| {
                self.advertised_known
                    .then(|| self.advertised.contains("hypervisor"))
            })
    }
    pub fn render(&self) -> String {
        let unknown = "Unknown";
        let number = |value: Option<usize>| {
            value
                .map(|v| v.to_string())
                .unwrap_or_else(|| unknown.into())
        };
        let mut text = format!(
            "Architecture: {} (guest-visible)\nVendor: {}\nModel: {}\nLogical CPUs: {}; allowed CPUs: {}\nVisible sockets / cores: {} / {}\nThreads/core: {}\nFrequency: {}\nCaches (one visible CPU): {}\nMicroarch level: {}\n",
            std::env::consts::ARCH,
            self.vendor.as_deref().unwrap_or(unknown),
            self.model.as_deref().unwrap_or(unknown),
            number(self.logical_cpus),
            number(self.schedulable_cpus),
            number(self.sockets),
            number(self.cores),
            self.threads_per_core
                .map(|(min, max)| if min == max {
                    min.to_string()
                } else {
                    format!("{min}–{max}")
                })
                .unwrap_or_else(|| unknown.into()),
            self.frequency_mhz
                .map(|(min, max)| format!("{min:.0}–{max:.0} MHz (reported sample)"))
                .unwrap_or_else(|| "Unavailable".into()),
            if self.caches.is_empty() {
                "Unavailable".into()
            } else {
                self.caches.join("; ")
            },
            self.isa_level.map(IsaLevel::as_str).unwrap_or(unknown),
        );
        for (name, features) in [
            ("AES-NI", &[Feature::Aes][..]),
            ("AVX", &[Feature::Avx][..]),
            ("AVX2", &[Feature::Avx2][..]),
            ("FMA", &[Feature::Fma][..]),
            ("BMI1 / BMI2", &[Feature::Bmi1, Feature::Bmi2][..]),
            ("AVX-512 (v4 set)", V4),
        ] {
            let value = self
                .capabilities
                .as_ref()
                .map(|caps| {
                    if features.iter().all(|f| caps.usable(*f)) {
                        "Supported"
                    } else {
                        "Unavailable"
                    }
                })
                .unwrap_or("Unknown (OS/CPU intersection unverified)");
            text.push_str(&format!("{name}: {value}\n"));
        }
        if let Some(caps) = &self.capabilities {
            let missing = caps.missing(IsaLevel::V3);
            if !missing.is_empty() {
                text.push_str(&format!("v3 missing: {}\n", missing.join(", ")));
            }
            text.push_str(&format!(
                "Virtualization extensions: VMX={} SVM={}\n",
                if caps.usable(Feature::Vmx) {
                    "Supported"
                } else {
                    "Unavailable"
                },
                if caps.usable(Feature::Svm) {
                    "Supported"
                } else {
                    "Unavailable"
                }
            ));
        } else {
            let advertised = |flag| {
                if self.advertised_known {
                    if self.advertised.contains(flag) {
                        "Supported"
                    } else {
                        "Unavailable"
                    }
                } else {
                    "Unknown"
                }
            };
            text.push_str(&format!(
                "Advertised VMX / SVM: {} / {} (OS usability unverified)\n",
                advertised("vmx"),
                advertised("svm")
            ));
        }
        text.push_str("Capabilities describe this execution environment; hypervisors may mask features. Physical host CPU unverified.\n");
        text
    }
}

fn read(root: &Path, path: impl AsRef<Path>, bound: u64) -> Option<String> {
    let mut text = String::new();
    fs::File::open(root.join(path))
        .ok()?
        .take(bound + 1)
        .read_to_string(&mut text)
        .ok()?;
    if text.len() as u64 > bound {
        return None;
    }
    Some(
        text.chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect(),
    )
}
fn field<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    block
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.trim() == name)
        .map(|(_, value)| value.trim())
        .filter(|v| !v.is_empty())
}
fn parse_cpuinfo(cpuinfo: &str) -> CpuFacts {
    let blocks: Vec<_> = cpuinfo
        .split("\n\n")
        .filter(|b| field(b, "processor").is_some())
        .take(16385)
        .collect();
    if blocks.len() > 16384 {
        return CpuFacts::default();
    }
    let mut facts = CpuFacts::default();
    if blocks.is_empty() {
        return facts;
    }
    facts.logical_cpus = Some(blocks.len());
    let unique = |name: &str| {
        let values: BTreeSet<_> = blocks
            .iter()
            .filter_map(|b| field(b, name))
            .take(16384)
            .collect();
        (!values.is_empty()).then(|| {
            values
                .into_iter()
                .take(8)
                .map(|v| v.chars().take(256).collect::<String>())
                .collect::<Vec<_>>()
                .join("; ")
        })
    };
    facts.vendor = unique("vendor_id");
    facts.model = unique("model name");
    let topology: Option<Vec<_>> = blocks
        .iter()
        .map(|b| Some((field(b, "physical id")?, field(b, "core id")?)))
        .collect();
    if let Some(topology) = topology {
        facts.sockets = Some(
            topology
                .iter()
                .map(|(socket, _)| socket)
                .collect::<BTreeSet<_>>()
                .len(),
        );
        let mut threads = BTreeMap::new();
        for core in topology {
            *threads.entry(core).or_insert(0usize) += 1;
        }
        facts.cores = Some(threads.len());
        facts.threads_per_core = Some((
            *threads.values().min().unwrap(),
            *threads.values().max().unwrap(),
        ));
    }
    let frequencies: Vec<_> = blocks
        .iter()
        .filter_map(|b| field(b, "cpu MHz")?.parse::<f64>().ok())
        .filter(|f| f.is_finite() && *f > 0.0 && *f < 100000.0)
        .collect();
    if let Some(first) = frequencies.first() {
        facts.frequency_mhz = Some(
            frequencies
                .iter()
                .fold((*first, *first), |(min, max), value| {
                    (min.min(*value), max.max(*value))
                }),
        );
    }
    let advertised: Option<Vec<BTreeSet<String>>> = blocks
        .iter()
        .map(|b| {
            Some(
                field(b, "flags")?
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect(),
            )
        })
        .collect();
    if let Some(mut sets) = advertised
        && let Some(mut common) = sets.pop()
    {
        for set in sets {
            common.retain(|feature| set.contains(feature));
        }
        facts.advertised = common;
        facts.advertised_known = true;
    }
    facts
}

pub fn inspect(root: &Path) -> CpuFacts {
    let mut facts = parse_cpuinfo(&read(root, "proc/cpuinfo", 4 * 1024 * 1024).unwrap_or_default());
    // Index entries are a sample, never a sum over shared caches.
    if let Ok(entries) = fs::read_dir(root.join("sys/devices/system/cpu/cpu0/cache")) {
        let mut indexes: Vec<_> = entries
            .take(64)
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name().to_str().is_some_and(|name| {
                    name.strip_prefix("index")
                        .is_some_and(|n| n.parse::<u32>().is_ok())
                })
            })
            .collect();
        indexes.sort_by_key(|entry| entry.file_name());
        for entry in indexes.into_iter().take(16) {
            let relative = Path::new("sys/devices/system/cpu/cpu0/cache").join(entry.file_name());
            if let (Some(level), Some(kind), Some(size)) = (
                read(root, relative.join("level"), 256),
                read(root, relative.join("type"), 256),
                read(root, relative.join("size"), 256),
            ) {
                facts
                    .caches
                    .push(format!("L{} {} {}", level.trim(), kind.trim(), size.trim()));
            }
        }
    }
    if root == Path::new("/")
        && let Some((capabilities, count)) = all_allowed_capabilities()
    {
        facts.isa_level = capabilities.level();
        facts.capabilities = Some(capabilities);
        facts.schedulable_cpus = Some(count);
    }
    facts
}

// Register decoding is independent of the executing CPU for deterministic tests.
fn decode_cpuid(
    one_edx: u32,
    one_ecx: u32,
    seven_ebx: u32,
    ext_ecx: u32,
    ext_edx: u32,
    xcr0: u64,
) -> Capabilities {
    let mut caps = Capabilities::default();
    for (register, bits) in [
        (
            one_edx,
            &[
                (0, Feature::Fpu),
                (8, Feature::Cx8),
                (15, Feature::Cmov),
                (23, Feature::Mmx),
                (24, Feature::Fxsr),
                (25, Feature::Sse),
                (26, Feature::Sse2),
            ][..],
        ),
        (
            one_ecx,
            &[
                (0, Feature::Sse3),
                (5, Feature::Vmx),
                (9, Feature::Ssse3),
                (12, Feature::Fma),
                (13, Feature::Cx16),
                (19, Feature::Sse41),
                (20, Feature::Sse42),
                (22, Feature::Movbe),
                (23, Feature::Popcnt),
                (25, Feature::Aes),
                (26, Feature::Xsave),
                (27, Feature::Osxsave),
                (28, Feature::Avx),
                (29, Feature::F16c),
                (31, Feature::Hypervisor),
            ][..],
        ),
        (
            seven_ebx,
            &[
                (3, Feature::Bmi1),
                (5, Feature::Avx2),
                (8, Feature::Bmi2),
                (16, Feature::Avx512f),
                (17, Feature::Avx512dq),
                (28, Feature::Avx512cd),
                (30, Feature::Avx512bw),
                (31, Feature::Avx512vl),
            ][..],
        ),
        (
            ext_ecx,
            &[(0, Feature::Lahf), (2, Feature::Svm), (5, Feature::Lzcnt)][..],
        ),
        (
            ext_edx,
            &[(11, Feature::Syscall), (29, Feature::LongMode)][..],
        ),
    ] {
        for (bit, feature) in bits {
            if register & (1 << bit) != 0 {
                caps.hardware.insert(*feature);
            }
        }
    }
    // Linux x86-64 userspace ABI requires OSFXSR/XMM and EFER.SCE support.
    // Unlike OSXSAVE, neither enabled bit is readable through CPUID.
    if caps.has(Feature::Fxsr) && caps.has(Feature::Sse2) {
        caps.hardware.insert(Feature::OsFxsr);
    }
    if caps.has(Feature::Syscall) && caps.has(Feature::LongMode) {
        caps.hardware.insert(Feature::Sce);
    }
    if caps.has(Feature::Xsave) && caps.has(Feature::Osxsave) {
        caps.xcr0 = xcr0;
    }
    caps
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn current_capabilities() -> Capabilities {
    use std::arch::x86_64::{__cpuid_count, _xgetbv};
    // CPUID is mandatory in long mode. Leaves/subleaves are guarded by maxima.
    unsafe {
        let max = __cpuid_count(0, 0).eax;
        if max < 1 {
            return Capabilities::default();
        }
        let one = __cpuid_count(1, 0);
        let seven_ebx = if max >= 7 { __cpuid_count(7, 0).ebx } else { 0 };
        let (ext_ecx, ext_edx) = if __cpuid_count(0x8000_0000, 0).eax >= 0x8000_0001 {
            let ext = __cpuid_count(0x8000_0001, 0);
            (ext.ecx, ext.edx)
        } else {
            (0, 0)
        };
        // XGETBV faults unless BOTH XSAVE and CR4.OSXSAVE are advertised.
        let xcr0 = if one.ecx & ((1 << 26) | (1 << 27)) == ((1 << 26) | (1 << 27)) {
            _xgetbv(0)
        } else {
            0
        };
        decode_cpuid(one.edx, one.ecx, seven_ebx, ext_ecx, ext_edx, xcr0)
    }
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn all_allowed_capabilities() -> Option<(Capabilities, usize)> {
    // Affinity is per Linux thread. A dedicated probe thread inherits the
    // caller's permitted CPUs, and exiting it guarantees even a failed restore
    // cannot leave the caller pinned to the final sampled CPU.
    std::thread::Builder::new()
        .name("rustaller-cpu-inspect".into())
        .spawn(probe_allowed_capabilities)
        .ok()?
        .join()
        .ok()?
}

#[cfg(all(target_arch = "x86_64", target_os = "linux"))]
fn probe_allowed_capabilities() -> Option<(Capabilities, usize)> {
    use std::{
        mem::size_of,
        time::{Duration, Instant},
    };
    unsafe extern "C" {
        fn sched_getaffinity(pid: i32, size: usize, mask: *mut usize) -> i32;
        fn sched_setaffinity(pid: i32, size: usize, mask: *const usize) -> i32;
    }
    struct Restore {
        mask: Vec<usize>,
        restored: bool,
    }
    impl Restore {
        fn restore(&mut self) -> bool {
            self.restored = unsafe {
                sched_setaffinity(0, self.mask.len() * size_of::<usize>(), self.mask.as_ptr()) == 0
            };
            self.restored
        }
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            if !self.restored {
                let _ = self.restore();
            }
        }
    }
    let start = Instant::now();
    let mut words = 128 / size_of::<usize>();
    let original = loop {
        let mut mask = vec![0usize; words];
        if unsafe { sched_getaffinity(0, words * size_of::<usize>(), mask.as_mut_ptr()) } == 0 {
            break mask;
        }
        words *= 2;
        if words * size_of::<usize>() > 1024 * 1024 {
            return None;
        }
    };
    let count = original
        .iter()
        .map(|word| word.count_ones() as usize)
        .sum::<usize>();
    // Bound pathological CPU inventories and total migration latency. Unknown
    // is safer than claiming a level from an incompletely sampled inventory.
    if count == 0 || count > 4096 {
        return None;
    }
    let mut restore = Restore {
        mask: original.clone(),
        restored: false,
    };
    let mut common: Option<Capabilities> = None;
    let mut single = vec![0usize; original.len()];
    for (index, word) in original.iter().enumerate() {
        for bit in 0..usize::BITS {
            if word & (1usize << bit) == 0 {
                continue;
            }
            if start.elapsed() > Duration::from_secs(2) {
                return None;
            }
            single[index] = 1usize << bit;
            if unsafe { sched_setaffinity(0, single.len() * size_of::<usize>(), single.as_ptr()) }
                != 0
            {
                return None;
            }
            let caps = current_capabilities();
            match &mut common {
                Some(common) => common.intersect(caps),
                None => common = Some(caps),
            }
            single[index] = 0;
        }
    }
    if !restore.restore() {
        return None;
    }
    let mut actual = vec![0usize; original.len()];
    if unsafe { sched_getaffinity(0, actual.len() * size_of::<usize>(), actual.as_mut_ptr()) } != 0
        || actual != original
    {
        return None;
    }
    Some((common?, count))
}
#[cfg(not(all(target_arch = "x86_64", target_os = "linux")))]
fn all_allowed_capabilities() -> Option<(Capabilities, usize)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v4() -> Capabilities {
        Capabilities {
            hardware: [V1, V2, V3, V4].into_iter().flatten().copied().collect(),
            xcr0: AVX512_STATE,
        }
    }
    #[test]
    fn cpuid_registers_decode_complete_v3_and_v4() {
        let bits = |positions: &[u32]| positions.iter().fold(0u32, |word, bit| word | (1 << bit));
        let edx = bits(&[0, 8, 15, 23, 24, 25, 26]);
        let ecx = bits(&[0, 9, 12, 13, 19, 20, 22, 23, 26, 27, 28, 29]);
        let ebx = bits(&[3, 5, 8, 16, 17, 28, 30, 31]);
        let ext_ecx = bits(&[0, 5]);
        let ext_edx = bits(&[11, 29]);
        assert_eq!(
            decode_cpuid(edx, ecx, ebx, ext_ecx, ext_edx, AVX512_STATE).level(),
            Some(IsaLevel::V4)
        );
        for bit in [12, 22, 26, 27, 28, 29] {
            assert_eq!(
                decode_cpuid(edx, ecx & !(1 << bit), ebx, ext_ecx, ext_edx, AVX512_STATE).level(),
                Some(IsaLevel::V2)
            );
        }
        for bit in [3, 5, 8] {
            assert_eq!(
                decode_cpuid(edx, ecx, ebx & !(1 << bit), ext_ecx, ext_edx, AVX512_STATE).level(),
                Some(IsaLevel::V2)
            );
        }
        assert_eq!(
            decode_cpuid(edx, ecx, ebx, ext_ecx & !(1 << 5), ext_edx, AVX512_STATE).level(),
            Some(IsaLevel::V2)
        );
        // Hardware AVX-512 with only XMM/YMM enabled remains v3.
        assert_eq!(
            decode_cpuid(edx, ecx, ebx, ext_ecx, ext_edx, AVX_STATE).level(),
            Some(IsaLevel::V3)
        );
    }
    #[test]
    fn cumulative_levels() {
        let mut caps = v4();
        assert_eq!(caps.level(), Some(IsaLevel::V4));
        for f in V4 {
            caps.hardware.remove(f);
        }
        assert_eq!(caps.level(), Some(IsaLevel::V3));
        for f in V3 {
            caps.hardware.remove(f);
        }
        assert_eq!(caps.level(), Some(IsaLevel::V2));
        for f in V2 {
            caps.hardware.remove(f);
        }
        assert_eq!(caps.level(), Some(IsaLevel::V1));
        caps.hardware.remove(&Feature::Cx8);
        assert_eq!(caps.level(), None);
    }
    #[test]
    fn every_v1_v2_v3_requirement_is_cumulative() {
        for feature in [V1, V2, V3].into_iter().flatten() {
            let mut caps = v4();
            caps.hardware.remove(feature);
            assert!(
                !matches!(caps.level(), Some(IsaLevel::V3 | IsaLevel::V4)),
                "{} is required",
                feature.name()
            );
            assert!(caps.missing(IsaLevel::V3).contains(&feature.name()));
        }
    }
    #[test]
    fn avx2_alone_is_not_v3() {
        let caps = Capabilities {
            hardware: [Feature::Avx2].into_iter().collect(),
            xcr0: AVX512_STATE,
        };
        assert_eq!(caps.level(), None);
    }
    #[test]
    fn avx_requires_os_enabled_xmm_and_ymm() {
        for state in [0, 1 << 1, 1 << 2] {
            let mut caps = v4();
            caps.xcr0 = state;
            assert_eq!(caps.level(), Some(IsaLevel::V2));
            assert!(!caps.usable(Feature::Avx));
            assert!(!caps.usable(Feature::Avx2));
        }
        for feature in [Feature::Xsave, Feature::Osxsave] {
            let mut caps = v4();
            caps.hardware.remove(&feature);
            assert!(!caps.usable(Feature::Avx));
        }
    }
    #[test]
    fn all_v4_features_and_register_state_are_required() {
        for feature in V4 {
            let mut caps = v4();
            caps.hardware.remove(feature);
            assert_eq!(caps.level(), Some(IsaLevel::V3));
        }
        for bit in [5, 6, 7] {
            let mut caps = v4();
            caps.xcr0 &= !(1 << bit);
            assert_eq!(caps.level(), Some(IsaLevel::V3));
            assert!(!caps.usable(Feature::Avx512f));
        }
    }
    #[test]
    fn heterogeneous_or_masked_cpu_uses_common_capabilities() {
        let mut common = v4();
        let mut masked = v4();
        masked.hardware.remove(&Feature::Bmi2);
        common.intersect(masked);
        assert_eq!(common.level(), Some(IsaLevel::V2));
        let mut no_os_state = v4();
        no_os_state.xcr0 = 0;
        common.intersect(no_os_state);
        assert!(!common.usable(Feature::Avx));
    }
    #[test]
    fn topology_and_frequency_are_visible_not_model_inferences() {
        let cpu = "processor : 0\nvendor_id : GenuineIntel\nmodel name : Test\nphysical id : 0\ncore id : 0\ncpu MHz : 2400\nflags : aes vmx avx2\n\nprocessor : 1\nvendor_id : GenuineIntel\nmodel name : Test\nphysical id : 0\ncore id : 0\ncpu MHz : 1200\nflags : vmx\n\nprocessor : 2\nphysical id : 1\ncore id : 0\n";
        let facts = parse_cpuinfo(cpu);
        assert_eq!(facts.logical_cpus, Some(3));
        assert_eq!(facts.sockets, Some(2));
        assert_eq!(facts.cores, Some(2));
        assert_eq!(facts.threads_per_core, Some((1, 2)));
        assert_eq!(facts.frequency_mhz, Some((1200.0, 2400.0)));
        assert_eq!(facts.isa_level, None);
        assert!(facts.advertised.is_empty());
    }
    #[test]
    fn absent_and_malformed_fields_are_unknown() {
        for text in ["", "junk", "processor : 0\ncpu MHz : NaN\n"] {
            let facts = parse_cpuinfo(text);
            assert_eq!(facts.isa_level, None);
            assert_eq!(facts.frequency_mhz, None);
            assert_eq!(facts.cores, None);
            assert!(facts.render().contains("Unknown"));
        }
    }
    #[test]
    #[cfg(all(target_arch = "x86_64", target_os = "linux"))]
    fn native_probe_preserves_calling_thread_affinity() {
        fn affinity() -> Option<Vec<usize>> {
            unsafe extern "C" {
                fn sched_getaffinity(pid: i32, size: usize, mask: *mut usize) -> i32;
            }
            for bytes in [128, 256, 512, 1024, 2048, 4096, 8192, 16384] {
                let mut mask = vec![0usize; bytes / std::mem::size_of::<usize>()];
                if unsafe { sched_getaffinity(0, bytes, mask.as_mut_ptr()) } == 0 {
                    return Some(mask);
                }
            }
            None
        }
        // A sandbox that denies affinity queries must retain Unknown ISA.
        let before = affinity();
        let detected = all_allowed_capabilities();
        assert_eq!(before, affinity());
        if before.is_none() {
            assert!(detected.is_none());
        }
    }
}
