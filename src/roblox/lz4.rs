#![allow(unused_extern_crates)]

extern crate libc;
extern crate lz4_sys;

use errors::*;
use self::libc::*;

// While these are defined as c_char in the actual headers, we cheat a little.
// Hopefully this isn't an UB invitation.
extern "C" {
    fn LZ4_compressBound(size: c_int) -> c_int;
    fn LZ4_compress_default(source: *const c_uchar, dest: *mut c_uchar,
                            source_size: c_int, max_dest_size: c_int) -> c_int;
    fn LZ4_decompress_safe (source: *const c_uchar, dest: *mut c_uchar,
                            compressed_size: c_int, max_decompressed_size: c_int) -> c_int;
}

pub fn compress(data: &[u8]) -> Result<Vec<u8>> {
    let c_int_max = c_int::max_value() as usize;
    ensure!(data.len() <= c_int_max, ErrorKind::LZ4Error);
    let bound = unsafe { LZ4_compressBound(data.len() as c_int) };
    ensure!(bound > 0, ErrorKind::LZ4Error);
    unsafe {
        let mut vec = Vec::with_capacity(bound as usize);
        let compressed_len = LZ4_compress_default(data.as_ptr(), vec.as_mut_ptr(),
                                                  data.len() as c_int, bound);
        ensure!(compressed_len > 0, ErrorKind::LZ4Error);
        vec.set_len(compressed_len as usize);
        vec.shrink_to_fit();
        Ok(vec)
    }
}

pub fn decompress(data: &[u8], decompressed_len: usize) -> Result<Vec<u8>> {
    let c_int_max = c_int::max_value() as usize;
    ensure!(data.len()       <= c_int_max, ErrorKind::LZ4Error);
    ensure!(decompressed_len <= c_int_max, ErrorKind::LZ4Error);
    unsafe {
        let mut vec = Vec::with_capacity(decompressed_len);
        vec.set_len(decompressed_len);
        let decompressed = LZ4_decompress_safe(data.as_ptr(), vec.as_mut_ptr(),
                                               data.len() as c_int, decompressed_len as c_int);
        ensure!(decompressed > 0 && decompressed as usize == decompressed_len, ErrorKind::LZ4Error);
        Ok(vec)
    }
}