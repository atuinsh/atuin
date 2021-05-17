use core::char;
use core::mem::{self, ManuallyDrop};

use crate::convert::traits::WasmAbi;
use crate::convert::{FromWasmAbi, IntoWasmAbi, RefFromWasmAbi};
use crate::convert::{OptionFromWasmAbi, OptionIntoWasmAbi, ReturnWasmAbi};
use crate::{Clamped, JsValue};

unsafe impl WasmAbi for () {}

#[repr(C)]
pub struct WasmOptionalI32 {
    pub present: u32,
    pub value: i32,
}

unsafe impl WasmAbi for WasmOptionalI32 {}

#[repr(C)]
pub struct WasmOptionalU32 {
    pub present: u32,
    pub value: u32,
}

unsafe impl WasmAbi for WasmOptionalU32 {}

#[repr(C)]
pub struct WasmOptionalF32 {
    pub present: u32,
    pub value: f32,
}

unsafe impl WasmAbi for WasmOptionalF32 {}

#[repr(C)]
pub struct WasmOptionalF64 {
    pub present: u32,
    pub value: f64,
}

unsafe impl WasmAbi for WasmOptionalF64 {}

#[repr(C)]
pub struct Wasm64 {
    pub low: u32,
    pub high: u32,
}

unsafe impl WasmAbi for Wasm64 {}

#[repr(C)]
pub struct WasmOptional64 {
    pub present: u32,
    pub low: u32,
    pub high: u32,
}

unsafe impl WasmAbi for WasmOptional64 {}

macro_rules! type_wasm_native {
    ($($t:tt as $c:tt => $r:tt)*) => ($(
        impl IntoWasmAbi for $t {
            type Abi = $c;

            #[inline]
            fn into_abi(self) -> $c { self as $c }
        }

        impl FromWasmAbi for $t {
            type Abi = $c;

            #[inline]
            unsafe fn from_abi(js: $c) -> Self { js as $t }
        }

        impl IntoWasmAbi for Option<$t> {
            type Abi = $r;

            #[inline]
            fn into_abi(self) -> $r {
                match self {
                    None => $r {
                        present: 0,
                        value: 0 as $c,
                    },
                    Some(me) => $r {
                        present: 1,
                        value: me as $c,
                    },
                }
            }
        }

        impl FromWasmAbi for Option<$t> {
            type Abi = $r;

            #[inline]
            unsafe fn from_abi(js: $r) -> Self {
                if js.present == 0 {
                    None
                } else {
                    Some(js.value as $t)
                }
            }
        }
    )*)
}

type_wasm_native!(
    i32 as i32 => WasmOptionalI32
    isize as i32 => WasmOptionalI32
    u32 as u32 => WasmOptionalU32
    usize as u32 => WasmOptionalU32
    f32 as f32 => WasmOptionalF32
    f64 as f64 => WasmOptionalF64
);

macro_rules! type_abi_as_u32 {
    ($($t:tt)*) => ($(
        impl IntoWasmAbi for $t {
            type Abi = u32;

            #[inline]
            fn into_abi(self) -> u32 { self as u32 }
        }

        impl FromWasmAbi for $t {
            type Abi = u32;

            #[inline]
            unsafe fn from_abi(js: u32) -> Self { js as $t }
        }

        impl OptionIntoWasmAbi for $t {
            #[inline]
            fn none() -> u32 { 0x00FF_FFFFu32 }
        }

        impl OptionFromWasmAbi for $t {
            #[inline]
            fn is_none(js: &u32) -> bool { *js == 0x00FF_FFFFu32 }
        }
    )*)
}

type_abi_as_u32!(i8 u8 i16 u16);

macro_rules! type_64 {
    ($($t:tt)*) => ($(
        impl IntoWasmAbi for $t {
            type Abi = Wasm64;

            #[inline]
            fn into_abi(self) -> Wasm64 {
                Wasm64 {
                    low: self as u32,
                    high: (self >> 32) as u32,
                }
            }
        }

        impl FromWasmAbi for $t {
            type Abi = Wasm64;

            #[inline]
            unsafe fn from_abi(js: Wasm64) -> $t {
                $t::from(js.low) | ($t::from(js.high) << 32)
            }
        }

        impl IntoWasmAbi for Option<$t> {
            type Abi = WasmOptional64;

            #[inline]
            fn into_abi(self) -> WasmOptional64 {
                match self {
                    None => WasmOptional64 {
                        present: 0,
                        low: 0 as u32,
                        high: 0 as u32,
                    },
                    Some(me) => WasmOptional64 {
                        present: 1,
                        low: me as u32,
                        high: (me >> 32) as u32,
                    },
                }
            }
        }

        impl FromWasmAbi for Option<$t> {
            type Abi = WasmOptional64;

            #[inline]
            unsafe fn from_abi(js: WasmOptional64) -> Self {
                if js.present == 0 {
                    None
                } else {
                    Some($t::from(js.low) | ($t::from(js.high) << 32))
                }
            }
        }
    )*)
}

