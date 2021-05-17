use pin_project::pin_project;
# [pin (__private (project = Proj , project_ref = ProjRef , project_replace = ProjOwn))]
enum Enum<T, U> {
    Struct {
        #[pin]
        pinned: T,
        unpinned: U,
    },
    Tuple(#[pin] T, U),
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
enum Proj<'pin, T, U>
where
    Enum<T, U>: 'pin,
{
    Struct {
        pinned: ::pin_project::__private::Pin<&'pin mut (T)>,
        unpinned: &'pin mut (U),
    },
    Tuple(::pin_project::__private::Pin<&'pin mut (T)>, &'pin mut (U)),
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
enum ProjRef<'pin, T, U>
where
    Enum<T, U>: 'pin,
{
    Struct {
        pinned: ::pin_project::__private::Pin<&'pin (T)>,
        unpinned: &'pin (U),
    },
    Tuple(::pin_project::__private::Pin<&'pin (T)>, &'pin (U)),
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
enum ProjOwn<T, U> {
    Struct {
        pinned: ::pin_project::__private::PhantomData<T>,
        unpinned: U,
    },
    Tuple(::pin_project::__private::PhantomData<T>, U),
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
        fn project<'pin>(self: ::pin_project::__private::Pin<&'pin mut Self>) -> Proj<'pin, T, U> {
            unsafe {
                match self.get_unchecked_mut() {
                    Self::Struct { pinned, unpinned } => Proj::Struct {
                        pinned: ::pin_project::__private::Pin::new_unchecked(pinned),
                        unpinned,
                    },
                    Self::Tuple(_0, _1) => {
                        Proj::Tuple(::pin_project::__private::Pin::new_unchecked(_0), _1)
                    }
                    Self::Unit => Proj::Unit,
                }
            }
        }
        #[allow(clippy::missing_const_for_fn)]
        fn project_ref<'pin>(
            self: ::pin_project::__private::Pin<&'pin Self>,
        ) -> ProjRef<'pin, T, U> {
            unsafe {
                match self.get_ref() {
                    Self::Struct { pinned, unpinned } => ProjRef::Struct {
                        pinned: ::pin_project::__private::Pin::new_unchecked(pinned),
                        unpinned,
                    },
                    Self::Tuple(_0, _1) => {
                        ProjRef::Tuple(::pin_project::__private::Pin::new_unchecked(_0), _1)
                    }
                    Self::Unit => ProjRef::Unit,
                }
            }
        }
        fn project_replace(
            self: ::pin_project::__private::Pin<&mut Self>,
            __replacement: Self,
        ) -> ProjOwn<T, U> {
            unsafe {
                let __self_ptr: *mut Self = self.get_unchecked_mut();
                let __guard = ::pin_project::__private::UnsafeOverwriteGuard {
                    target: __self_ptr,
                    value: ::pin_project::__private::ManuallyDrop::new(__replacement),
                };
                match &mut *__self_ptr {
                    Self::Struct { pinned, unpinned } => {
                        let __result = ProjOwn::Struct {
                            pinned: ::pin_project::__private::PhantomData,
                            unpinned: ::pin_project::__private::ptr::read(unpinned),
                        };
                        {
                            let __guard = ::pin_project::__private::UnsafeDropInPlaceGuard(pinned);
                        }
                        __result
                    }
                    Self::Tuple(_0, _1) => {
                        let __result = ProjOwn::Tuple(
                            ::pin_project::__private::PhantomData,
                            ::pin_project::__private::ptr::read(_1),
                        );
                        {
                            let __guard = ::pin_project::__private::UnsafeDropInPlaceGuard(_0);
                        }
                        __result
                    }
                    Self::Unit => {
                        let __result = ProjOwn::Unit;
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
