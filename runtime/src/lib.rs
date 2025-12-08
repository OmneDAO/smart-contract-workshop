//! Runtime helper library for pysub-generated WebAssembly modules.

/// Placeholder allocator symbol; implementation will be added later.
#[no_mangle]
pub extern "C" fn __alloc(_size: u32) -> u32 {
    0
}
