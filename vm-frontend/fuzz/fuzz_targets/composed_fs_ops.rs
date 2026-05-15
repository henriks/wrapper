#![no_main]

use agentvm_composed_fs::fuzz_harness::run_fs_operation_bytes;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    run_fs_operation_bytes(data);
});
