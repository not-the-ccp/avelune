#![no_main]
use avelune::audio::v1 as audio;
use avelune_reference::audio as reference;
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    if let Ok((rate, ch, pcm)) = audio::decode(data) {
        let (rr, rc, rpcm) =
            reference::decode(data).expect("reference must accept canonical-accepted ALA1");
        assert_eq!((rate, ch, pcm), (rr, rc, rpcm));
    }
});
