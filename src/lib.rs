//! 二进制对象表示法 模块

//! Binary Object Notation
//!
//! 本模块定义了一个使用二进制表示数据的协议，并实现该协议
//!
//! 模块提供写的接口，将数据按照协议序列化为二进制。
//!
//! 提供读的接口，将满足协议的二进制反序列化为数据。
//!
//! TODO 为本协议实现Serializer、Deserializer
//!
//! 协议内容：
//!
//! 小端-非网络字节序，和quic一致
//! 用于通讯的类型需要压缩表示，充分利用第一个字节
//! 0=null
//! 1=false
//! 2=true
//! 3=浮点数0.0，4=浮点数1.0，5=16位浮点数，6=32位浮点数，7=64位浮点数，8=128位浮点数;
//! 9=8位负整数，10=16位负整数，11=32位负整数，12=48位负整数，13=64位负整数，14=128位负整数
//! 15~35= -1~19
//! 36=8位正整数，37=16位正整数，38=32位正整数，39=48位正整数，40=64位正整数，41=128位正整数

//! 42-106=0-64长度的UTF8字符串，
//! 107=8位长度的UTF8字符串，108=16位长度的UTF8字符串，109=32位长度的UTF8字符串，110=48位长度的UTF8字符串

//! 111-175=0-64长度的二进制数据，
//! 176=8位长度的二进制数据，177=16位长度的二进制数据，178=32位长度的二进制数据，179=48位长度的二进制数据

//! 180-244=0-64长度的容器，包括对象、数组和map、枚举
//! 245=8位长度的容器，246=16位长度的容器，247=32位长度的容器，248=48位长度的容器
//! 之后的一个4字节的整数表示类型。
//! 类型：
//! 	0 表示忽略
//! 	1 通用对象
//! 	2 通用数组
//! 	3 通用map

//! 如果是通用对象、数组、map，后面会有一个动态长度的整数，表示元素的数量。

//! 容器，由于有总大小的描述，从而可以只对感兴趣的部分作反序列化
//! TODO 定义一个全类型的枚举 enum BonType<T>， ReadNext WriteNext 的 T 应该为BonType。提供一个 read(&self) -> BonType<T>

#![allow(warnings)]
#![feature(exclusive_range_pattern)]
#![feature(test)]
#[warn(unconditional_recursion)]
// extern crate pi_data_view;
use std::cmp::{Eq, Ord, Ordering, PartialEq, PartialOrd};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::hash::Hash;
use std::marker::Sized;
use std::ops::Deref;
use std::ops::Range;
use std::sync::Arc;

use bytes::{Buf, BufMut, Bytes};
use num_bigint::{BigInt, Sign};

/// ReadBuffer，用于将二进制反序列化为对应数据
#[derive(Default, Clone, Debug)]
pub struct ReadBuffer<'a> {
    // u8数组
    pub bytes: &'a [u8],
    // 头部指针
    pub head: usize,
}

/// 定义读时的错误
#[derive(Clone, Debug)]
pub enum ReadBonErr {
    Overflow {
        try_index: usize,
        len: usize,
    },
    TypeNoMatch {
        try_read: String,
        act_type: (String, u8),
        head: usize,
    },
    Other(String),
    IsContainer(u8),
}

impl fmt::Display for ReadBonErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadBonErr::Overflow { try_index, len } => write!(
                f,
                "ReadBonError overflow try_index = {:?}, len = {:?}",
                try_index, len
            ),
            ReadBonErr::TypeNoMatch {
                try_read,
                act_type,
                head,
            } => write!(
                f,
                "ReadBonError TypeNoMatch try_read = {:?}, act_type = {:?}, head = {:?}",
                try_read, act_type, head
            ),
            ReadBonErr::Other(s) => write!(f, "ReadBonError Other other = {:?}", s),
            ReadBonErr::IsContainer(_) => write!(f, "IsContainer!"),
        }
    }
}

impl Error for ReadBonErr {}

impl ReadBonErr {
    #[inline]
    fn overflow(try_index: usize, len: usize) -> ReadBonErr {
        ReadBonErr::Overflow {
            try_index: try_index,
            len: len,
        }
    }

    fn type_no_match(try_read: String, type_code: u8, head: usize) -> ReadBonErr {
        let t = match type_code {
            0 => "null".to_string(),
            1 => "false".to_string(),
            2 => "true".to_string(),
            3 => "0.0".to_string(),
            4 => "1.0".to_string(),
            5..9 => "float".to_string(),
            9..15 => "int".to_string(),
            15 => "-1".to_string(),
            16..36 => (type_code - 16).to_string(),
            36..42 => "uint".to_string(),
            42..111 => "string".to_string(),
            111..180 => "bin".to_string(),
            180..249 => "container".to_string(),
            _ => "invalid type".to_string(),
        };

        ReadBonErr::TypeNoMatch {
            try_read: try_read,
            act_type: (t, type_code),
            head: head,
        }
    }

    fn other(message: String) -> ReadBonErr {
        ReadBonErr::Other(message)
    }
}

impl<'a> PartialOrd for ReadBuffer<'a> {
    fn partial_cmp(&self, other: &ReadBuffer<'a>) -> Option<Ordering> {
        let mut b1 = ReadBuffer::new(self.bytes, 0);
        let mut b2 = ReadBuffer::new(other.bytes, 0);
        let b1_type = b1.get_type_chunk().unwrap();
        let b2_type = b2.get_type_chunk().unwrap();

        let is_b1_container = b1_type >= 180 && b1_type < 249;
        let is_b2_container = b2_type >= 180 && b2_type < 249;

        if is_b1_container && is_b2_container {
            match b1_type {
                180..246 => b1.head += 1 + 1 + 4, // 1字节类型 + "可变长度"占用的字节 + 4字节哈希
                246 => b1.head += 1 + 2 + 4,
                247 => b1.head += 1 + 4 + 4,
                248 => b1.head += 1 + 6 + 4,
                _ => {
                    log::warn!("pi_bon unknown container type {:?}", b1_type);
                    return None;
                }
            }
            b1.bytes.advance(b1.head);
            match b2_type {
                180..246 => b2.head += 1 + 1 + 4,
                246 => b2.head += 1 + 2 + 4,
                247 => b2.head += 1 + 4 + 4,
                248 => b2.head += 1 + 6 + 4,
                _ => {
                    log::warn!("pi_bon unknown container type {:?}", b2_type);
                    return None;
                }
            }
            b2.bytes.advance(b2.head);
        }

        loop {
            match partial_cmp(&mut b1, &mut b2) {
                Ok(None) => return None,
                Ok(Some(Ordering::Equal)) => {
                    if b1.len() == 0 {
                        return Some(Ordering::Equal);
                    }
                }
                Ok(Some(Ordering::Greater)) => return Some(Ordering::Greater),
                Ok(Some(Ordering::Less)) => return Some(Ordering::Less),
                Err(e) => {
                    log::warn!("pi_bon partial_cmp error {:?}", e);
                    return None;
                }
            }
        }
    }
}

impl<'a> PartialEq for ReadBuffer<'a> {
    fn eq(&self, other: &ReadBuffer<'a>) -> bool {
        match self.partial_cmp(other) {
            Some(Ordering::Equal) => return true,
            _ => return false,
        };
    }
}

impl<'a> Eq for ReadBuffer<'a> {}

impl<'a> Ord for ReadBuffer<'a> {
    fn cmp(&self, other: &ReadBuffer<'a>) -> Ordering {
        match self.partial_cmp(other) {
            Some(v) => v,
            None => panic!("partial_cmp fail self:{:?}, other:{:?}", self, other),
        }
    }
}

impl<'a> ReadBuffer<'a> {
    /// 创建ReadBuffer, buf必须符合bon协议， 否则当调用其partial_cmp会直接panic
    /// head指定反序列化开始的位置
    pub fn new(buf: &[u8], head: usize) -> ReadBuffer {
        ReadBuffer {
            bytes: buf,
            head: head,
        }
    }

    /// 获取ReadBuffer当前的读指针（读是从头部开始，即下一次调用read方法，将从self.head位置向后反序列化）
    #[inline]
    pub fn head(&self) -> usize {
        self.head
    }

    /// 二进制的长度
    #[inline]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// 获取接下来要反序列化的数据的类型
    #[inline]
    pub fn get_type(&mut self) -> Result<u8, ReadBonErr> {
        self.probe_border(1)?;
        Ok(self.bytes.get_u8())
    }

    /// 获取接下来要反序列化的数据的类型(不改变bytes偏移)
    #[inline]
    pub fn get_type_chunk(&mut self) -> Result<u8, ReadBonErr> {
        self.probe_border(1)?;
        Ok(self.bytes.chunk()[0])
    }

    /// 读一个布尔类型，如果二进制当前的值不是布尔类型，返回Err
    #[inline]
    pub fn read_bool(&mut self) -> Result<bool, ReadBonErr> {
        self.probe_border(1)?;
        let t = self.bytes.get_u8();
        self.head += 1;
        match t {
            1 => Ok(false),
            2 => Ok(true),
            _ => Err(ReadBonErr::type_no_match(
                "bool".to_string(),
                t,
                self.head - 1,
            )),
        }
    }

    /// 读一个u8类型，如果二进制当前的值不是u8类型，返回Err
    #[inline]
    pub fn read_u8(&mut self) -> Result<u8, ReadBonErr> {
        let r = self.read_integer::<u32>()?;
        Ok(r as u8)
    }

    /// 读一个u16类型，如果二进制当前的值不是u16类型，返回Err
    #[inline]
    pub fn read_u16(&mut self) -> Result<u16, ReadBonErr> {
        let r = self.read_integer::<u32>()?;
        Ok(r as u16)
    }

    /// 读一个u32类型，如果二进制当前的值不是u32类型，返回Err
    #[inline]
    pub fn read_u32(&mut self) -> Result<u32, ReadBonErr> {
        self.read_integer::<u32>()
    }

    /// 读一个u64类型，如果二进制当前的值不是u64类型，返回Err
    #[inline]
    pub fn read_u64(&mut self) -> Result<u64, ReadBonErr> {
        self.read_integer::<u64>()
    }

    /// 读一个usize类型，如果二进制当前的值不是usize类型，返回Err
    #[inline]
    pub fn read_usize(&mut self) -> Result<usize, ReadBonErr> {
        let r = self.read_integer::<u64>()?;
        Ok(r as usize)
    }

