#![no_main]
use avelune::video::v1 as video;
use avelune_reference::video_decoder as reference;
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    if let Ok((id, p, deps)) = video::decode(data, &[]) {
        if let Ok((rid, r, rdeps)) = reference::decode(data, &[]) {
            assert_eq!(id, rid);
            assert_eq!(deps, rdeps);
            assert_eq!(p.y(), r.y);
            assert_eq!(p.u(), r.u);
            assert_eq!(p.v(), r.v);
        }
    }
});
