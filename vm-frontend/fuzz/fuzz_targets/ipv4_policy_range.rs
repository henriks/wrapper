#![no_main]

use agentvm_frontend::network_policy::Ipv4Range;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(input) = std::str::from_utf8(data) {
        let _ = Ipv4Range::parse(input);
    }
});
