#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = agentvm_frontend::supervisor_control::decode_control_request(data);
    let _ = agentvm_frontend::supervisor_control::decode_control_response(data);
});
