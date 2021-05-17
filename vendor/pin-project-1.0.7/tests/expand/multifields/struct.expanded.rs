use pin_project::pin_project;
#[pin(__private(project_replace))]
struct Struct<T, U> {
    #[pin]
    pinned1: T,
    #[pin]
    pinned2: T,
    unpinned1: U,
    unpinned2: U,
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
        pinned1: ::pin_project::__private::Pin<&'pin mut (T)>,
        pinned2: ::pin_project::__private::Pin<&'pin mut (T)>,
        unpinned1: &'pin mut (U),
        unpinned2: &'pin mut (U),
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
        pinned1: ::pin_project::__private::Pin<&'pin (T)>,
        pinned2: ::pin_project::__private::Pin<&'pin (T)>,
        unpinned1: &'pin (U),
        unpinned2: &'pin (U),
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
    struct __StructProjectionOwned<T, U> {
        pinned1: ::pin_project::__private::PhantomData<T>,
        pinned2: ::pin_project::__private::PhantomData<T>,
        unpinned1: U,
        unpinned2: U,
    }
    impl<T, U> Struct<T, U> {
        fn project<'pin>(
            self: ::pin_project::__private::Pin<&'pin mut Self>,
        ) -> __StructProjection<'pin, T, U> {
            unsafe {
                let Self {
                    pinned1,
                    pinned2,
                    unpinned1,
                    unpinned2,
                } = self.get_unchecked_mut();
                __StructProjection {
                    pinned1: ::pin_project::__private::Pin::new_unchecked(pinned1),
                    pinned2: ::pin_project::__private::Pin::new_unchecked(pinned2),
                    unpinned1,
                    unpinned2,
                }
            }
        }
        #[allow(clippy::missing_const_for_fn)]
        fn project_ref<'pin>(
            self: ::pin_project::__private::Pin<&'pin Self>,
        ) -> __StructProjectionRef<'pin, T, U> {
            unsafe {
                let Self {
                    pinned1,
                    pinned2,
                    unpinned1,
                    unpinned2,
                } = self.get_ref();
                __StructProjectionRef {
                    pinned1: ::pin_project::__private::Pin::new_unchecked(pinned1),
                    pinned2: ::pin_project::__private::Pin::new_unchecked(pinned2),
                    unpinned1,
                    unpinned2,
                }
            }
        }
        fn project_replace(
            self: ::pin_project::__private::Pin<&mut Self>,
            __replacement: Self,
        ) -> __StructProjectionOwned<T, U> {
            unsafe {
                let __self_ptr: *mut Self = self.get_unchecked_mut();
                let __guard = ::pin_project::__private::UnsafeOverwriteGuard {
                    target: __self_ptr,
                    value: ::pin_project::__private::ManuallyDrop::new(__replacement),
                };
                let Self {
                    pinned1,
                    pinned2,
                    unpinned1,
                    unpinned2,
                } = &mut *__self_ptr;
                let __result = __StructProjectionOwned {
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
        }
    }
    #[forbid(unaligned_references, safe_packed_borrows)]
    fn __assert_not_repr_packed<T, U>(this: &Struct<T, U>) {
        let _ = &this.pinned1;
        let _ = &this.pinned2;
        let _ = &this.unpinned1;
        let _ = &this.unpinned2;
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
        __field1: T,
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