    /// 读一个u128类型，如果二进制当前的值不是u128类型，返回Err
    #[inline]
    pub fn read_u128(&mut self) -> Result<u128, ReadBonErr> {
        self.read_integer::<u128>()
    }

    /// 读一个i8类型，如果二进制当前的值不是i8类型，返回Err
    #[inline]
    pub fn read_i8(&mut self) -> Result<i8, ReadBonErr> {
        let r = self.read_integer::<i32>()?;
        Ok(r as i8)
    }

    /// 读一个i16类型，如果二进制当前的值不是i16类型，返回Err
    #[inline]
    pub fn read_i16(&mut self) -> Result<i16, ReadBonErr> {
        let r = self.read_integer::<i32>()?;
        Ok(r as i16)
    }

    /// 读一个ui32类型，如果二进制当前的值不是i32类型，返回Err
    #[inline]
    pub fn read_i32(&mut self) -> Result<i32, ReadBonErr> {
        self.read_integer::<i32>()
    }

    /// 读一个i61类型，如果二进制当前的值不是i64类型，返回Err
    #[inline]
    pub fn read_i64(&mut self) -> Result<i64, ReadBonErr> {
        self.read_integer::<i64>()
    }

    /// 读一个isize类型，如果二进制当前的值不是isize类型，返回Err
    #[inline]
    pub fn read_isize(&mut self) -> Result<isize, ReadBonErr> {
        let r = self.read_integer::<i64>()?;
        Ok(r as isize)
    }

    /// 读一个i128类型，如果二进制当前的值不是i128类型，返回Err
    #[inline]
    pub fn read_i128(&mut self) -> Result<i128, ReadBonErr> {
        self.read_integer::<i128>()
    }

    /// 读一个f32类型，如果二进制当前的值不是f32类型，返回Err
    #[inline]
    pub fn read_f32(&mut self) -> Result<f32, ReadBonErr> {
        self.probe_border(1)?;
        let t = self.bytes.get_u8();
        self.head += 1;
        match t {
            3 => Ok(0.0),
            4 => Ok(1.0),
            5..7 => {
                self.probe_border(4)?;
                self.head += 4;
                Ok(self.bytes.get_f32_le())
            }
            _ => {
                self.head -= 1;
                if let Ok(r) = self.read_integer::<u32>() {
                    return Ok(r as f32);
                }
                return Err(ReadBonErr::type_no_match(
                    "f32".to_string(),
                    t,
                    self.head - 1,
                ));
            }
        }
    }

    /// 读一个f64类型，如果二进制当前的值不是f64类型，返回Err
    pub fn read_f64(&mut self) -> Result<f64, ReadBonErr> {
        self.probe_border(1)?;
        let t = self.bytes.get_u8();
        self.head += 1;
        match t {
            3 => Ok(0.0),
            4 => Ok(1.0),
            6 => {
                self.probe_border(4)?;
                self.head += 4;
                Ok(self.bytes.get_f32_le() as f64)
            }
            7 => {
                self.probe_border(8)?;
                self.head += 8;
                Ok(self.bytes.get_f64_le())
            }
            _ => {
                self.head -= 1;
                if let Ok(r) = self.read_integer::<u64>() {
                    return Ok(r as f64);
                }
                return Err(ReadBonErr::type_no_match(
                    "f64".to_string(),
                    t,
                    self.head - 1,
                ));
            }
        }
    }

    /// 读出一个动态长度，正整数，不允许大于0x20000000
    #[inline]
    pub fn read_lengthen(&mut self) -> Result<u32, ReadBonErr> {
        self.probe_border(1)?;
        // let t = self.bytes.get_u8();
        let t = self.get_type_chunk().unwrap();
        if t < 0x80 {
            self.head += 1;
            self.bytes.advance(1);
            Ok(t as u32)
        } else if t < 0xC0 {
            self.head += 2;
            Ok(self.bytes.get_u16_ne() as u32 - 0x8000)
        } else if t < 0xE0 {
            self.head += 4;
            Ok(self.bytes.get_u32_ne() as u32 - 0xC0000000)
        } else {
            return Err(ReadBonErr::type_no_match(
                "lengthen".to_string(),
                t,
                self.head - 1,
            ));
        }
    }

    // 读一个二进制类型，如果二进制当前的值不是二进制类型，返回Err
    #[inline]
    pub fn read_bin(&mut self) -> Result<Vec<u8>, ReadBonErr> {
        self.probe_border(1)?;
        let t = self.bytes.get_u8();
        self.head += 1;

        self.read_bin_inner(t)
    }

    fn read_bin_inner(&mut self, t: u8) -> Result<Vec<u8>, ReadBonErr> {
        let len: usize;
        if t >= 111 && t <= 175 {
            len = (t as usize) - 111;
            self.head += len;
        } else {
            match t {
                176 => {
                    len = self.bytes.get_u8() as usize as usize;
                    self.head += len + 1;
                }
                177 => {
                    len = self.bytes.get_u16_le() as usize;
                    self.head += len + 2;
                }
                178 => {
                    len = self.bytes.get_u32_le() as usize;
                    self.head += len + 4;
                }
                179 => {
                    len = self.bytes.get_u16_le() as usize
                        + (self.bytes.get_u32_le() * 0x10000) as usize;
                    self.head += len + 6;
                }
                _ => {
                    return Err(ReadBonErr::type_no_match(
                        "bin".to_string(),
                        t,
                        self.head - 1,
                    ));
                }
            };
        }

        let dst = self.bytes.copy_to_bytes(len).to_vec();
        Ok(dst)
    }

    /// 读一个utf8编码的字符串类型，如果二进制当前的值不是utf8编码的字符串类型类型，返回Err
    #[inline]
    pub fn read_utf8(&mut self) -> Result<String, ReadBonErr> {
        self.probe_border(1)?;
        let t = self.bytes.get_u8();
        self.head += 1;
        self.read_utf8_inner(t)
    }

    fn read_utf8_inner(&mut self, t: u8) -> Result<String, ReadBonErr> {
        let len: usize;
        if t >= 42 && t <= 106 {
            len = t as usize - 42;
            self.head += len;
        } else {
            match t {
                107 => {
                    len = self.bytes.get_u8() as usize as usize;
                    self.head += len + 1;
                }
                108 => {
                    len = self.bytes.get_u16_le() as usize;
                    self.head += len + 2;
                }
                109 => {
                    len = self.bytes.get_u32_le() as usize;
                    self.head += len + 4;
                }
                110 => {
                    len = self.bytes.get_u16_le() as usize
                        + (self.bytes.get_u32_le() * 0x10000) as usize;
                    self.head += len + 6;
                }
                _ => {
                    return Err(ReadBonErr::type_no_match(
                        "string".to_string(),
                        t,
                        self.head - 1,
                    ));
                }
            }
        }

        let dst = self.bytes.copy_to_bytes(len);
        Ok(String::from_utf8_lossy(&*dst).to_string())
    }

    /// 读一个容器类型，如果二进制当前的值不是容器类型，返回Err
    pub fn read_container<T, F>(&mut self, read_next: F) -> Result<T, ReadBonErr>
    where
        F: FnOnce(&mut ReadBuffer, u32, u64) -> Result<T, ReadBonErr>,
    {
        self.probe_border(1)?;
        let t = self.bytes.get_u8();
        self.head += 1;
        let len: u64;
        if t >= 180 && t <= 244 {
            len = t as u64 - 180;
            self.head += len as usize;
        } else {
            match t {
                245 => {
                    len = self.bytes.get_u8() as u64;
                    self.head += 5;
                }
                246 => {
                    len = self.bytes.get_u16_le() as u64;
                    self.head += 6;
                }
                247 => {
                    len = self.bytes.get_u32_le() as u64;
                    self.head += 8;
                }
                248 => {
                    len =
                        self.bytes.get_u16_le() as u64 + (self.bytes.get_u32_le() * 0x10000) as u64;
                    self.head += 10;
                }
                _ => {
                    return Err(ReadBonErr::type_no_match(
                        "container".to_string(),
                        t,
                        self.head - 1,
                    ));
                }
            }
        }
        let tt = self.bytes.get_u32_le();
        read_next(self, tt, len)
    }

