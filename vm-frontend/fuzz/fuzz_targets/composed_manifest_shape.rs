#![no_main]

use agentvm_composed_fs::validate_manifest_json_shape;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = validate_manifest_json_shape(text);
    }
});

