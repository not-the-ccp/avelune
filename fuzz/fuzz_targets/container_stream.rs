#![no_main]
use libfuzzer_sys::fuzz_target;
use avelune_prod::container::v1::StreamParser;
fuzz_target!(|data: &[u8]| {
    let mut p=StreamParser::default(); let mut pos=0usize; let mut step=1usize;
    while pos<data.len(){ let n=step.min(data.len()-pos); let _=p.push(&data[pos..pos+n]); pos+=n; step=(step*5+3)%97+1; }
    let _=p.finish();
});