    /// 下一个值是否为None
    #[inline]
    pub fn is_nil(&mut self) -> Result<bool, ReadBonErr> {
        self.probe_border(1)?;
        // let first = self.bytes.get_u8();
        let first = self.get_type_chunk().unwrap();
        if first == 0 {
            self.head += 1;
            self.bytes.advance(1);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 读下一个数据，已经读到最后，返回Err。否则，返回下一个数据
    pub fn read(&mut self) -> Result<EnumValue, ReadBonErr> {
        self.probe_border(1)?;
        let first = self.bytes.get_u8();
        self.head += 1;
        match first {
            0 => Ok(EnumValue::Void),
            1 => Ok(EnumValue::Bool(false)),
            2 => Ok(EnumValue::Bool(true)),
            3 => Ok(EnumValue::F32(0.0)),
            4 => Ok(EnumValue::F32(1.0)),
            5 => Err(ReadBonErr::other("16-bit float unsupported".to_string())),
            6 => {
                self.head += 4;
                Ok(EnumValue::F32(self.bytes.get_f32_le()))
            }
            7 => {
                self.head += 8;
                Ok(EnumValue::F64(self.bytes.get_f64_le()))
            }
            8 => Err(ReadBonErr::other("128-bit float unsupported".to_string())),
            15 => Ok(EnumValue::I8(-1)),
            16..36 => Ok(EnumValue::U8(first - 16)),
            36 => {
                self.head += 1;
                Ok(EnumValue::U8(self.bytes.get_u8()))
            }
            37 => {
                self.head += 2;
                Ok(EnumValue::U16(self.bytes.get_u16_le()))
            }
            38 => {
                self.head += 4;
                Ok(EnumValue::U32(self.bytes.get_u32_le()))
            }
            39 => {
                self.head += 6;
                Ok(EnumValue::F64(
                    (self.bytes.get_u16_le() as u64 + ((self.bytes.get_u32_le() as u64) << 16))
                        as f64,
                ))
            }
            40 => {
                self.head += 8;
                Ok(EnumValue::U64(self.bytes.get_u64_le() as u64))
            }
            41 => {
                self.head += 16;
                Ok(EnumValue::U128(self.bytes.get_u128_le() as u128))
            }
            9 => {
                self.head += 1;
                Ok(EnumValue::I16(-(self.bytes.get_u8() as i16)))
            }
            10 => {
                self.head += 2;
                Ok(EnumValue::I32(-(self.bytes.get_u16_le() as i32)))
            }
            11 => {
                self.head += 4;
                Ok(EnumValue::I64(-(self.bytes.get_u32_le() as i64)))
            }
            12 => {
                self.head += 6;
                Ok(EnumValue::F64(
                    (-(self.bytes.get_u16_le() as i64) - ((self.bytes.get_u32_le() as i64) << 16))
                        as f64,
                ))
            }
            13 => {
                self.head += 8;
                Ok(EnumValue::I64(-(self.bytes.get_u64_le() as i64)))
            }
            14 => {
                self.head += 16;
                Ok(EnumValue::I128(-(self.bytes.get_u128_le() as i128)))
            }
            42..111 => self.read_utf8_inner(first).map(|op| EnumValue::Str(op)),
            111..180 => self.read_bin_inner(first).map(|op| EnumValue::Bin(op)),
            245..249 => {
                return Err(ReadBonErr::IsContainer(first));
            }
            _ => Err(ReadBonErr::other(format!("Unexpected type: {}", first))),
        }
    }

    /// 读一个整数类型，如果二进制当前的值不是整数类型，返回Err
    fn read_integer<
        T: AsFrom<u32> + AsFrom<u64> + AsFrom<i32> + AsFrom<i64> + AsFrom<i128> + AsFrom<u128>,
    >(
        &mut self,
    ) -> Result<T, ReadBonErr> {
        self.probe_border(1)?;
        let t = self.bytes.get_u8();
        self.head += 1;
        if t >= 15 && t <= 35 {
            Ok(T::from((t as i32) - 16))
        } else {
            match t {
                9 => {
                    self.head += 1;
                    Ok(T::from(-(self.bytes.get_u8() as i32)))
                }
                10 => {
                    self.head += 2;
                    Ok(T::from(-(self.bytes.get_u16_le() as i32)))
                }
                11 => {
                    self.head += 4;
                    Ok(T::from(-(self.bytes.get_u32_le() as i64)))
                }
                12 => {
                    self.head += 6;
                    Ok(T::from(
                        -(self.bytes.get_u16_le() as i64)
                            - ((self.bytes.get_u32_le() as i64) << 16),
                    ))
                }
                13 => {
                    self.head += 8;
                    Ok(T::from(-(self.bytes.get_u64_le() as i64)))
                }
                14 => {
                    self.head += 16;
                    Ok(T::from(-(self.bytes.get_u128_le() as i128)))
                }
                36 => {
                    self.head += 1;
                    Ok(T::from(self.bytes.get_u8() as u32))
                }
                37 => {
                    self.head += 2;
                    Ok(T::from(self.bytes.get_u16_le() as u32))
                }
                38 => {
                    self.head += 4;
                    Ok(T::from(self.bytes.get_u32_le()))
                }
                39 => {
                    self.head += 6;
                    Ok(T::from(
                        self.bytes.get_u16_le() as u64 + ((self.bytes.get_u32_le() as u64) << 16),
                    ))
                }
                40 => {
                    self.head += 8;
                    Ok(T::from(self.bytes.get_u64_le() as u64))
                }
                41 => {
                    self.head += 8;
                    Ok(T::from(self.bytes.get_u128_le() as u128))
                }
                _ => {
                    log::error!("read integer error, act_type: {}, bin: {:?}", t, self.bytes);
                    Err(ReadBonErr::type_no_match(
                        "integer".to_string(),
                        t,
                        self.head - 1,
                    ))
                }
            }
        }
    }

    //探测边界， 如果越界， 返回错误
    #[inline]
    fn probe_border(&self, len: usize) -> Result<(), ReadBonErr> {
        if len > self.bytes.len() {
            return Err(ReadBonErr::overflow(len, self.bytes.len()));
        } else {
            return Ok(());
        }
    }
}

/// 用于对数据进行序列化
#[derive(Default, Clone, Debug, Hash)]
pub struct WriteBuffer {
    // u8数组
    pub bytes: Vec<u8>,
    // 尾部指针
    tail: usize,
}

impl Deref for WriteBuffer {
    type Target = Vec<u8>;
    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

impl PartialOrd for WriteBuffer {
    fn partial_cmp(&self, other: &WriteBuffer) -> Option<Ordering> {
        ReadBuffer::new(self.bytes.as_slice(), 0)
            .partial_cmp(&ReadBuffer::new(other.bytes.as_slice(), 0))
    }
}

impl PartialEq for WriteBuffer {
    fn eq(&self, other: &WriteBuffer) -> bool {
        match self.partial_cmp(other) {
            Some(Ordering::Equal) => return true,
            _ => return false,
        };
    }
}

impl Eq for WriteBuffer {}

impl Ord for WriteBuffer {
    fn cmp(&self, other: &WriteBuffer) -> Ordering {
        match self.partial_cmp(other) {
            Some(v) => v,
            None => panic!("partial_cmp fail self:{:?}, other:{:?}", self, other),
        }
    }
}

impl WriteBuffer {
    /// 创建WriteBuffer
    pub fn new() -> WriteBuffer {
        WriteBuffer {
            bytes: Vec::new(),
            tail: 0,
        }
    }

    /// 使用现有的buffer创建WriteBuffer，并指定下一次写的指针位置，tail为下一次写的指针。
    pub fn with_bytes(buf: Vec<u8>, tail: usize) -> WriteBuffer {
        WriteBuffer {
            bytes: buf,
            tail: tail,
        }
    }

    /// 创建WriteBuffer，并指定容量
    pub fn with_capacity(size: usize) -> WriteBuffer {
        WriteBuffer {
            bytes: Vec::with_capacity(size),
            tail: 0,
        }
    }

    /// 下一次写的起始位置
    pub fn tail(&self) -> usize {
        self.tail
    }

    /// 拿到当前WriteBuffer中的buffer的引用
    pub fn get_byte(&self) -> &Vec<u8> {
        &self.bytes
    }

    /// 拿到当前WriteBuffer中的buffer的所有权
    pub fn unwrap(self) -> Vec<u8> {
        self.bytes
    }

    /// 清空buffer
    pub fn clear(&mut self) {
        self.tail = 0;
    }

    /// 写一个u8
    pub fn write_u8(&mut self, v: u8) {
        self.write_uint32(v as u32);
    }

    /// 写一个u16
    pub fn write_u16(&mut self, v: u16) {
        self.write_uint32(v as u32);
    }

    /// 写一个u32
    pub fn write_u32(&mut self, v: u32) {
        self.write_uint32(v);
    }

    /// 写一个u64
    pub fn write_u64(&mut self, v: u64) {
        self.write_uint64(v);
    }

    /// 写一个u128
    pub fn write_u128(&mut self, v: u128) {
        self.write_uint128(v);
    }

    /// 写一个i8
    pub fn write_i8(&mut self, v: i8) {
        self.write_int32(v as i32);
    }

    /// 写一个i16
    pub fn write_i16(&mut self, v: i16) {
        self.write_int32(v as i32);
    }

    /// 写一个i32
    pub fn write_i32(&mut self, v: i32) {
        self.write_int32(v);
    }

    /// 写一个i64
    pub fn write_i64(&mut self, v: i64) {
        self.write_int64(v);
    }

    /// 写一个i128
    pub fn write_i128(&mut self, v: i128) {
        self.write_int128(v);
    }

    /// 写一个None
    pub fn write_nil(&mut self) {
        self.try_extend_capity(1);
        self.bytes.put_u8(0);
        self.tail += 1;
    }

    /// 写一个bool
    pub fn write_bool(&mut self, v: bool) {
        self.try_extend_capity(1);
        self.bytes.put_u8(match v {
            true => 2,
            false => 1,
        });
        self.tail += 1;
    }

    /// 写一个f32
    pub fn write_f32(&mut self, v: f32) {
        if v == 0.0 {
            self.try_extend_capity(1);
            self.bytes.put_u8(3);
            self.tail += 1;
            return;
        }
        if v == 1.0 {
            self.try_extend_capity(1);
            self.bytes.put_u8(4);
            self.tail += 1;
            return;
        }
        self.try_extend_capity(5);
        self.bytes.put_u8(6);
        self.bytes.put_f32_le(v);
        self.tail += 5;
    }

    /// 写一个f64
    pub fn write_f64(&mut self, v: f64) {
        if v == 0.0 {
            self.try_extend_capity(1);
            self.bytes.put_u8(3);
            self.tail += 1;
            return;
        }
        if v == 1.0 {
            self.try_extend_capity(1);
            self.bytes.put_u8(4);
            self.tail += 1;
            return;
        }
        self.try_extend_capity(9);
        self.bytes.put_u8(7);
        self.bytes.put_f64_le(v);
        self.tail += 9;
    }
    /// 写入一个动态长度，正整数，不允许大于0x20000000。
    /// * 1字节： 0xxxxxxx
    /// * 2字节： 10xxxxxx xxxxxxxx
    /// * 4字节： 110xxxxx xxxxxxxx xxxxxxxx xxxxxxxx
    pub fn write_lengthen(&mut self, t: u32) -> Result<(), ReadBonErr> {
        if t < 0x80 {
            self.try_extend_capity(1);
            self.bytes.put_u8(t as u8);
            self.tail += 1;
        } else if t < 0x4000 {
            self.try_extend_capity(2);
            self.bytes.put_u16_ne((0x8000 + t) as u16);
            self.tail += 2;
        } else if t < 0x20000000 {
            self.try_extend_capity(4);
            self.bytes.put_u32_ne((0xC0000000 + t) as u32);
            self.tail += 4;
        } else {
            return Err(ReadBonErr::other(format!("Invalid lengthen: {}", t)));
        }
        Ok(())
    }

    ///写字符串
    pub fn write_utf8(&mut self, s: &str) {
        self.write_data(s.as_bytes(), 42);
    }

    /// 写二进制数据
    pub fn write_bin(&mut self, arr: &[u8], range: Range<usize>) {
        self.write_data(&arr[range], 111)
    }

    /// 写容器。容器有数组，map，枚举，struct
    pub fn write_container<T, F>(
        &mut self,
        o: &T,
        write_next: F,
        estimated_size: Option<usize>,
    ) -> Result<(), ReadBonErr>
    where
        F: Fn(&mut WriteBuffer, &T) -> Result<(), ReadBonErr>,
    {
        let t = self.bytes.len();
        let len_bytes: usize; //描述容器长度的值的字节数
        let capacity = self.bytes.capacity();
        // 根据预估大小，预留出足够的空间来写入容器的总大小
        let estimated_size = match estimated_size {
            Some(v) => v,
            None => 0xffff,
        };
        let mut limit_size: u64;

        if estimated_size <= 64 {
            self.try_extend_capity(5 + estimated_size);
            len_bytes = 0;
            limit_size = 64;
        } else if estimated_size <= 0xff {
            self.try_extend_capity(6 + estimated_size);
            len_bytes = 1;
            limit_size = 0xff;
        } else if estimated_size <= 0xffff {
            self.try_extend_capity(8 + estimated_size);
            len_bytes = 3;
            limit_size = 0xffff;
        } else if estimated_size <= 0xffffffff {
            self.try_extend_capity(10 + estimated_size);
            len_bytes = 5;
            limit_size = 0xffffffff;
        } else if estimated_size as u64 <= 0xffffffffffff {
            self.try_extend_capity(12 + estimated_size);
            len_bytes = 7;
            limit_size = 0xffffffffffff;
        } else {
            self.try_extend_capity(14 + estimated_size);
            len_bytes = 9;
            limit_size = 0xffffffffffffffff;
        }
        let tt = self.tail;
        write_next(self, o)?;
        let len = (self.tail - tt) as u64;
        // 判断实际写入的大小超出预期的大小，需要移动数据
        if limit_size < len && len > 64 {
            let mut len_bytes1: usize = 0;
            if limit_size <= 64 && len <= 0xff {
                len_bytes1 = 1;
                limit_size = 0xff;
            } else if len <= 0xffff {
                len_bytes1 = 3;
                limit_size = 0xffff;
            } else if len <= 0xffffffff {
                len_bytes1 = 5;
                limit_size = 0xffffffff;
            } else if len <= 0xffffffffffff as u64 {
                len_bytes1 = 7;
                limit_size = 0xffffffffffff;
            } else if len <= 0xfffffffffffffffe + 1 {
                len_bytes1 = 9;
                limit_size = 0xffffffffffffffff;
            }

            let offset = len_bytes1 - len_bytes;
            let l = self.bytes.len();
            self.try_extend_capity(l + offset - capacity);
            // self.bytes.move_part(t..l, t + offset);
            move_part(&mut self.bytes, t..l, t + offset);
            self.tail += offset;
        }
        // 根据实际的限制大小，写入实际长度
        match limit_size {
            64 => {
                self.bytes.put_u8((180 + len) as u8);
                Ok(())
            }
            0xff => {
                self.bytes.put_u8(245);
                self.bytes.put_u8(len as u8);
                Ok(())
            }
            0xffff => {
                let mut v: Vec<u8> = Vec::with_capacity(1 + 2 + self.bytes.len());
                v.put_u8(246);
                v.put_u16_le(len as u16);
                v.extend_from_slice(&self.bytes);
                std::mem::replace(&mut self.bytes, v);
                Ok(())
            }
            0xffffffff => {
                let mut v: Vec<u8> = Vec::with_capacity(1 + 4 + self.bytes.len());
                v.put_u8(247);
                v.put_u32_le(len as u32);
                v.extend_from_slice(&self.bytes);
                std::mem::replace(&mut self.bytes, v);
                Ok(())
            }
            0xffffffffffff => {
                let mut v: Vec<u8> = Vec::with_capacity(1 + 6 + self.bytes.len());
                v.put_u8(248);
                v.put_u16_le((len & 0xffff) as u16);
                v.put_u32_le((len >> 16) as u32);
                std::mem::replace(&mut self.bytes, v);
                Ok(())
            }

            _ => return Err(ReadBonErr::other("Container overflow".to_string())),
        }
    }

    // 扩容
    fn extend_capity(&mut self, len: usize) {
        let old_capacity = self.bytes.capacity();
        if old_capacity > 4194304 {
            //4M
            self.bytes.reserve_exact(len * 2); //准确扩容
        } else {
            self.bytes.reserve(len); //使用vec内部规则扩容（扩大为原有大小的两倍）
        }
    }

    // 尝试扩容
    fn try_extend_capity(&mut self, len: usize) {
        if self.bytes.len() + len > self.bytes.capacity() {
            self.extend_capity(len);
        }
    }

    //写字符串或二进制
    fn write_data(&mut self, arr: &[u8], t: u8) {
        let length = arr.len();
        if length <= 64 {
            self.try_extend_capity(1 + length);
            // 长度小于等于64， 本字节直接表达
            self.bytes.put_u8(t + length as u8);
            self.tail += 1;
        } else if length <= 0xff {
            self.try_extend_capity(2 + length);
            // 长度小于256， 用下一个1字节记录
            self.bytes.put_u8(t + 65);
            self.bytes.put_u8(length as u8);
            self.tail += 2;
        } else if length <= 0xffff {
            self.try_extend_capity(3 + length);
            self.bytes.put_u8(t + 66);
            self.bytes.put_u16_le(length as u16);
            self.tail += 3;
        } else if length <= 0xffffffff {
            self.try_extend_capity(5 + length);
            self.bytes.put_u8(t + 67);
            self.bytes.put_u32_le(length as u32);
            self.tail += 5;
        } else if length as u64 <= 0xffffffffffff {
            self.try_extend_capity(7 + length);
            self.bytes.put_u8(t + 68);
            self.bytes.put_u16_le((length & 0xffff) as u16);
            self.bytes.put_u32_le((length >> 16) as u32);
            self.tail += 7;
        } else {
            self.try_extend_capity(9 + length);
            self.bytes.put_u8(t + 69);
            self.bytes.put_u64_le(t as u64);
            self.tail += 9;
        }
        self.bytes.put(arr);
        self.tail += length;
    }

    // 写32的整数
    // 在32为平台依然高效
    fn write_int32(&mut self, mut v: i32) {
        if v >= -1 && v < 20 {
            self.write_common(v as i8);
            return;
        }
        let mut t = 36;
        if v < 0 {
            v = -v;
            t = 36 - 27;
        }
        self.writei_32(v as u32, t);
    }

    // 写64位整数
    // 在64位平台下依然高效
    fn write_int64(&mut self, mut v: i64) {
        if v >= -1 && v < 20 {
            self.write_common(v as i8);
            return;
        }
        let mut t = 36;
        if v < 0 {
            v = -v;
            t = 36 - 27;
        }
        if v <= 0x7FFFFFFF {
            self.writei_32(v as u32, t);
        } else {
            self.writei_64(v as u64, t);
        }
    }

    // 写128位整数
    fn write_int128(&mut self, mut v: i128) {
        if v >= -1 && v < 20 {
            self.write_common(v as i8);
            return;
        }
        let mut t = 36;
        if v < 0 {
            v = -v;
            t = 36 - 27;
        }
        if v <= 0x7FFFFFFF {
            self.writei_32(v as u32, t);
        } else if v <= 0x7FFFFFFFFFFFFFFF {
            self.writei_64(v as u64, t);
        } else {
            self.write_128(v as u128, t + 5);
        }
    }

    // 写一个32位正整数
    // 在32位平台下依然高效
    fn write_uint32(&mut self, v: u32) {
        if v < 20 {
            self.write_common(v as i8);
        } else {
            self.writeu_32(v as u32);
        }
    }

    // 写一个64位正整数
    // 在64位平台下依然高效
    fn write_uint64(&mut self, v: u64) {
        if v < 20 {
            self.write_common(v as i8);
        } else if v <= 0xFFFFFFFF {
            self.writeu_32(v as u32);
        } else {
            self.writeu_64(v);
        }
    }

    // 写一个128位的正整数
    fn write_uint128(&mut self, v: u128) {
        if v < 20 {
            self.write_common(v as i8);
        } else if v <= 0xFFFFFFFF {
            self.writeu_32(v as u32);
        } else if v <= 0xFFFFFFFFFFFF {
            self.writeu_64(v as u64);
        } else {
            self.write_128(v, 41);
        }
    }

    // 写32数字, 不包括-1~19
    #[inline]
    fn writei_32(&mut self, v: u32, t: u8) {
        if v <= 0x7F {
            self.write_8(v as u8, t);
        } else if v <= 0x7FFF {
            self.write_16(v as u16, t + 1);
        } else {
            self.write_32(v as u32, t + 2);
        }
    }

    //写64位数字， 只有大于32位数字时调用此方法
    #[inline]
    fn writei_64(&mut self, v: u64, t: u8) {
        if v <= 0x7FFFFFFFFFFF {
            self.write_48(v, t + 3);
        } else {
            self.write_64(v, t + 4);
        }
    }

    //写32正整数, 不包括-1~19
    #[inline]
    fn writeu_32(&mut self, v: u32) {
        if v <= 0xFF {
            self.write_8(v as u8, 36);
        } else if v <= 0xFFFF {
            self.write_16(v as u16, 37);
        } else {
            self.write_32(v as u32, 38);
        }
    }

    //写64位正整数， 只有大于32位数字时调用此方法
    #[inline]
    fn writeu_64(&mut self, v: u64) {
        if v <= 0xFFFFFFFFFFFF {
            self.write_48(v, 39);
        } else {
            self.write_64(v, 40);
        }
    }

    //写常用数字-1~19
    #[inline]
    fn write_common(&mut self, v: i8) {
        self.try_extend_capity(1);
        self.bytes.put_u8((v + 16) as u8);
        self.tail += 1;
    }

    // 写一个字节
    #[inline]
    fn write_8(&mut self, v: u8, t: u8) {
        self.try_extend_capity(2);
        self.bytes.put_u8(t);
        self.bytes.put_u8(v);
        self.tail += 2;
    }

    // 写2字节（小端）
    #[inline]
    fn write_16(&mut self, v: u16, t: u8) {
        self.try_extend_capity(3);
        self.bytes.put_u8(t);
        self.bytes.put_u16_le(v as u16);
        self.tail += 3;
    }

    // 写4字节（小端）
    #[inline]
    fn write_32(&mut self, v: u32, t: u8) {
        self.try_extend_capity(5);
        self.bytes.put_u8(t);
        self.bytes.put_u32_le(v as u32);
        self.tail += 5;
    }

    // 写6字节（小端）
    #[inline]
    fn write_48(&mut self, v: u64, t: u8) {
        self.try_extend_capity(7);
        self.bytes.put_u8(t);
        self.bytes.put_u16_le((v & 0xffff) as u16);
        self.bytes.put_u32_le((v >> 16) as u32);
        self.tail += 7;
    }

    // 写8字节（小端）
    #[inline]
    fn write_64(&mut self, v: u64, t: u8) {
        self.try_extend_capity(9);
        self.bytes.put_u8(t);
        self.bytes.put_u64_le(v as u64);
        self.tail += 9;
    }

    // 写16字节（小端）
    #[inline]
    fn write_128(&mut self, v: u128, t: u8) {
        self.try_extend_capity(17);
        self.bytes.put_u8(t);
        self.bytes.put_u128_le(v);
        self.tail += 17;
    }
}

trait AsFrom<T> {
    fn from(t: T) -> Self;
}

impl AsFrom<u32> for u32 {
    fn from(t: u32) -> u32 {
        t
    }
}
impl AsFrom<u64> for u32 {
    fn from(t: u64) -> u32 {
        t as u32
    }
}

impl AsFrom<u128> for u32 {
    fn from(t: u128) -> u32 {
        t as u32
    }
}

impl AsFrom<i32> for u32 {
    fn from(t: i32) -> u32 {
        t as u32
    }
}

impl AsFrom<i64> for u32 {
    fn from(t: i64) -> u32 {
        t as u32
    }
}

impl AsFrom<i128> for u32 {
    fn from(t: i128) -> u32 {
        t as u32
    }
}

impl AsFrom<u64> for u64 {
    fn from(t: u64) -> u64 {
        t
    }
}
impl AsFrom<u32> for u64 {
    fn from(t: u32) -> u64 {
        t as u64
    }
}

impl AsFrom<u128> for u64 {
    fn from(t: u128) -> u64 {
        t as u64
    }
}

impl AsFrom<i32> for u64 {
    fn from(t: i32) -> u64 {
        t as u64
    }
}

impl AsFrom<i64> for u64 {
    fn from(t: i64) -> u64 {
        t as u64
    }
}

impl AsFrom<i128> for u64 {
    fn from(t: i128) -> u64 {
        t as u64
    }
}

impl AsFrom<u64> for i32 {
    fn from(t: u64) -> i32 {
        t as i32
    }
}

impl AsFrom<u32> for i32 {
    fn from(t: u32) -> i32 {
        t as i32
    }
}

impl AsFrom<u128> for i32 {
    fn from(t: u128) -> i32 {
        t as i32
    }
}

impl AsFrom<i32> for i32 {
    fn from(t: i32) -> i32 {
        t
    }
}

impl AsFrom<i64> for i32 {
    fn from(t: i64) -> i32 {
        t as i32
    }
}

impl AsFrom<i128> for i32 {
    fn from(t: i128) -> i32 {
        t as i32
    }
}

impl AsFrom<u64> for i64 {
    fn from(t: u64) -> i64 {
        t as i64
    }
}

impl AsFrom<u32> for i64 {
    fn from(t: u32) -> i64 {
        t as i64
    }
}

impl AsFrom<u128> for i64 {
    fn from(t: u128) -> i64 {
        t as i64
    }
}

impl AsFrom<i32> for i64 {
    fn from(t: i32) -> i64 {
        t as i64
    }
}

impl AsFrom<i64> for i64 {
    fn from(t: i64) -> i64 {
        t
    }
}

impl AsFrom<i128> for i64 {
    fn from(t: i128) -> i64 {
        t as i64
    }
}

impl AsFrom<u64> for u128 {
    fn from(t: u64) -> u128 {
        t as u128
    }
}

impl AsFrom<u32> for u128 {
    fn from(t: u32) -> u128 {
        t as u128
    }
}

impl AsFrom<u128> for u128 {
    fn from(t: u128) -> u128 {
        t
    }
}

impl AsFrom<i32> for u128 {
    fn from(t: i32) -> u128 {
        t as u128
    }
}

impl AsFrom<i64> for u128 {
    fn from(t: i64) -> u128 {
        t as u128
    }
}

impl AsFrom<i128> for u128 {
    fn from(t: i128) -> u128 {
        t as u128
    }
}

impl AsFrom<u64> for i128 {
    fn from(t: u64) -> i128 {
        t as i128
    }
}

impl AsFrom<u32> for i128 {
    fn from(t: u32) -> i128 {
        t as i128
    }
}

impl AsFrom<u128> for i128 {
    fn from(t: u128) -> i128 {
        t as i128
    }
}

impl AsFrom<i32> for i128 {
    fn from(t: i32) -> i128 {
        t as i128
    }
}

impl AsFrom<i64> for i128 {
    fn from(t: i64) -> i128 {
        t as i128
    }
}

impl AsFrom<i128> for i128 {
    fn from(t: i128) -> i128 {
        t
    }
}

pub trait Encode: Sized {
    fn encode(&self, bb: &mut WriteBuffer);
}

pub trait Decode: Sized {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr>;
}

impl Encode for u8 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_u8(self.clone());
    }
}

impl Decode for u8 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_u8()
    }
}

