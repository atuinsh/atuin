#![allow(dead_code)]

#[macro_use]
extern crate nom;

use nom::{
  error::ErrorKind,
  number::streaming::{be_f32, be_u16, be_u32, be_u64},
  Err, IResult, Needed,
};

use std::str;

fn mp4_box(input: &[u8]) -> IResult<&[u8], &[u8]> {
  match be_u32(input) {
    Ok((i, offset)) => {
      let sz: usize = offset as usize;
      if i.len() >= sz - 4 {
        Ok((&i[(sz - 4)..], &i[0..(sz - 4)]))
      } else {
        Err(Err::Incomplete(Needed::new(offset as usize + 4)))
      }
    }
    Err(e) => Err(e),
  }
}

#[cfg_attr(rustfmt, rustfmt_skip)]
#[derive(PartialEq,Eq,Debug)]
struct FileType<'a> {
  major_brand:         &'a str,
  major_brand_version: &'a [u8],
  compatible_brands:   Vec<&'a str>
}

#[cfg_attr(rustfmt, rustfmt_skip)]
#[allow(non_snake_case)]
#[derive(Debug,Clone)]
pub struct Mvhd32 {
  version_flags: u32, // actually:
  // version: u8,
  // flags: u24       // 3 bytes
  created_date:  u32,
  modified_date: u32,
  scale:         u32,
  duration:      u32,
  speed:         f32,
  volume:        u16, // actually a 2 bytes decimal
  /* 10 bytes reserved */
  scaleA:        f32,
  rotateB:       f32,
  angleU:        f32,
  rotateC:       f32,
  scaleD:        f32,
  angleV:        f32,
  positionX:     f32,
  positionY:     f32,
  scaleW:        f32,
  preview:       u64,
  poster:        u32,
  selection:     u64,
  current_time:  u32,
  track_id:      u32
}

#[cfg_attr(rustfmt, rustfmt_skip)]
#[allow(non_snake_case)]
#[derive(Debug,Clone)]
pub struct Mvhd64 {
  version_flags: u32, // actually:
  // version: u8,
  // flags: u24       // 3 bytes
  created_date:  u64,
  modified_date: u64,
  scale:         u32,
  duration:      u64,
  speed:         f32,
  volume:        u16, // actually a 2 bytes decimal
  /* 10 bytes reserved */
  scaleA:        f32,
  rotateB:       f32,
  angleU:        f32,
  rotateC:       f32,
  scaleD:        f32,
  angleV:        f32,
  positionX:     f32,
  positionY:     f32,
  scaleW:        f32,
  preview:       u64,
  poster:        u32,
  selection:     u64,
  current_time:  u32,
  track_id:      u32
}

#[allow(non_snake_case)]
named!(mvhd32 <&[u8], MvhdBox>,
  do_parse!(
  version_flags: be_u32 >>
  created_date:  be_u32 >>
  modified_date: be_u32 >>
  scale:         be_u32 >>
  duration:      be_u32 >>
  speed:         be_f32 >>
  volume:        be_u16 >> // actually a 2 bytes decimal
              take!(10) >>
  scale_a:       be_f32 >>
  rotate_b:      be_f32 >>
  angle_u:       be_f32 >>
  rotate_c:      be_f32 >>
  scale_d:       be_f32 >>
  angle_v:       be_f32 >>
  position_x:    be_f32 >>
  position_y:    be_f32 >>
  scale_w:       be_f32 >>
  preview:       be_u64 >>
  poster:        be_u32 >>
  selection:     be_u64 >>
  current_time:  be_u32 >>
  track_id:      be_u32 >>
  (
    MvhdBox::M32(Mvhd32 {
      version_flags: version_flags,
      created_date:  created_date,
      modified_date: modified_date,
      scale:         scale,
      duration:      duration,
      speed:         speed,
      volume:        volume,
      scaleA:        scale_a,
      rotateB:       rotate_b,
      angleU:        angle_u,
      rotateC:       rotate_c,
      scaleD:        scale_d,
      angleV:        angle_v,
      positionX:     position_x,
      positionY:     position_y,
      scaleW:        scale_w,
      preview:       preview,
      poster:        poster,
      selection:     selection,
      current_time:  current_time,
      track_id:      track_id
    })
  ))
);

