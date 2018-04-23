#![allow(unused_extern_crates)]

extern crate libc;
extern crate lz4_sys;

use self::libc::*;

use errors::*;

extern "C" {
    fn LZ4_compressBound(size: c_int) -> c_int;
    fn LZ4_compress_default(source: *const c_char, dest: *mut c_char,
                            source_size: c_int, max_dest_size: c_int) -> c_int;
    fn LZ4_decompress_safe (source: *const c_char, dest: *mut c_char,
                            compressed_size: c_int, max_decompressed_size: c_int) -> c_int;
}

crate fn compress(data: &[u8]) -> Result<Vec<u8>> {
    let c_int_max = c_int::max_value() as usize;
    ensure!(data.len() <= c_int_max);
    let bound = unsafe { LZ4_compressBound(data.len() as c_int) };
    ensure!(bound > 0);
    unsafe {
        let mut vec = Vec::with_capacity(bound as usize);
        let compressed_len = LZ4_compress_default(data.as_ptr() as *const c_char,
                                                  vec.as_mut_ptr() as *mut c_char,
                                                  data.len() as c_int, bound);
        ensure!(compressed_len > 0);
        vec.set_len(compressed_len as usize);
        vec.shrink_to_fit();
        Ok(vec)
    }
}

crate fn decompress(data: &[u8], decompressed_len: usize) -> Result<Vec<u8>> {
    let c_int_max = c_int::max_value() as usize;
    ensure!(data.len()       <= c_int_max);
    ensure!(decompressed_len <= c_int_max);
    unsafe {
        let mut vec = Vec::with_capacity(decompressed_len);
        vec.set_len(decompressed_len);
        let decompressed = LZ4_decompress_safe(data.as_ptr() as *const c_char,
                                               vec.as_mut_ptr() as *mut c_char,
                                               data.len() as c_int, decompressed_len as c_int);
        ensure!(decompressed > 0 && decompressed as usize == decompressed_len);
        Ok(vec)
    }
}