impl Encode for u16 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_u16(self.clone());
    }
}

impl Decode for u16 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_u16()
    }
}

impl Encode for u32 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_u32(self.clone());
    }
}

impl Decode for u32 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_u32()
    }
}

impl Encode for u64 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_u64(self.clone());
    }
}

impl Decode for u64 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_u64()
    }
}

impl Encode for u128 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_u128(self.clone());
    }
}

impl Decode for u128 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_u128()
    }
}

impl Encode for i8 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_i8(self.clone());
    }
}

impl Decode for i8 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_i8()
    }
}

impl Encode for i16 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_i16(self.clone());
    }
}

impl Decode for i16 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_i16()
    }
}

impl Encode for i32 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_i32(self.clone())
    }
}

impl Decode for i32 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_i32()
    }
}

impl Encode for i64 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_i64(self.clone());
    }
}

impl Decode for i64 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_i64()
    }
}

impl Encode for i128 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_i128(self.clone());
    }
}

impl Decode for i128 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_i128()
    }
}

impl Encode for f32 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_f32(self.clone());
    }
}

impl Decode for f32 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_f32()
    }
}

impl Encode for f64 {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_f64(self.clone());
    }
}

impl Decode for f64 {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_f64()
    }
}

impl Encode for bool {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_bool(self.clone());
    }
}

