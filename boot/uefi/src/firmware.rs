use alloc::{format, vec::Vec};
use core::time::Duration;
use rustaller_boot_core::{CONFIG_PATH, Config, Entry, MAX_CONFIG_BYTES};
use uefi::{
    CString16, Handle, Result, Status, boot, cstr16, entry, println,
    proto::{
        console::{
            serial::{ControlBits, Serial},
            text::Key,
        },
        device_path::{
            DevicePath,
            build::{DevicePathBuilder, media::FilePath},
        },
        loaded_image::LoadedImage,
        media::file::{Directory, File, FileAttribute, FileInfo, FileMode, RegularFile},
    },
    runtime, system,
};

#[entry]
fn main() -> Status {
    if let Err(e) = uefi::helpers::init() {
        return e.status();
    }
    println!("RUSTALLER-BOOT-UEFI-1");
    match run() {
        Ok(()) => Status::SUCCESS,
        Err(e) => {
            println!("rustaller-boot: {}", e.status());
            e.status()
        }
    }
}

fn run() -> Result<()> {
    let mut byte = [0u8; 1];
    match runtime::get_variable(
        cstr16!("SecureBoot"),
        &runtime::VariableVendor::GLOBAL_VARIABLE,
        &mut byte,
    ) {
        Ok(([0], _)) => (),
        Err(e) if e.status() == Status::NOT_FOUND => (), // Variable absent on firmware without Secure Boot.
        _ => {
            println!("Secure Boot enabled or unreadable");
            return Err(Status::SECURITY_VIOLATION.into());
        }
    }
    let parent = boot::image_handle();
    let mut root = boot::get_image_file_system(parent)?.open_volume()?;
    let config = read_config(&mut root, CONFIG_PATH).or_else(|e| {
        println!(
            "{}: {}. Opening recovery configuration.",
            CONFIG_PATH,
            e.status()
        );
        read_config(&mut root, "/rustaller/boot.toml.previous")
    })?;
    let entry = select(&config);
    println!("Booting {} ({})", entry.name, entry.kernel);
    let kernel_path = filename(&entry.kernel)?;
    let mut kernel = open(&mut root, &kernel_path)?;
    let mut header = [0u8; 4096];
    read_exact(&mut kernel, &mut header)?;
    if !rustaller_boot_core::valid_efi_stub(&header) {
        return Err(Status::UNSUPPORTED.into());
    }
    drop(kernel);
    let mut stream = Vec::new();
    for source in &entry.initrds {
        // Align the next cpio header after any early uncompressed microcode archives.
        let len = stream.len().next_multiple_of(4);
        stream.resize(len, 0);
        append_file(&mut root, source, &mut stream, 512 * 1024 * 1024)?;
    }
    drop(root);
    let options = CString16::try_from(
        config
            .cmdline()
            .map_err(|_| Status::INVALID_PARAMETER)?
            .as_str(),
    )
    .map_err(|_| Status::INVALID_PARAMETER)?;
    crate::initrd::with_provider(stream, || {
        let child = load_kernel(parent, &kernel_path)?;
        let result = (|| {
            let mut image = boot::open_protocol_exclusive::<LoadedImage>(child)?;
            // The NUL-terminated UTF-16 buffer lives until StartImage returns.
            unsafe {
                image.set_load_options(options.as_ptr().cast(), options.num_bytes() as u32);
            }
            drop(image);
            boot::start_image(child)
        })();
        // StartImage normally unloads a returning application. Clear any
        // remaining handle before its borrowed load options go out of scope.
        if let Ok(mut image) = boot::open_protocol_exclusive::<LoadedImage>(child) {
            unsafe {
                image.set_load_options(core::ptr::null(), 0);
            }
            drop(image);
            boot::unload_image(child)?;
        }
        result
    })
}

fn read_config(root: &mut Directory, path: &str) -> Result<Config> {
    let mut text = Vec::new();
    append_file(root, path, &mut text, MAX_CONFIG_BYTES)?;
    Config::parse(core::str::from_utf8(&text).map_err(|_| Status::INVALID_PARAMETER)?).map_err(
        |e| {
            println!("{}", e);
            Status::INVALID_PARAMETER.into()
        },
    )
}

