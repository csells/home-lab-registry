use std::{error::Error, ffi::c_void, io, ptr};

use libloading::Library;
use serde_json::{Value, json};

const WORDS: usize = 256;
const PATTERN: u32 = 0x8c37_aa21;

fn status(code: i32, call: &str) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{call} returned CUDA error {code}"
        )))
    }
}

fn check() -> Result<Value, Box<dyn Error>> {
    // CUDA supplies these driver symbols. Symbol lifetimes retain the library
    // until all memory and context operations have finished.
    unsafe {
        let driver = Library::new("libcuda.so.1")?;
        let init = driver.get::<unsafe extern "C" fn(u32) -> i32>(b"cuInit")?;
        let get_device =
            driver.get::<unsafe extern "C" fn(*mut i32, i32) -> i32>(b"cuDeviceGet")?;
        let retain = driver.get::<unsafe extern "C" fn(*mut *mut c_void, i32) -> i32>(
            b"cuDevicePrimaryCtxRetain",
        )?;
        let release =
            driver.get::<unsafe extern "C" fn(i32) -> i32>(b"cuDevicePrimaryCtxRelease_v2")?;
        let current = driver.get::<unsafe extern "C" fn(*mut c_void) -> i32>(b"cuCtxSetCurrent")?;
        let allocate =
            driver.get::<unsafe extern "C" fn(*mut u64, usize) -> i32>(b"cuMemAlloc_v2")?;
        let free = driver.get::<unsafe extern "C" fn(u64) -> i32>(b"cuMemFree_v2")?;
        let fill = driver.get::<unsafe extern "C" fn(u64, u32, usize) -> i32>(b"cuMemsetD32_v2")?;
        let download = driver
            .get::<unsafe extern "C" fn(*mut c_void, u64, usize) -> i32>(b"cuMemcpyDtoH_v2")?;
        let mut device = 0;
        let mut context = ptr::null_mut();
        status(init(0), "cuInit")?;
        status(get_device(&mut device, 0), "cuDeviceGet")?;
        status(retain(&mut context, device), "cuDevicePrimaryCtxRetain")?;
        let test = (|| -> io::Result<usize> {
            status(current(context), "cuCtxSetCurrent")?;
            let mut observed = [0_u32; WORDS];
            let mut address = 0;
            let length = std::mem::size_of_val(&observed);
            status(allocate(&mut address, length), "cuMemAlloc_v2")?;
            let result = (|| {
                status(fill(address, PATTERN, WORDS), "cuMemsetD32_v2")?;
                status(
                    download(observed.as_mut_ptr().cast(), address, length),
                    "cuMemcpyDtoH_v2",
                )?;
                if observed.iter().any(|value| *value != PATTERN) {
                    return Err(io::Error::other(
                        "GPU readback differs from the value written",
                    ));
                }
                Ok(length)
            })();
            let freed = status(free(address), "cuMemFree_v2");
            result.and_then(|length| freed.map(|()| length))
        })();
        let cleared = status(current(ptr::null_mut()), "cuCtxSetCurrent");
        let released = status(release(device), "cuDevicePrimaryCtxRelease_v2");
        let bytes = test?;
        cleared?;
        released?;
        Ok(json!({"device":device,"bytes_verified":bytes,"cuda":"verified"}))
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    println!("{}", check()?);
    Ok(())
}
