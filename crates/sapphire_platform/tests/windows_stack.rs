//! The stack a Windows program's main thread gets, which `.cargo/config.toml`'s `/STACK` sets. The
//! default 1 MiB overflows when the main thread helps build the anime character shader's pipeline, so
//! a `RUSTFLAGS` that silently replaced that setting would bring back a crash seen one launch in about
//! a hundred. Test programs link with the same flags as the app.
#![cfg(windows)]

const MAIN_THREAD_STACK: u64 = 8 * 1024 * 1024;

#[test]
fn a_program_here_reserves_the_main_threads_stack() {
    let program = std::env::current_exe().expect("this program's path");
    let image = std::fs::read(program).expect("this program reads");
    let at = u32::from_le_bytes(image[0x3c..0x40].try_into().expect("four bytes")) as usize;
    assert_eq!(&image[at..at + 4], b"PE\0\0");
    // After the signature and the 20-byte file header: PE32+'s magic, and its stack reserve at 72.
    let optional = at + 4 + 20;
    assert_eq!(u16::from_le_bytes([image[optional], image[optional + 1]]), 0x20b, "a 64-bit image");
    let bytes = image[optional + 72..optional + 80].try_into().expect("eight bytes");
    assert_eq!(u64::from_le_bytes(bytes), MAIN_THREAD_STACK);
}