#[allow(non_snake_case)]
named!(mvhd64 <&[u8], MvhdBox>,
  do_parse!(
  version_flags: be_u32 >>
  created_date:  be_u64 >>
  modified_date: be_u64 >>
  scale:         be_u32 >>
  duration:      be_u64 >>
  speed:         be_f32 >>
  volume:        be_u16 >> // actually a 2 bytes decimal
              take!(10) >>
  scale_a:       be_f32 >>
  rotate_b:      be_f32 >>
  angle_u:       be_f32 >>
  rotate_c:      be_f32 >>
  scale_d:       be_f32 >>
  angle_v:       be_f32 >>
  position_x:    be_f32 >>
  position_y:    be_f32 >>
  scale_w:       be_f32 >>
  preview:       be_u64 >>
  poster:        be_u32 >>
  selection:     be_u64 >>
  current_time:  be_u32 >>
  track_id:      be_u32 >>
  (
    MvhdBox::M64(Mvhd64 {
      version_flags: version_flags,
      created_date:  created_date,
      modified_date: modified_date,
      scale:         scale,
      duration:      duration,
      speed:         speed,
      volume:        volume,
      scaleA:        scale_a,
      rotateB:       rotate_b,
      angleU:        angle_u,
      rotateC:       rotate_c,
      scaleD:        scale_d,
      angleV:        angle_v,
      positionX:     position_x,
      positionY:     position_y,
      scaleW:        scale_w,
      preview:       preview,
      poster:        poster,
      selection:     selection,
      current_time:  current_time,
      track_id:      track_id
    })
  ))
);

#[derive(Debug, Clone)]
pub enum MvhdBox {
  M32(Mvhd32),
  M64(Mvhd64),
}

#[derive(Debug, Clone)]
pub enum MoovBox {
  Mdra,
  Dref,
  Cmov,
  Rmra,
  Iods,
  Mvhd(MvhdBox),
  Clip,
  Trak,
  Udta,
}

#[derive(Debug)]
enum MP4BoxType {
  Ftyp,
  Moov,
  Mdat,
  Free,
  Skip,
  Wide,
  Mdra,
  Dref,
  Cmov,
  Rmra,
  Iods,
  Mvhd,
  Clip,
  Trak,
  Udta,
  Unknown,
}

#[derive(Debug)]
struct MP4BoxHeader {
  length: u32,
  tag: MP4BoxType,
}

named!(brand_name<&[u8],&str>, map_res!(take!(4), str::from_utf8));

named!(filetype_parser<&[u8], FileType>,
  do_parse!(
    m: brand_name          >>
    v: take!(4)            >>
    c: many0!(brand_name)  >>
    (FileType{ major_brand: m, major_brand_version:v, compatible_brands: c })
  )
);

fn mvhd_box(input: &[u8]) -> IResult<&[u8], MvhdBox> {
  let res = if input.len() < 100 {
    Err(Err::Incomplete(Needed::new(100)))
  } else if input.len() == 100 {
    mvhd32(input)
  } else if input.len() == 112 {
    mvhd64(input)
  } else {
    Err(Err::Error(error_position!(input, ErrorKind::TooLarge)))
  };
  println!("res: {:?}", res);
  res
}

fn unknown_box_type(input: &[u8]) -> IResult<&[u8], MP4BoxType> {
  Ok((input, MP4BoxType::Unknown))
}

//named!(box_type<&[u8], MP4BoxType>,
fn box_type(input: &[u8]) -> IResult<&[u8], MP4BoxType> {
  alt!(input,
    tag!("ftyp") => { |_| MP4BoxType::Ftyp } |
    tag!("moov") => { |_| MP4BoxType::Moov } |
    tag!("mdat") => { |_| MP4BoxType::Mdat } |
    tag!("free") => { |_| MP4BoxType::Free } |
    tag!("skip") => { |_| MP4BoxType::Skip } |
    tag!("wide") => { |_| MP4BoxType::Wide } |
    unknown_box_type
  )
}

// warning, an alt combinator with 9 branches containing a tag combinator
// can make the compilation very slow. Use functions as sub parsers,
// or split into multiple alt! parsers if it gets slow
named!(moov_type<&[u8], MP4BoxType>,
  alt!(
    tag!("mdra") => { |_| MP4BoxType::Mdra } |
    tag!("dref") => { |_| MP4BoxType::Dref } |
    tag!("cmov") => { |_| MP4BoxType::Cmov } |
    tag!("rmra") => { |_| MP4BoxType::Rmra } |
    tag!("iods") => { |_| MP4BoxType::Iods } |
    tag!("mvhd") => { |_| MP4BoxType::Mvhd } |
    tag!("clip") => { |_| MP4BoxType::Clip } |
    tag!("trak") => { |_| MP4BoxType::Trak } |
    tag!("udta") => { |_| MP4BoxType::Udta }
  )
);

named!(box_header<&[u8],MP4BoxHeader>,
  do_parse!(
    length: be_u32 >>
    tag: box_type  >>
    (MP4BoxHeader{ length: length, tag: tag})
  )
);

named!(moov_header<&[u8],MP4BoxHeader>,
  do_parse!(
    length: be_u32 >>
    tag: moov_type >>
    (MP4BoxHeader{ length: length, tag: tag})
  )
);