fn load_kernel(parent: Handle, filename: &CString16) -> Result<Handle> {
    let device = boot::open_protocol_exclusive::<LoadedImage>(parent)?
        .device()
        .ok_or(Status::NOT_FOUND)?;
    let prefix = boot::open_protocol_exclusive::<DevicePath>(device)?;
    let mut bytes = Vec::new();
    let mut builder = DevicePathBuilder::with_vec(&mut bytes);
    for node in prefix.node_iter() {
        builder = builder.push(&node).map_err(|_| Status::BAD_BUFFER_SIZE)?;
    }
    let path = builder
        .push(&FilePath {
            path_name: filename,
        })
        .and_then(|b| b.finalize())
        .map_err(|_| Status::BAD_BUFFER_SIZE)?;
    drop(prefix);
    boot::load_image(
        parent,
        boot::LoadImageSource::FromDevicePath {
            device_path: path,
            boot_policy: uefi::proto::BootPolicy::ExactMatch,
        },
    )
}

fn filename(path: &str) -> Result<CString16> {
    CString16::try_from(path.replace('/', "\\").as_str())
        .map_err(|_| Status::INVALID_PARAMETER.into())
}
fn open(root: &mut Directory, path: &CString16) -> Result<RegularFile> {
    root.open(path, FileMode::Read, FileAttribute::empty())?
        .into_regular_file()
        .ok_or_else(|| Status::INVALID_PARAMETER.into())
}
fn read_exact(file: &mut RegularFile, mut buf: &mut [u8]) -> Result<()> {
    while !buf.is_empty() {
        let count = buf.len().min(1024 * 1024);
        let n = file.read(&mut buf[..count])?;
        if n == 0 || n > count {
            return Err(Status::DEVICE_ERROR.into());
        }
        buf = &mut buf[n..];
    }
    Ok(())
}
fn append_file(root: &mut Directory, path: &str, out: &mut Vec<u8>, limit: usize) -> Result<()> {
    let mut file = open(root, &filename(path)?)?;
    let size = file.get_boxed_info::<FileInfo>()?.file_size();
    let start = out.len();
    if size == 0 || size > (limit - start) as u64 {
        return Err(Status::BAD_BUFFER_SIZE.into());
    }
    out.try_reserve_exact(size as usize)
        .map_err(|_| Status::OUT_OF_RESOURCES)?;
    out.resize(start + size as usize, 0);
    read_exact(&mut file, &mut out[start..])
}

fn select(cfg: &Config) -> &Entry {
    let mut serial = cfg
        .boot
        .serial
        .then(|| {
            let handle = boot::get_handle_for_protocol::<Serial>().ok()?;
            // A firmware console splitter can own the port. In that case ConIn/Out
            // already provides its configured redirection.
            let mut serial = boot::open_protocol_exclusive::<Serial>(handle).ok()?;
            let mut mode = *serial.io_mode();
            mode.baud_rate = cfg.boot.serial_baudrate.into();
            mode.timeout = 1000;
            serial.set_attributes(&mode).ok()?;
            Some(serial)
        })
        .flatten();
    let keys = b"123456789abcdefg";
    for (entry, key) in cfg.entries.iter().zip(keys) {
        let line = format!(
            "[{}] {} {}{}\r\n",
            *key as char,
            entry.name,
            entry.version.as_deref().unwrap_or(""),
            if entry.name == cfg.boot.default {
                " (default)"
            } else {
                ""
            }
        );
        uefi::print!("{}", line);
        if let Some(port) = serial.as_mut() {
            let _ = port.write(line.as_bytes());
        }
    }
    for _ in 0..u32::from(cfg.boot.timeout_seconds) * 20 {
        let mut key = system::with_stdin(|s| match s.read_key() {
            Ok(Some(Key::Printable(c))) => u8::try_from(u16::from(c)).ok(),
            _ => None,
        });
        if key.is_none()
            && let Some(port) = serial.as_mut()
            && port
                .get_control_bits()
                .is_ok_and(|b| !b.contains(ControlBits::INPUT_BUFFER_EMPTY))
        {
            let mut byte = [0];
            if port.read(&mut byte).is_ok() {
                key = Some(byte[0]);
            }
        }
        if let Some(key) = key {
            if key == 13 {
                break;
            }
            if let Some(entry) = keys
                .iter()
                .position(|b| *b == key)
                .and_then(|i| cfg.entries.get(i))
            {
                return entry;
            }
        }
        boot::stall(Duration::from_millis(50));
    }
    cfg.default_entry()
}