impl Decode for bool {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_bool()
    }
}

impl Encode for usize {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_u64(self.clone() as u64);
    }
}

impl Decode for usize {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_usize()
    }
}

impl Encode for isize {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_i64(self.clone() as i64);
    }
}

impl Decode for isize {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_isize()
    }
}

impl Encode for String {
    fn encode(&self, bb: &mut WriteBuffer) {
        bb.write_utf8(self);
    }
}

impl Decode for String {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        bb.read_utf8()
    }
}

impl<K: Encode + Eq + Hash, V: Encode> Encode for HashMap<K, V> {
    fn encode(&self, bb: &mut WriteBuffer) {
        //self.typeid().encode(bb);
        self.len().encode(bb);
        for (k, v) in self.iter() {
            k.encode(bb);
            v.encode(bb);
        }
    }
}

impl<K: Decode + Eq + Hash, V: Decode> Decode for HashMap<K, V> {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        let mut map = HashMap::new();
        let count = usize::decode(bb)?;
        for _ in 0..count {
            map.insert(K::decode(bb)?, V::decode(bb)?);
        }
        Ok(map)
    }
}

impl<T: Encode> Encode for Vec<T> {
    fn encode(&self, bb: &mut WriteBuffer) {
        self.len().encode(bb);
        for v in self.iter() {
            v.encode(bb);
        }
    }
}

impl<T: Decode> Decode for Vec<T> {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        let count = usize::decode(bb)?;
        let mut vec = Vec::new();
        for _ in 0..count {
            vec.push(T::decode(bb)?);
        }
        Ok(vec)
    }
}

impl<T: Encode> Encode for Option<T> {
    fn encode(&self, bb: &mut WriteBuffer) {
        match self {
            &Some(ref v) => {
                v.encode(bb);
            }
            &None => {
                bb.write_nil();
            }
        }
    }
}

impl<T: Decode> Decode for Option<T> {
    fn decode(bb: &mut ReadBuffer) -> Result<Self, ReadBonErr> {
        match bb.is_nil()? {
            true => Ok(None),
            false => Ok(Some(T::decode(bb)?)),
        }
    }
}

