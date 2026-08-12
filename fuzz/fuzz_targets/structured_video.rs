#![no_main]
use libfuzzer_sys::fuzz_target;
use avelune_prod::video::v1 as p;
use avelune_video_ref_v1 as r;
fn byte(data:&[u8],i:usize)->u8{data.get(i).copied().unwrap_or(0)}
fn shifted(src:&p::Frame420,dx:usize,dy:usize)->p::Frame420{
    fn plane(s:&[u8],w:usize,h:usize,dx:usize,dy:usize)->Vec<u8>{let mut o=vec![0;s.len()];for y in 0..h{for x in 0..w{let sx=x.saturating_sub(dx).min(w-1);let sy=y.saturating_sub(dy).min(h-1);o[y*w+x]=s[sy*w+sx];}}o}
    let w=src.width as usize;let h=src.height as usize;
    p::Frame420::from_planes(src.width,src.height,plane(src.y(),w,h,dx,dy),plane(src.u(),w/2,h/2,dx.min(1),dy.min(1)),plane(src.v(),w/2,h/2,dx.min(1),dy.min(1))).unwrap()
}
fuzz_target!(|data: &[u8]| {
    if data.len()<4{return}
    let w=2*((usize::from(byte(data,0))%32)+1); let h=2*((usize::from(byte(data,1))%24)+1);
    let q=[1u16,17,48,96,192,511][usize::from(byte(data,2))%6];
    let ylen=w*h; let clen=ylen/4; let mut x=0x9e3779b97f4a7c15u64 ^ u64::from(byte(data,3));
    let mut next=||{x^=x<<13;x^=x>>7;x^=x<<17; x as u8};
    let mut source=Vec::with_capacity(ylen+2*clen); for i in 0..source.capacity(){source.push(data.get(4+i).copied().unwrap_or_else(&mut next));}
    let f=p::Frame420::from_planes(w as u32,h as u32,source[..ylen].to_vec(),source[ylen..ylen+clen].to_vec(),source[ylen+clen..].to_vec()).unwrap();
    let opt=p::EncodeOptions{qstep:q,motion_radius:3,max_refs:4,..Default::default()};
    let mut enc=p::VideoEncoder::new(opt); let mut prod=p::VideoDecoder::new();
    let e1=enc.encode(1,&f).unwrap(); let (_,pd1,_)=prod.decode(&e1.packet).unwrap(); let (_,rd1,_)=r::decode(&e1.packet,&[]).unwrap();
    assert_eq!(pd1.y(),rd1.y);assert_eq!(pd1.u(),rd1.u);assert_eq!(pd1.v(),rd1.v); if q==1{assert_eq!(pd1,f);}
    if byte(data,3)&1!=0 {
        let f2=shifted(&f,usize::from(byte(data,3)>>1)&3,usize::from(byte(data,3)>>3)&3);
        let e2=enc.encode(2,&f2).unwrap(); let (_,pd2,_)=prod.decode(&e2.packet).unwrap();
        let refs=[(1u64,&rd1)]; let (_,rd2,_)=r::decode(&e2.packet,&refs).unwrap();
        assert_eq!(pd2.y(),rd2.y);assert_eq!(pd2.u(),rd2.u);assert_eq!(pd2.v(),rd2.v); if q==1{assert_eq!(pd2,f2);}
    }
});
