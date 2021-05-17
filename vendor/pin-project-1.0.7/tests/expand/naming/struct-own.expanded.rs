use pin_project::pin_project;
# [pin (__private (project_replace = ProjOwn))]
struct Struct<T, U> {
    #[pin]
    pinned: T,
    unpinned: U,
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
struct ProjOwn<T, U> {
    pinned: ::pin_project::__private::PhantomData<T>,
    unpinned: U,
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
    struct __StructProjection<'pin, T, U>
    where
        Struct<T, U>: 'pin,
    {
        pinned: ::pin_project::__private::Pin<&'pin mut (T)>,
        unpinned: &'pin mut (U),
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
    struct __StructProjectionRef<'pin, T, U>
    where
        Struct<T, U>: 'pin,
    {
        pinned: ::pin_project::__private::Pin<&'pin (T)>,
        unpinned: &'pin (U),
    }
    impl<T, U> Struct<T, U> {
        fn project<'pin>(
            self: ::pin_project::__private::Pin<&'pin mut Self>,
        ) -> __StructProjection<'pin, T, U> {
            unsafe {
                let Self { pinned, unpinned } = self.get_unchecked_mut();
                __StructProjection {
                    pinned: ::pin_project::__private::Pin::new_unchecked(pinned),
                    unpinned,
                }
            }
        }
        #[allow(clippy::missing_const_for_fn)]
        fn project_ref<'pin>(
            self: ::pin_project::__private::Pin<&'pin Self>,
        ) -> __StructProjectionRef<'pin, T, U> {
            unsafe {
                let Self { pinned, unpinned } = self.get_ref();
                __StructProjectionRef {
                    pinned: ::pin_project::__private::Pin::new_unchecked(pinned),
                    unpinned,
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
                let Self { pinned, unpinned } = &mut *__self_ptr;
                let __result = ProjOwn {
                    pinned: ::pin_project::__private::PhantomData,
                    unpinned: ::pin_project::__private::ptr::read(unpinned),
                };
                {
                    let __guard = ::pin_project::__private::UnsafeDropInPlaceGuard(pinned);
                }
                __result
            }
        }
    }
    #[forbid(unaligned_references, safe_packed_borrows)]
    fn __assert_not_repr_packed<T, U>(this: &Struct<T, U>) {
        let _ = &this.pinned;
        let _ = &this.unpinned;
    }
    #[allow(missing_debug_implementations)]
    struct __Struct<'pin, T, U> {
        __pin_project_use_generics: ::pin_project::__private::AlwaysUnpin<
            'pin,
            (
                ::pin_project::__private::PhantomData<T>,
                ::pin_project::__private::PhantomData<U>,
            ),
        >,
        __field0: T,
    }
    impl<'pin, T, U> ::pin_project::__private::Unpin for Struct<T, U> where
        __Struct<'pin, T, U>: ::pin_project::__private::Unpin
    {
    }
    #[doc(hidden)]
    unsafe impl<'pin, T, U> ::pin_project::UnsafeUnpin for Struct<T, U> where
        __Struct<'pin, T, U>: ::pin_project::__private::Unpin
    {
    }
    trait StructMustNotImplDrop {}
    #[allow(clippy::drop_bounds, drop_bounds)]
    impl<T: ::pin_project::__private::Drop> StructMustNotImplDrop for T {}
    impl<T, U> StructMustNotImplDrop for Struct<T, U> {}
    #[doc(hidden)]
    impl<T, U> ::pin_project::__private::PinnedDrop for Struct<T, U> {
        unsafe fn drop(self: ::pin_project::__private::Pin<&mut Self>) {}
    }
};
fn main() {}