pub fn partial_cmp<'a>(
    b1: &mut ReadBuffer<'a>,
    b2: &mut ReadBuffer<'a>,
) -> Result<Option<Ordering>, ReadBonErr> {
    let err = "partial_cmp err";
    let t1 = b1.get_type_chunk().expect(err);
    let t2 = b2.get_type_chunk().expect(err);
    // println!("partial_cmp  b1:{:?}, b2:{:?}", b1.bytes, b2.bytes);
    // println!(
    //     "###########################t1:{:?}, t2:{:?}, b1.head:{}, b2.head:{}, b1:{:?}, b2:{:?}",
    //     t1, t2, b1.head, b2.head, &b1, &b2
    // );
    match (t1, t2) {
        (3..8, 3..42) => {
            // b1是浮点数， b2是数字,需要读取比较对象的值进行比较
            let v1 = match t1 < 7 {
                true => b1.read_f32().expect(err) as f64,
                false => b1.read_f64().expect(err),
            };
            compare_number(b2, v1, t2)
        }
        (3..8, 0..3) => {
            // b1是浮点数， b2是非数字， 并且b1的类型值大于b2的类型值，则认为b1更大
            let len1 = base_type_len(b1, t1)?;
            b1.head += len1;
            b2.head += 1;
            b1.bytes.advance(len1);
            b2.bytes.advance(1);
            Ok(Some(Ordering::Greater))
        }
        (3..8, _) => {
            // b1是浮点数， b2是非数字， 并且b1的类型值小于b2的类型值，则认为b1更小
            let len1 = base_type_len(b1, t1)?;
            b1.head += len1;
            b1.bytes.advance(len1);
            let len2 = base_type_len(b2, t2)?;
            b2.head += len2;
            b2.bytes.advance(len2);
            Ok(Some(Ordering::Less))
        }
        (9..42, 3..8) => {
            // b1是整数， b2是浮点数，需要读取比较对象的值进行比较
            let v2 = match t2 < 7 {
                true => b2.read_f32().expect(err) as f64,
                false => b2.read_f64().expect(err),
            };
            match compare_number(b1, v2, t1)? {
                Some(Ordering::Less) => Ok(Some(Ordering::Greater)),
                Some(Ordering::Greater) => Ok(Some(Ordering::Less)),
                Some(Ordering::Equal) => Ok(Some(Ordering::Equal)),
                None => Ok(None),
            }
        }
        (9..42, 9..42) => {
            // b1是整数, b2是整数
            if t1 > t2 {
                //同是整数， 类型较大的，值也较大
                let len1 = base_type_len(b1, t1)?;
                let len2 = base_type_len(b2, t2)?;
                b1.bytes.advance(len1);
                b2.bytes.advance(len2);

                b1.head += len1;
                b2.head += len2;

                return Ok(Some(Ordering::Greater));
            } else if t1 < t2 {
                //同是整数， 类型较小的，值也较小
                let len1 = base_type_len(b1, t1)?;
                let len2 = base_type_len(b2, t2)?;
                b1.bytes.advance(len1);
                b2.bytes.advance(len2);

                b1.head += len1;
                b2.head += len2;
                return Ok(Some(Ordering::Less));
            } else if t1 > 14 && t1 < 36 {
                //同是整数且类型相等， 当类型值在15~35之间时，其表示的数值大小是确定的（-1~19）， 因此， b1与b2相等
                let len1 = base_type_len(b1, t1)?;
                let len2 = base_type_len(b2, t2)?;
                b1.bytes.advance(len1);
                b2.bytes.advance(len2);

                b1.head += len1;
                b2.head += len2;
                return Ok(Some(Ordering::Equal));
            } else {
                //同是整数且类型相等，但并不是常用数字（-1~19），需要读值进行比较
                return compare_int(b1, b2, t1);
            }
        }
        (9..42, 0..3) => {
            //b1是整数， b2是非数字，并且b1的类型值更大，则b1更大
            let len1 = base_type_len(b1, t1)?;
            b1.bytes.advance(len1);
            b2.bytes.advance(1);

            b1.head += len1;
            b2.head += 1;
            Ok(Some(Ordering::Greater))
        }
        (9..42, _) => {
            //b1是整数， b2是非数字，并且b1的类型值更小，则b1更小
            let len1 = base_type_len(b1, t1)?;
            let len2 = base_type_len(b2, t2)?;
            b1.bytes.advance(len1);
            b2.bytes.advance(len2);
            b1.head += len1;
            b2.head += len2;
            Ok(Some(Ordering::Less))
        }
        (0..3, _) => {
            //b1是null, true或false， 理论上除了与自身相等， 无法与其他类型的值进行比较， 规定其大小与其类型值保持一致
            b1.head += 1;
            b1.bytes.advance(1);
            let len2 = base_type_len(b2, t2)?;
            b2.head += len2;
            b2.bytes.advance(len2);
            if t2 > t1 {
                //t1小于3， t2大于t1,
                return Ok(Some(Ordering::Less));
            } else if t2 < t1 {
                return Ok(Some(Ordering::Greater));
            } else {
                return Ok(Some(Ordering::Equal));
            }
        }
        (42..111, _) => {
            //b1是字符串
            if t2 > 110 {
                //b1是字符串， b2是非字符串，且b1的类型值更小， 则b1更小
                let len1 = base_type_len(b1, t1)?;
                let len2 = base_type_len(b2, t2)?;
                b1.bytes.advance(len1);
                b2.bytes.advance(len2);

                b1.head += len1;
                b2.head += len2;
                return Ok(Some(Ordering::Less));
            } else if t2 < 42 {
                //b1是字符串， b2是非字符串，且b1的类型值更大， 则b1更大
                let len1 = base_type_len(b1, t1)?;
                let len2 = base_type_len(b2, t2)?;
                b1.bytes.advance(len1);
                b2.bytes.advance(len2);

                b1.head += len1;
                b2.head += len2;
                return Ok(Some(Ordering::Greater));
            } else {
                //b1是字符串， b2也是字符串，需要读值比较字符串的二进制数据的大小
                return compare_str(b1, b2);
            }
        }
        (111..180, _) => {
            // b1是二进制
            if t2 > 179 {
                // b1是二进制， b2是非二进制，且b1的类型值更小， 则b1更小
                let len1 = base_type_len(b1, t1)?;
                let len2 = base_type_len(b2, t2)?;
                b1.bytes.advance(len1);
                b2.bytes.advance(len2);

                b1.head += len1;
                b2.head += len2;
                return Ok(Some(Ordering::Less));
            } else if t2 < 111 {
                // b1是二进制， b2是非二进制，且b1的类型值更大， 则b1更大
                let len1 = base_type_len(b1, t1)?;
                let len2 = base_type_len(b2, t2)?;
                b1.bytes.advance(len1);
                b2.bytes.advance(len2);

                b1.head += len1;
                b2.head += len2;
                return Ok(Some(Ordering::Greater));
            } else {
                // b1是二进制， b2也是二进制，需要读值比二进制数据的大小
                return compare_bin(b1, b2);
            }
        }
        (_, 0) => {
            return Ok(Some(Ordering::Greater));
        }
        (249, 249) => {
            // b1.head += 32;
            // b2.head += 32;
            // b1.bytes[b1.head - 32..b1.head].partial_cmp(&b2.bytes[b2.head - 32..b2.head])
            // BigInt::from_bytes_le(Sign::Plus, &b1.bytes[b1.head - 32..b1.head]);
            let b1n = to_bigint(b1);
            let b2n = to_bigint(b2);
            return Ok(b1n.partial_cmp(&b2n));
        }
        _ => {
            // b1是容器， b2也是二进制，需要读值比二进制数据的大小
            if t2 < 180 {
                // b1是容器， b2是非容器，b1的类型值更大， 则b1更大
                let len1 = base_type_len(b1, t1)?;
                let len2 = base_type_len(b2, t2)?;
                b1.bytes.advance(len1);
                b2.bytes.advance(len2);

                b1.head += len1;
                b2.head += len2;
                return Ok(Some(Ordering::Greater));
            } else {
                // b1是容器， b2也是容器，需要读值比容器二进制数据的大小
                return compare_contain(b1, b2);
            }
        }
    }
}

fn to_bigint<'a>(bb: &mut ReadBuffer<'a>) -> BigInt {
    let mut base = BigInt::from(1);
    let mut n = BigInt::from(0);
    let mut c = BigInt::from(4294967296 as i64);
    bb.head += 1;
    bb.bytes.advance(1);
    for _ in 0..=7 {
        bb.head += 4;
        // n += BigInt::from_bytes_le(Sign::Plus, &bb.bytes[bb.head - 4..bb.head]);
        n += BigInt::from_bytes_le(Sign::Plus, &bb.bytes.copy_to_bytes(4));
        base *= &c;
    }
    n
}

pub fn base_type_len(bb: &mut ReadBuffer, t: u8) -> Result<usize, ReadBonErr> {
    let len = match t {
        0..5 | 15..36 => 1,
        5 => return Err(ReadBonErr::Other("16-bit float unsupported".into())),
        6 => 5,
        7 => 9,
        8 => return Err(ReadBonErr::Other("128-bit float unsupported".into())),
        9 | 36 => 2,
        10 | 37 => 3,
        11 | 38 => 5,
        12 | 39 => 7,
        13 | 40 => 9,
        14 | 41 => 17,
        42..107 => (t - 42) as usize + 1,
        111..176 => (t - 111) as usize + 1,
        107 | 176 => {
            bb.bytes.advance(1);
            bb.bytes.get_u8() as usize
        }
        108 | 177 => {
            bb.bytes.advance(1);
            bb.bytes.get_u16_le() as usize
        }
        109 | 178 => {
            bb.bytes.advance(1);
            bb.bytes.get_u32_le() as usize
        }
        110 | 179 => {
            bb.bytes.advance(1);
            bb.bytes.get_u32_le() as usize + 2
        }
        249 | 250 => 32,
        _ => return Err(ReadBonErr::Other(format!("Unknown type code: {}", t))),
    };

    Ok(len)
}

fn compare_number<'a>(rb: &mut ReadBuffer<'a>, v1: f64, t2: u8) -> Result<Option<Ordering>, ReadBonErr> {
    let v2 = match t2 {
        3..8 => rb.read_f64()?,
        9..14 => rb.read_i64()? as f64,
        14 => {
            rb.head += 17;
            rb.bytes.advance(17);
            return Ok(Some(Ordering::Greater));
        }
        15 => {
            rb.head += 1;
            rb.bytes.advance(1);
            -1.0
        }
        16..41 => rb.read_u64()? as f64,
        41 => {
            rb.head += 17;
            rb.bytes.advance(17);
            return Ok(Some(Ordering::Less));
        }
        _ => {
            return Err(ReadBonErr::type_no_match(
                "number".to_string(),
                t2,
                rb.head,
            ))
        },
    };
    if v1.is_nan() {
        if v2.is_nan() {
            return Ok(Some(Ordering::Equal));
        } else {
            return Ok(Some(Ordering::Less));
        }
    }
    Ok(v1.partial_cmp(&v2))
}

fn compare_int<'a>(
    rb1: &mut ReadBuffer<'a>,
    rb2: &mut ReadBuffer<'a>,
    t: u8,
) -> Result<Option<Ordering>, ReadBonErr> {
    let err = "compare_int";
    match t {
        9..14 => {
            let v1 = rb1.read_i64()?;
            let v2 = rb2.read_i64()?;
            Ok(v1.partial_cmp(&v2))
        }
        14 => {
            let v1 = rb1.read_i128()?;
            let v2 = rb2.read_i128()?;
            Ok(v1.partial_cmp(&v2))
        }
        36..41 => {
            let v1 = rb1.read_u64()?;
            let v2 = rb2.read_u64()?;
            Ok(v1.partial_cmp(&v2))
        }
        41 => {
            let v1 = rb1.read_u128()?;
            let v2 = rb2.read_u128()?;
            Ok(v1.partial_cmp(&v2))
        }
        _ => Err(ReadBonErr::type_no_match(
            "integer".to_string(),
            t,
            rb1.head - 1,
        )),
    }
}

