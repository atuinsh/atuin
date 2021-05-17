use pin_project::pin_project;
# [pin (__private (project = EnumProj , project_ref = EnumProjRef , project_replace = EnumProjOwn))]
enum Enum<T, U> {
    Struct {
        #[pin]
        pinned1: T,
        #[pin]
        pinned2: T,
        unpinned1: U,
        unpinned2: U,
    },
    Tuple(#[pin] T, #[pin] T, U, U),
    Unit,
}
#[allow(box_pointers)]
#[allow(deprecated)]
#[allow(explicit_outlives_requirements)]
#[allow(single_use_lifetimes)]
#[allow(unreachable_pub)]
#[allow(clippy::unknown_clippy_lints)]
#[allow(clippy::pattern_type_mismatch)]
#[allow(clippy::redundant_pub_crate)]
#[allow(dead_code)]
#[allow(clippy::mut_mut)]
#[allow(clippy::type_repetition_in_bounds)]
enum EnumProj<'pin, T, U>
where
    Enum<T, U>: 'pin,
{
    Struct {
        pinned1: ::pin_project::__private::Pin<&'pin mut (T)>,
        pinned2: ::pin_project::__private::Pin<&'pin mut (T)>,
        unpinned1: &'pin mut (U),
        unpinned2: &'pin mut (U),
    },
    Tuple(
        ::pin_project::__private::Pin<&'pin mut (T)>,
        ::pin_project::__private::Pin<&'pin mut (T)>,
        &'pin mut (U),
        &'pin mut (U),
    ),
    Unit,
}
#[allow(box_pointers)]
#[allow(deprecated)]
#[allow(explicit_outlives_requirements)]
#[allow(single_use_lifetimes)]
#[allow(unreachable_pub)]
#[allow(clippy::unknown_clippy_lints)]
#[allow(clippy::pattern_type_mismatch)]
#[allow(clippy::redundant_pub_crate)]
#[allow(dead_code)]
#[allow(clippy::ref_option_ref)]
#[allow(clippy::type_repetition_in_bounds)]
enum EnumProjRef<'pin, T, U>
where
    Enum<T, U>: 'pin,
{
    Struct {
        pinned1: ::pin_project::__private::Pin<&'pin (T)>,
        pinned2: ::pin_project::__private::Pin<&'pin (T)>,
        unpinned1: &'pin (U),
        unpinned2: &'pin (U),
    },
    Tuple(
        ::pin_project::__private::Pin<&'pin (T)>,
        ::pin_project::__private::Pin<&'pin (T)>,
        &'pin (U),
        &'pin (U),
    ),
    Unit,
}
#[allow(box_pointers)]
#[allow(deprecated)]
#[allow(explicit_outlives_requirements)]
#[allow(single_use_lifetimes)]
#[allow(unreachable_pub)]
#[allow(clippy::unknown_clippy_lints)]
#[allow(clippy::pattern_type_mismatch)]
#[allow(clippy::redundant_pub_crate)]
#[allow(dead_code)]
#[allow(variant_size_differences)]
#[allow(clippy::large_enum_variant)]
enum EnumProjOwn<T, U> {
    Struct {
        pinned1: ::pin_project::__private::PhantomData<T>,
        pinned2: ::pin_project::__private::PhantomData<T>,
        unpinned1: U,
        unpinned2: U,
    },
    Tuple(
        ::pin_project::__private::PhantomData<T>,
        ::pin_project::__private::PhantomData<T>,
        U,
        U,
    ),
    Unit,
}
#[allow(box_pointers)]
#[allow(deprecated)]
#[allow(explicit_outlives_requirements)]
#[allow(single_use_lifetimes)]
#[allow(unreachable_pub)]
#[allow(clippy::unknown_clippy_lints)]
#[allow(clippy::pattern_type_mismatch)]
#[allow(clippy::redundant_pub_crate)]
#[allow(clippy::semicolon_if_nothing_returned)]
#[allow(clippy::used_underscore_binding)]
const _: () = {
    impl<T, U> Enum<T, U> {
        fn project<'pin>(
            self: ::pin_project::__private::Pin<&'pin mut Self>,
        ) -> EnumProj<'pin, T, U> {
            unsafe {
                match self.get_unchecked_mut() {
                    Self::Struct {
                        pinned1,
                        pinned2,
                        unpinned1,
                        unpinned2,
                    } => EnumProj::Struct {
                        pinned1: ::pin_project::__private::Pin::new_unchecked(pinned1),
                        pinned2: ::pin_project::__private::Pin::new_unchecked(pinned2),
                        unpinned1,
                        unpinned2,
                    },
                    Self::Tuple(_0, _1, _2, _3) => EnumProj::Tuple(
                        ::pin_project::__private::Pin::new_unchecked(_0),
                        ::pin_project::__private::Pin::new_unchecked(_1),
                        _2,
                        _3,
                    ),
                    Self::Unit => EnumProj::Unit,
                }
            }
        }
        #[allow(clippy::missing_const_for_fn)]
        fn project_ref<'pin>(
            self: ::pin_project::__private::Pin<&'pin Self>,
        ) -> EnumProjRef<'pin, T, U> {
            unsafe {
                match self.get_ref() {
                    Self::Struct {
                        pinned1,
                        pinned2,
                        unpinned1,
                        unpinned2,
                    } => EnumProjRef::Struct {
                        pinned1: ::pin_project::__private::Pin::new_unchecked(pinned1),
                        pinned2: ::pin_project::__private::Pin::new_unchecked(pinned2),
                        unpinned1,
                        unpinned2,
                    },
                    Self::Tuple(_0, _1, _2, _3) => EnumProjRef::Tuple(
                        ::pin_project::__private::Pin::new_unchecked(_0),
                        ::pin_project::__private::Pin::new_unchecked(_1),
                        _2,
                        _3,
                    ),
                    Self::Unit => EnumProjRef::Unit,
                }
            }
        }
        fn project_replace(
            self: ::pin_project::__private::Pin<&mut Self>,
            __replacement: Self,
        ) -> EnumProjOwn<T, U> {
            unsafe {
                let __self_ptr: *mut Self = self.get_unchecked_mut();
                let __guard = ::pin_project::__private::UnsafeOverwriteGuard {
                    target: __self_ptr,
                    value: ::pin_project::__private::ManuallyDrop::new(__replacement),
                };
                match &mut *__self_ptr {
                    Self::Struct {
                        pinned1,
                        pinned2,
                        unpinned1,
                        unpinned2,
                    } => {
                        let __result = EnumProjOwn::Struct {
                            pinned1: ::pin_project::__private::PhantomData,
                            pinned2: ::pin_project::__private::PhantomData,
                            unpinned1: ::pin_project::__private::ptr::read(unpinned1),
                            unpinned2: ::pin_project::__private::ptr::read(unpinned2),
                        };
                        {
                            let __guard = ::pin_project::__private::UnsafeDropInPlaceGuard(pinned2);
                            let __guard = ::pin_project::__private::UnsafeDropInPlaceGuard(pinned1);
                        }
                        __result
                    }
                    Self::Tuple(_0, _1, _2, _3) => {
                        let __result = EnumProjOwn::Tuple(
                            ::pin_project::__private::PhantomData,
                            ::pin_project::__private::PhantomData,
                            ::pin_project::__private::ptr::read(_2),
                            ::pin_project::__private::ptr::read(_3),
                        );
                        {
                            let __guard = ::pin_project::__private::UnsafeDropInPlaceGuard(_1);
                            let __guard = ::pin_project::__private::UnsafeDropInPlaceGuard(_0);
                        }
                        __result
                    }
                    Self::Unit => {
                        let __result = EnumProjOwn::Unit;
                        {}
                        __result
                    }
                }
            }
        }
    }
    #[allow(missing_debug_implementations)]
    struct __Enum<'pin, T, U> {
        __pin_project_use_generics: ::pin_project::__private::AlwaysUnpin<
            'pin,
            (
                ::pin_project::__private::PhantomData<T>,
                ::pin_project::__private::PhantomData<U>,
            ),
        >,
        __field0: T,
        __field1: T,
        __field2: T,
        __field3: T,
    }
    impl<'pin, T, U> ::pin_project::__private::Unpin for Enum<T, U> where
        __Enum<'pin, T, U>: ::pin_project::__private::Unpin
    {
    }
    #[doc(hidden)]
    unsafe impl<'pin, T, U> ::pin_project::UnsafeUnpin for Enum<T, U> where
        __Enum<'pin, T, U>: ::pin_project::__private::Unpin
    {
    }
    trait EnumMustNotImplDrop {}
    #[allow(clippy::drop_bounds, drop_bounds)]
    impl<T: ::pin_project::__private::Drop> EnumMustNotImplDrop for T {}
    impl<T, U> EnumMustNotImplDrop for Enum<T, U> {}
    #[doc(hidden)]
    impl<T, U> ::pin_project::__private::PinnedDrop for Enum<T, U> {
        unsafe fn drop(self: ::pin_project::__private::Pin<&mut Self>) {}
    }
};
fn main() {}
