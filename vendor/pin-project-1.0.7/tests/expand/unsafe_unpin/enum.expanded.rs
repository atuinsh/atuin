use pin_project::{pin_project, UnsafeUnpin};
# [pin (__private (UnsafeUnpin , project = EnumProj , project_ref = EnumProjRef))]
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
enum EnumProj<'pin, T, U>
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
enum EnumProjRef<'pin, T, U>
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
#[allow(clippy::semicolon_if_nothing_returned)]
#[allow(clippy::used_underscore_binding)]
const _: () = {
    impl<T, U> Enum<T, U> {
        fn project<'pin>(
            self: ::pin_project::__private::Pin<&'pin mut Self>,
        ) -> EnumProj<'pin, T, U> {
            unsafe {
                match self.get_unchecked_mut() {
                    Self::Struct { pinned, unpinned } => EnumProj::Struct {
                        pinned: ::pin_project::__private::Pin::new_unchecked(pinned),
                        unpinned,
                    },
                    Self::Tuple(_0, _1) => {
                        EnumProj::Tuple(::pin_project::__private::Pin::new_unchecked(_0), _1)
                    }
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
                    Self::Struct { pinned, unpinned } => EnumProjRef::Struct {
                        pinned: ::pin_project::__private::Pin::new_unchecked(pinned),
                        unpinned,
                    },
                    Self::Tuple(_0, _1) => {
                        EnumProjRef::Tuple(::pin_project::__private::Pin::new_unchecked(_0), _1)
                    }
                    Self::Unit => EnumProjRef::Unit,
                }
            }
        }
    }
    impl<'pin, T, U> ::pin_project::__private::Unpin for Enum<T, U> where
        ::pin_project::__private::Wrapper<'pin, Self>: ::pin_project::UnsafeUnpin
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
unsafe impl<T: Unpin, U> UnsafeUnpin for Enum<T, U> {}
fn main() {}