fn compare_str<'a>(
    rb1: &mut ReadBuffer<'a>,
    rb2: &mut ReadBuffer<'a>,
) -> Result<Option<Ordering>, ReadBonErr> {
    rb1.head += 1;
    rb2.head += 1;
    let t1 = rb1.get_type()?;
    let t2 = rb2.get_type()?;
    let len1 = match t1 {
        42..107 => (t1 - 42) as usize,
        107 => {
            rb1.head += 1;
            rb1.bytes.get_u8() as usize
        }
        108 => {
            rb1.head += 2;
            rb1.bytes.get_u16_le() as usize
        }
        109 => {
            rb1.head += 4;
            rb1.bytes.get_u32_le() as usize
        }
        110 => {
            rb1.head += 6;
            rb1.bytes.get_u16_le() as usize + (rb1.bytes.get_u32_le() * 0x10000) as usize
        }
        _ => {
            return Err(ReadBonErr::type_no_match(
                "string".to_string(),
                t1,
                rb1.head - 1,
            ))
        }
    };

    let len2 = match t2 {
        42..107 => (t2 - 42) as usize,
        107 => {
            rb2.head += 1;
            rb2.bytes.get_u8() as usize
        }
        108 => {
            rb2.head += 2;
            rb2.bytes.get_u16_le() as usize
        }
        109 => {
            rb2.head += 4;
            rb2.bytes.get_u32_le() as usize
        }
        110 => {
            rb2.head += 6;
            rb2.bytes.get_u16_le() as usize + (rb2.bytes.get_u32_le() * 0x10000) as usize
        }
        _ => {
            return Err(ReadBonErr::type_no_match(
                "string".to_string(),
                t2,
                rb2.head - 1,
            ))
        }
    };

    rb1.head += len1;
    rb2.head += len2;
    // println!("head1:{}, len1:{},head2:{}, len2:{}, t1:{},  byte1_len:{}, byte1:{:?}", head1, len1, head2, len2, t1, rb1.bytes.len(), rb1.bytes);
    // println!("{:?}, {:?}", &rb1.bytes[rb1.head - len1..rb1.head], &rb2.bytes[rb2.head - len2..rb2.head]);
    // println!("rb1 start = {:?}, rb1 end = {:?}, rb1 = {:?}", rb1.head - len1, rb1.head, rb1.bytes);
    // println!("rb2 start = {:?}, rb2 end = {:?}, rb2 = {:?}", rb2.head - len2, rb2.head, rb2.bytes);
    let dst1 = rb1.bytes.copy_to_bytes(len1);
    let dst2 = rb2.bytes.copy_to_bytes(len2);

    // rb1.bytes[rb1.head - len1..rb1.head].partial_cmp(&rb2.bytes[rb2.head - len2..rb2.head])
    Ok(dst1.partial_cmp(&dst2))
}

fn compare_bin<'a>(rb1: &mut ReadBuffer<'a>, rb2: &mut ReadBuffer<'a>) -> Result<Option<Ordering>, ReadBonErr> {
    rb1.head += 1;
    rb2.head += 1;
    let t1 = rb1.get_type()?;
    let t2 = rb2.get_type()?;
    let len1 = match t1 {
        111..176 => (t1 - 111) as usize,
        176 => {
            rb1.head += 1;
            rb1.bytes.get_u8() as usize
        }
        177 => {
            rb1.head += 2;
            rb1.bytes.get_u16_le() as usize
        }
        178 => {
            rb1.head += 4;
            rb1.bytes.get_u32_le() as usize
        }
        179 => {
            rb1.head += 6;
            rb1.bytes.get_u16_le() as usize + (rb1.bytes.get_u32_le() * 0x10000) as usize
        }
        _ => {
            return Err(ReadBonErr::Other(format!("t1 is not bin:{}", t1)))
        }
    };

    let len2 = match t2 {
        111..176 => (t2 - 111) as usize,
        176 => {
            rb2.head += 1;
            rb2.bytes.get_u8() as usize
        }
        177 => {
            rb2.head += 2;
            rb2.bytes.get_u16_le() as usize
        }
        178 => {
            rb2.head += 4;
            rb2.bytes.get_u32_le() as usize
        }
        179 => {
            rb2.head += 6;
            rb2.bytes.get_u16_le() as usize + (rb2.bytes.get_u32_le() * 0x10000) as usize
        }
        _ => {
            return Err(ReadBonErr::Other(format!("t2 is not bin:{}", t2)))
        }
    };

    rb1.head += len1;
    rb2.head += len2;

    let dst1 = rb1.bytes.copy_to_bytes(len1);
    let dst2 = rb2.bytes.copy_to_bytes(len2);

    Ok(dst1.partial_cmp(&dst2))
    // rb1.bytes[rb1.head - len1..rb1.head].partial_cmp(&rb2.bytes[rb2.head - len2..rb2.head])
}

fn compare_contain<'a>(rb1: &mut ReadBuffer<'a>, rb2: &mut ReadBuffer<'a>) -> Result<Option<Ordering>, ReadBonErr> {
    let t1 = rb1.get_type()?;
    let t2 = rb2.get_type()?;
    rb1.head += 1;
    rb2.head += 1;
    match t1 {
        180..245 => (t1 - 180) as usize,
        245 => {
            rb1.head += 1;
            rb1.bytes.get_u8() as usize
        }
        246 => {
            rb1.head += 2;
            rb1.bytes.get_u16_le() as usize
        }
        247 => {
            rb1.head += 4;
            rb1.bytes.get_u32_le() as usize
        }
        248 => {
            rb1.head += 6;
            rb1.bytes.get_u16_le() as usize + (rb1.bytes.get_u32_le() * 0x10000) as usize
        }
        _ => {
            return Err(ReadBonErr::Other(format!("Invalid contain type t1: {}", t1)));
        }
    };

    match t2 {
        180..245 => (t2 - 180) as usize,
        245 => {
            rb2.head += 1;
            rb2.bytes.get_u8() as usize
        }
        246 => {
            rb2.head += 2;
            rb2.bytes.get_u16_le() as usize
        }
        247 => {
            rb2.head += 4;
            rb2.bytes.get_u32_le() as usize
        }
        248 => {
            rb2.head += 6;
            rb2.bytes.get_u16_le() as usize + (rb2.bytes.get_u32_le() * 0x10000) as usize
        }
        _ => {
            return Err(ReadBonErr::Other(format!("Invalid contain type t2: {}", t2)));
        }
    };

    return Ok(Some(Ordering::Equal));
}

pub enum EnumType {
    Void,
    Bool,
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    F32,
    F64,
    Str(u64),
    Bin(u64),
    Arr(u32, u64),
    Map(u32, u64),
    Struct(u64),
}
#[derive(Debug)]
pub enum EnumValue {
    Void,
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    U128(u128),
    I8(i8),
    I16(i16),
    I32(i32),
    I64(i64),
    I128(i128),
    F32(f32),
    F64(f64),
    Str(String),
    Bin(Vec<u8>),
    Arr(Arc<Vec<EnumValue>>),
    Map(HashMap<Arc<EnumValue>, Arc<EnumValue>>),
    Struct(Arc<StructValue>),
}

#[derive(Debug)]
pub struct StructValue {
    pub hash: u32,
    pub fields: Vec<FieldValue>,
}

#[derive(Debug)]
pub struct FieldValue {
    pub name: String,
    pub fvalue: EnumValue,
}

fn move_part(bytes: &mut Vec<u8>, range: Range<usize>, offset: usize) {
    unsafe {
        let len = bytes.len();
        let dl = range.end - range.start;
        if len < offset + dl {
            bytes.set_len(offset + dl);
        }
        let src = bytes.as_mut_ptr();
        src.wrapping_offset(range.start as isize)
            .copy_to(src.wrapping_offset(offset as isize), dl)
    }
}

#[cfg(test)]
extern crate rand;
#[cfg(test)]
extern crate test;
#[cfg(test)]
mod tests {
    use super::*;
    use rand::prelude::*;
    use test::Bencher;

    macro_rules! test_number {
        ($x: ty, $func: ident, $r: ident, $w: ident) => {
            #[test]
            fn $func() -> Result<(), Box<dyn Error>> {
                let cases: Vec<$x> = (0..200).map(|_| thread_rng().gen::<$x>()).collect();
                for case in cases {
                    let mut buf = WriteBuffer::with_bytes(vec![], 0);
                    buf.$w(case);
                    let mut read_buf = ReadBuffer::new(buf.get_byte(), 0);
                    assert_eq!(read_buf.$r()?, case);
                }
                Ok(())
            }
        };
    }

    test_number!(bool, test_bool, read_bool, write_bool);
    test_number!(u8, test_u8, read_u8, write_u8);
    test_number!(u16, test_u16, read_u16, write_u16);
    test_number!(u32, test_u32, read_u32, write_u32);
    test_number!(u64, test_u64, read_u64, write_u64);
    test_number!(u128, test_u128, read_u128, write_u128);
    test_number!(i8, test_i8, read_i8, write_i8);
    test_number!(i16, test_i16, read_i16, write_i16);
    test_number!(i32, test_i32, read_i32, write_i32);
    test_number!(i64, test_i64, read_i64, write_i64);
    test_number!(i128, test_i128, read_i128, write_i128);
    test_number!(f32, test_f32, read_f32, write_f32);
    test_number!(f64, test_f64, read_f64, write_f64);

    macro_rules! test_utf8 {
        ($func: ident, $size: expr) => {
            #[test]
            fn $func() -> Result<(), Box<dyn Error>> {
                let mut s = String::new();
                (0..$size).for_each(|_| s.push(thread_rng().gen::<char>()));
                let mut buf = WriteBuffer::with_bytes(vec![], 0);
                buf.write_utf8(&s);
                let mut read_buf = ReadBuffer::new(buf.get_byte(), 0);
                assert_eq!(read_buf.read_utf8()?, s);

                Ok(())
            }
        };
    }

    test_utf8!(test_utf8_10, 10);
    test_utf8!(test_utf8_100, 100);
    test_utf8!(test_utf8_1000, 1000);
    test_utf8!(test_utf8_10000, 10000);

