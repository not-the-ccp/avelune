#![no_main]
use libfuzzer_sys::fuzz_target;
use avelune::audio::v1 as prod;
use avelune_reference::audio as reference;
fuzz_target!(|data: &[u8]| {
    if let Ok((rate,ch,pcm))=prod::decode(data) {
        let (rr,rc,rpcm)=reference::decode(data).expect("reference must accept production-accepted ALA1");
        assert_eq!((rate,ch,pcm),(rr,rc,rpcm));
    }
});
