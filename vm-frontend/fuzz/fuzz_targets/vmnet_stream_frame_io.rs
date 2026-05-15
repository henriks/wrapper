#![no_main]

use std::io::Cursor;

use agentvm_frontend::vmnet_stream::{QemuFrameIo, VmnetStreamError};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut reader = QemuFrameIo::new(Cursor::new(data.to_vec()), 65_535);
    match reader.read_frame() {
        Ok(Some(frame)) => assert!(!frame.is_empty() && frame.len() <= 65_535),
        Ok(None) => {}
        Err(VmnetStreamError::InvalidFrameLength { length, max }) => {
            assert_eq!(max, 65_535);
            assert!(length == 0 || length > max);
        }
        Err(VmnetStreamError::TruncatedLength { received }) => {
            assert!((1..4).contains(&received));
        }
        Err(VmnetStreamError::TruncatedFrame) => {}
        Err(VmnetStreamError::FrameTooLarge { .. }) => {
            panic!("read_frame returned write-side FrameTooLarge error");
        }
        Err(VmnetStreamError::Io(error)) => {
            panic!("cursor-backed fuzz input returned I/O error: {error}");
        }
    }

    let write_len = data.len().min(65_535);
    if write_len > 0 {
        let mut writer = QemuFrameIo::new(Cursor::new(Vec::new()), 65_535);
        writer
            .write_frame(&data[..write_len])
            .expect("bounded non-empty frame should encode");
    }
});