    #[test]
    fn test_bin() -> Result<(), Box<dyn Error>> {
        let mut buf = WriteBuffer::with_bytes(vec![], 0);
        let arr: Vec<u8> = (0..1000).map(|_| thread_rng().gen::<u8>()).collect();
        buf.write_bin(&arr, 0..1000);

        let mut read_buf = ReadBuffer::new(buf.get_byte(), 0);
        assert_eq!(read_buf.read_bin()?, arr);

        Ok(())
    }

    //测试大小比较
    #[test]
    fn test_ord() {
        let mut buf1 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf2 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf3 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf4 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf5 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf6 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf7 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf8 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf9 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf10 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf11 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf12 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf13 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf14 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf15 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf16 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf17 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf18 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf19 = WriteBuffer::with_bytes(Vec::new(), 0);
        let mut buf20 = WriteBuffer::with_bytes(Vec::new(), 0);
        buf1.write_nil();
        buf2.write_bool(false);
        buf3.write_bool(true);
        buf4.write_f32(0.0);
        buf5.write_f32(1.0);
        buf6.write_f32(5.1);
        buf7.write_f32(5.6);
        buf8.write_f64(7.5);
        buf9.write_f64(3.4);
        buf10.write_i8(-1);
        buf11.write_i8(-1);
        buf12.write_i8(120);
        buf13.write_u32(10);
        buf14.write_i32(5);
        buf15.write_utf8("abcdefg");
        buf16.write_utf8("abcdefgh");
        buf17.write_utf8("abcddfgh");
        buf18.write_bin(&[5; 10], 0..10);
        buf19.write_bin(&[6; 5], 0..5);
        buf20.write_bin(&[6; 5], 0..5);
        let read_buf1 = ReadBuffer::new(buf1.get_byte(), 0);
        let read_buf2 = ReadBuffer::new(buf2.get_byte(), 0);
        let read_buf3 = ReadBuffer::new(buf3.get_byte(), 0);
        let read_buf4 = ReadBuffer::new(buf4.get_byte(), 0);
        let read_buf5 = ReadBuffer::new(buf5.get_byte(), 0);
        let read_buf6 = ReadBuffer::new(buf6.get_byte(), 0);
        let read_buf7 = ReadBuffer::new(buf7.get_byte(), 0);
        let read_buf8 = ReadBuffer::new(buf8.get_byte(), 0);
        let read_buf9 = ReadBuffer::new(buf9.get_byte(), 0);
        let read_buf10 = ReadBuffer::new(buf10.get_byte(), 0);
        let read_buf11 = ReadBuffer::new(buf11.get_byte(), 0);
        let read_buf12 = ReadBuffer::new(buf12.get_byte(), 0);
        let read_buf13 = ReadBuffer::new(buf13.get_byte(), 0);
        let read_buf14 = ReadBuffer::new(buf14.get_byte(), 0);
        let read_buf15 = ReadBuffer::new(buf15.get_byte(), 0);
        let read_buf16 = ReadBuffer::new(buf16.get_byte(), 0);
        let read_buf17 = ReadBuffer::new(buf17.get_byte(), 0);
        let read_buf18 = ReadBuffer::new(buf18.get_byte(), 0);
        let read_buf19 = ReadBuffer::new(buf19.get_byte(), 0);
        let read_buf20 = ReadBuffer::new(buf20.get_byte(), 0);
        assert_eq!(read_buf1 < read_buf2, true); //测试null, false
        assert_eq!(read_buf2 < read_buf3, true); //测试false, true
        assert_eq!(read_buf2 < read_buf4, true); //测试false, 0.0
        assert_eq!(read_buf4 < read_buf5, true); //测试0.0, 1.0
        assert_eq!(read_buf9 < read_buf7, true); //测试3.4, 5.6
        assert_eq!(read_buf6 < read_buf7, true); //测试5.1, 5.6
        assert_eq!(read_buf7 < read_buf8, true); //测试5.6, 7.5
        assert_eq!(read_buf10 < read_buf6, true); //测试-1, 5.6
        assert_eq!(read_buf10 == read_buf11, true); //测试-1, -1
        assert_eq!(read_buf11 < read_buf12, true); //测试-1, 200
        assert_eq!(read_buf12 > read_buf13, true); //测试 120, 10
        assert_eq!(read_buf13 > read_buf14, true); //测试 10, 5
        assert_eq!(read_buf1 < read_buf15, true); //测试 null, "abcdefg"
        assert_eq!(read_buf4 < read_buf15, true); //测试 0.0, "abcdefg"
        assert_eq!(read_buf6 < read_buf15, true); //测试 5.1, "abcdefg"
        assert_eq!(read_buf12 < read_buf15, true); //测试 200, "abcdefg"
        assert_eq!(read_buf13 < read_buf15, true); //测试 10, "abcdefg"
        assert_eq!(read_buf15 < read_buf16, true); //测试 "abcdefg", "abcdefgh"
        assert_eq!(read_buf15 > read_buf17, true); //测试 "abcdefg", "abcddfgh"
        assert_eq!(read_buf13 < read_buf18, true); //测试 10, &[5;10]
        assert_eq!(read_buf6 < read_buf18, true); //测试 5.1, &[5;10]
        assert_eq!(read_buf3 < read_buf18, true); //测试 true, &[5;10]
        assert_eq!(read_buf15 < read_buf18, true); //测试 "abcdefg", &[5;10]
        assert_eq!(read_buf18 < read_buf19, true); //测试 &[5;10], &[6;5]
        assert_eq!(read_buf19 == read_buf20, true); //测试 &[6;5], &[6;5]
        let mut buf1 = WriteBuffer::with_bytes(Vec::new(), 0);
        buf1.write_nil(); //null
        buf1.write_bool(false); //false
        buf1.write_bool(true); //true
        buf1.write_f32(0.0); //0.0
        buf1.write_f32(1.0); //1.0
        buf1.write_f32(5.0); //1.0
        buf1.write_u8(5);
        buf1.write_utf8("abc");
        let mut buf2 = WriteBuffer::with_bytes(Vec::new(), 0);
        buf2.write_utf8("acc");
        buf2.write_u8(5);
        assert_eq!(
            ReadBuffer::new(buf1.get_byte(), 0) < ReadBuffer::new(buf2.get_byte(), 0),
            true
        ); //测试 abc,5 < acc,5
    }

    #[test]
    fn test_container_cmp() {
        // struct xxx { x: bool, y: [&i32]}
        let mut w1 = WriteBuffer::new();
        let v = vec![1, 2, 3, 4, 100];
        w1.write_container(
            &v,
            |w1, e| {
                let hash = 0x12345678u32.to_le_bytes();
                w1.bytes.extend_from_slice(&hash);
                w1.tail += 4;
                w1.write_bool(true);
                for elem in e {
                    w1.write_i32(elem.clone());
                }
                Ok(())
            },
            None,
        );

        let mut w2 = WriteBuffer::new();
        let v2 = vec![1, 2, 3, 4, 99];
        w2.write_container(
            &v2,
            |w2, e| {
                let hash = 0x12345678u32.to_le_bytes();
                w2.bytes.extend_from_slice(&hash);
                w2.tail += 4;
                w2.write_bool(false);
                for elem in e {
                    w2.write_i32(elem.clone());
                }
                Ok(())
            },
            None,
        );

        assert!(w1 > w2);
    }

    macro_rules! bench_container_cmp {
        ($size: expr, $func: ident) => {
            #[bench]
            fn $func(b: &mut Bencher) {
                let mut sort = vec![];
                (0..$size).for_each(|_| {
                    let mut w2 = WriteBuffer::new();
                    let v2 = thread_rng().gen::<u32>();
                    w2.write_container(
                        &v2,
                        |w2, _| {
                            let hash = 0x12345678u32.to_le_bytes();
                            w2.bytes.extend_from_slice(&hash);
                            w2.tail += 4;
                            w2.write_u32(v2);

                            let mut s = String::new();
                            (0..20).for_each(|_| {
                                let ch = thread_rng().gen::<char>();
                                s.push(ch);
                            });
                            w2.write_utf8(&s);
                            Ok(())
                        },
                        None,
                    );
                    sort.push(w2);
                });
                b.iter(|| {
                    sort.sort();
                })
            }
        };
    }

    bench_container_cmp!(1000, bench_container_cmp_1000);
    bench_container_cmp!(10000, bench_container_cmp_10000);
    bench_container_cmp!(100000, bench_container_cmp_100000);

    macro_rules! bench_number {
        ($x:ty, $func:ident, $r: ident, $w: ident) => {
            #[bench]
            fn $func(b: &mut Bencher) {
                let v: $x = random();
                b.iter(|| {
                    let mut buf = WriteBuffer::with_bytes(vec![], 0);
                    buf.$w(v);
                    let mut read_buf = ReadBuffer::new(buf.get_byte(), 0);
                    read_buf.$r().unwrap();
                });
            }
        };
    }

    bench_number!(bool, bench_bool, read_bool, write_bool);
    bench_number!(u8, bench_u8, read_u8, write_u8);
    bench_number!(u16, bench_u16, read_u16, write_u16);
    bench_number!(u32, bench_u32, read_u32, write_u32);
    bench_number!(u64, bench_u64, read_u64, write_u64);
    bench_number!(u128, bench_u128, read_u128, write_u128);
    bench_number!(i8, bench_i8, read_i8, write_i8);
    bench_number!(i16, bench_i16, read_i16, write_i16);
    bench_number!(i32, bench_i32, read_i32, write_i32);
    bench_number!(i64, bench_i64, read_i64, write_i64);
    bench_number!(i128, bench_i128, read_i128, write_i128);
    bench_number!(f32, bench_f32, read_f32, write_f32);
    bench_number!(f64, bench_f64, read_f64, write_f64);

    macro_rules! bench_utf8 {
        ($size: expr, $func: ident) => {
            #[bench]
            fn $func(b: &mut Bencher) {
                let mut s = String::new();
                (0..$size).for_each(|_| s.push(thread_rng().gen::<char>()));
                b.iter(|| {
                    let mut buf = WriteBuffer::with_bytes(vec![], 0);
                    buf.write_utf8(&s);
                    let mut read_buf = ReadBuffer::new(buf.get_byte(), 0);
                    read_buf.read_utf8().unwrap();
                });
            }
        };
    }

    bench_utf8!(200, bench_utf8_small);
    bench_utf8!(2000, bench_utf8_median);
    bench_utf8!(20000, bench_utf8_large);
}
