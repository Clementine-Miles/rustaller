//! Linux EFI stub v1+ asks for one initrd via a vendor media device path.
use alloc::vec::Vec;
use core::{ffi::c_void, ptr};
use uefi::{
    Handle, Identify, Result, Status, boot, guid,
    proto::device_path::{
        DevicePath,
        build::{DevicePathBuilder, media::Vendor},
    },
};
use uefi_raw::{
    Boolean,
    protocol::{device_path::DevicePathProtocol, media::LoadFile2Protocol},
};

#[repr(C)]
struct Provider {
    interface: LoadFile2Protocol,
    bytes: Vec<u8>,
}

unsafe extern "efiapi" fn load(
    this: *mut LoadFile2Protocol,
    path: *const DevicePathProtocol,
    policy: Boolean,
    size: *mut usize,
    buffer: *mut c_void,
) -> Status {
    if this.is_null() || path.is_null() || size.is_null() {
        return Status::INVALID_PARAMETER;
    }
    if bool::from(policy) {
        return Status::UNSUPPORTED;
    }
    // Firmware passes the remaining device path after consuming the vendor node.
    // Its first four bytes are a packed EFI_DEVICE_PATH_PROTOCOL header.
    if unsafe { ptr::read_unaligned(path.cast::<[u8; 4]>()) } != [0x7f, 0xff, 4, 0] {
        return Status::NOT_FOUND;
    }
    // interface is the first field of our live repr(C) Provider. The caller
    // owns a buffer of the advertised size, separate from our immutable stream.
    let bytes = unsafe { &(*this.cast::<Provider>()).bytes };
    let offered = unsafe { size.read() };
    unsafe {
        size.write(bytes.len());
    }
    if buffer.is_null() || offered < bytes.len() {
        return Status::BUFFER_TOO_SMALL;
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), buffer.cast(), bytes.len());
    }
    Status::SUCCESS
}

pub fn with_provider(bytes: Vec<u8>, start: impl FnOnce() -> Result<()>) -> Result<()> {
    let provider = Provider {
        interface: LoadFile2Protocol { load_file: load },
        bytes,
    };
    let mut storage = Vec::new();
    let path = DevicePathBuilder::with_vec(&mut storage)
        .push(&Vendor {
            vendor_guid: guid!("5568e427-68fc-4f3d-ac74-ca555231cc68"),
            vendor_defined_data: &[],
        })
        .and_then(|b| b.finalize())
        .map_err(|_| Status::BAD_BUFFER_SIZE)?;
    let dp = path.as_ffi_ptr().cast();
    let interface = ptr::addr_of!(provider.interface).cast();
    // Both protocol objects and this image's callback code remain resident
    // across StartImage. Unregister in reverse order before returning.
    let handle = unsafe { boot::install_protocol_interface(None, &DevicePath::GUID, dp) }?;
    let installed = unsafe {
        boot::install_protocol_interface(Some(handle), &LoadFile2Protocol::GUID, interface)
    };
    let result = match installed {
        Ok(_) => {
            let result = start();
            unregister(handle, &LoadFile2Protocol::GUID, interface);
            result
        }
        Err(e) => Err(e),
    };
    unregister(handle, &DevicePath::GUID, dp);
    result
}
fn unregister(handle: Handle, guid: &uefi::Guid, interface: *const c_void) {
    // A still-installed interface may outlive this EFI application. In that
    // exceptional firmware failure, do not return and leave dangling pointers.
    if unsafe { boot::uninstall_protocol_interface(handle, guid, interface) }.is_err() {
        uefi::println!("Unable to release initrd protocol");
        loop {
            boot::stall(core::time::Duration::from_secs(1));
        }
    }
}
