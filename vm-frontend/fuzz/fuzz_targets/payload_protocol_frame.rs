#![no_main]

use agentvm_payload_protocol::{
    encode_frame, FrameDecoder, FrameError, FrameKind, MAX_FRAME_PAYLOAD,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut decoder = FrameDecoder::new();
    match decoder.push(data) {
        Ok(frames) => {
            for frame in frames {
                assert!(frame.payload.len() <= MAX_FRAME_PAYLOAD);
                let encoded =
                    encode_frame(frame.kind, &frame.payload).expect("decoded frame re-encodes");
                assert_eq!(encoded[0], frame.kind.as_byte());
            }
            let _ = decoder.finish();
        }
        Err(FrameError::PayloadTooLarge { length, max }) => {
            assert_eq!(max, MAX_FRAME_PAYLOAD);
            assert!(length > max);
        }
        Err(FrameError::IncompleteFrame { .. }) => {}
    }

    let bounded_len = data.len().min(1024);
    let encoded = encode_frame(FrameKind::from_byte(0xff), &data[..bounded_len])
        .expect("bounded payload encodes");
    let mut round_trip = FrameDecoder::new();
    let frames = round_trip.push(&encoded).expect("encoded frame decodes");
    round_trip.finish().expect("encoded frame is complete");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].kind, FrameKind::from_byte(0xff));
    assert_eq!(frames[0].payload, &data[..bounded_len]);
});
