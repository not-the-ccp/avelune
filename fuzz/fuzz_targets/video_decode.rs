#![no_main]
use libfuzzer_sys::fuzz_target;
use avelune::video::v1 as prod;
use avelune_reference::video_decoder as reference;
fuzz_target!(|data: &[u8]| {
    if let Ok((id,p,deps))=prod::decode(data,&[]) {
        if let Ok((rid,r,rdeps))=reference::decode(data,&[]) {
            assert_eq!(id,rid); assert_eq!(deps,rdeps);
            assert_eq!(p.y(),r.y); assert_eq!(p.u(),r.u); assert_eq!(p.v(),r.v);
        }
    }
});