type_64!(i64 u64);

impl IntoWasmAbi for bool {
    type Abi = u32;

    #[inline]
    fn into_abi(self) -> u32 {
        self as u32
    }
}

impl FromWasmAbi for bool {
    type Abi = u32;

    #[inline]
    unsafe fn from_abi(js: u32) -> bool {
        js != 0
    }
}

impl OptionIntoWasmAbi for bool {
    #[inline]
    fn none() -> u32 {
        0x00FF_FFFFu32
    }
}

impl OptionFromWasmAbi for bool {
    #[inline]
    fn is_none(js: &u32) -> bool {
        *js == 0x00FF_FFFFu32
    }
}

impl IntoWasmAbi for char {
    type Abi = u32;

    #[inline]
    fn into_abi(self) -> u32 {
        self as u32
    }
}

impl FromWasmAbi for char {
    type Abi = u32;

    #[inline]
    unsafe fn from_abi(js: u32) -> char {
        char::from_u32_unchecked(js)
    }
}

impl OptionIntoWasmAbi for char {
    #[inline]
    fn none() -> u32 {
        0x00FF_FFFFu32
    }
}

impl OptionFromWasmAbi for char {
    #[inline]
    fn is_none(js: &u32) -> bool {
        *js == 0x00FF_FFFFu32
    }
}

impl<T> IntoWasmAbi for *const T {
    type Abi = u32;

    #[inline]
    fn into_abi(self) -> u32 {
        self as u32
    }
}

impl<T> FromWasmAbi for *const T {
    type Abi = u32;

    #[inline]
    unsafe fn from_abi(js: u32) -> *const T {
        js as *const T
    }
}

impl<T> IntoWasmAbi for *mut T {
    type Abi = u32;

    #[inline]
    fn into_abi(self) -> u32 {
        self as u32
    }
}

impl<T> FromWasmAbi for *mut T {
    type Abi = u32;

    #[inline]
    unsafe fn from_abi(js: u32) -> *mut T {
        js as *mut T
    }
}

impl IntoWasmAbi for JsValue {
    type Abi = u32;

    #[inline]
    fn into_abi(self) -> u32 {
        let ret = self.idx;
        mem::forget(self);
        ret
    }
}

impl FromWasmAbi for JsValue {
    type Abi = u32;

    #[inline]
    unsafe fn from_abi(js: u32) -> JsValue {
        JsValue::_new(js)
    }
}

impl<'a> IntoWasmAbi for &'a JsValue {
    type Abi = u32;

    #[inline]
    fn into_abi(self) -> u32 {
        self.idx
    }
}

impl RefFromWasmAbi for JsValue {
    type Abi = u32;
    type Anchor = ManuallyDrop<JsValue>;

    #[inline]
    unsafe fn ref_from_abi(js: u32) -> Self::Anchor {
        ManuallyDrop::new(JsValue::_new(js))
    }
}

impl<T: OptionIntoWasmAbi> IntoWasmAbi for Option<T> {
    type Abi = T::Abi;

    #[inline]
    fn into_abi(self) -> T::Abi {
        match self {
            None => T::none(),
            Some(me) => me.into_abi(),
        }
    }
}

impl<T: OptionFromWasmAbi> FromWasmAbi for Option<T> {
    type Abi = T::Abi;

    #[inline]
    unsafe fn from_abi(js: T::Abi) -> Self {
        if T::is_none(&js) {
            None
        } else {
            Some(T::from_abi(js))
        }
    }
}

impl<T: IntoWasmAbi> IntoWasmAbi for Clamped<T> {
    type Abi = T::Abi;

    #[inline]
    fn into_abi(self) -> Self::Abi {
        self.0.into_abi()
    }
}

impl<T: FromWasmAbi> FromWasmAbi for Clamped<T> {
    type Abi = T::Abi;

    #[inline]
    unsafe fn from_abi(js: T::Abi) -> Self {
        Clamped(T::from_abi(js))
    }
}

impl IntoWasmAbi for () {
    type Abi = ();

    #[inline]
    fn into_abi(self) {
        self
    }
}

impl<T: IntoWasmAbi> ReturnWasmAbi for Result<T, JsValue> {
    type Abi = T::Abi;

    #[inline]
    fn return_abi(self) -> Self::Abi {
        match self {
            Ok(v) => v.into_abi(),
            Err(e) => crate::throw_val(e),
        }
    }
}
