//! A lightweight version of [pin-project] written with declarative macros.
//!
//! # Examples
//!
//! [`pin_project!`] macro creates a projection type covering all the fields of struct.
//!
//! ```rust
//! use std::pin::Pin;
//!
//! use pin_project_lite::pin_project;
//!
//! pin_project! {
//!     struct Struct<T, U> {
//!         #[pin]
//!         pinned: T,
//!         unpinned: U,
//!     }
//! }
//!
//! impl<T, U> Struct<T, U> {
//!     fn method(self: Pin<&mut Self>) {
//!         let this = self.project();
//!         let _: Pin<&mut T> = this.pinned; // Pinned reference to the field
//!         let _: &mut U = this.unpinned; // Normal reference to the field
//!     }
//! }
//! ```
//!
//! To use [`pin_project!`] on enums, you need to name the projection type
//! returned from the method.
//!
//! ```rust
//! use std::pin::Pin;
//!
//! use pin_project_lite::pin_project;
//!
//! pin_project! {
//!     #[project = EnumProj]
//!     enum Enum<T, U> {
//!         Variant { #[pin] pinned: T, unpinned: U },
//!    }
//! }
//!
//! impl<T, U> Enum<T, U> {
//!     fn method(self: Pin<&mut Self>) {
//!         match self.project() {
//!             EnumProj::Variant { pinned, unpinned } => {
//!                 let _: Pin<&mut T> = pinned;
//!                 let _: &mut U = unpinned;
//!             }
//!         }
//!     }
//! }
//! ```
//!
//! # [pin-project] vs pin-project-lite
//!
//! Here are some similarities and differences compared to [pin-project].
//!
//! ## Similar: Safety
//!
//! pin-project-lite guarantees safety in much the same way as [pin-project].
//! Both are completely safe unless you write other unsafe code.
//!
//! ## Different: Minimal design
//!
//! This library does not tackle as expansive of a range of use cases as
//! [pin-project] does. If your use case is not already covered, please use
//! [pin-project].
//!
//! ## Different: No proc-macro related dependencies
//!
//! This is the **only** reason to use this crate. However, **if you already
//! have proc-macro related dependencies in your crate's dependency graph, there
//! is no benefit from using this crate.** (Note: There is almost no difference
//! in the amount of code generated between [pin-project] and pin-project-lite.)
//!
//! ## Different: No useful error messages
//!
//! This macro does not handle any invalid input. So error messages are not to
//! be useful in most cases. If you do need useful error messages, then upon
//! error you can pass the same input to [pin-project] to receive a helpful
//! description of the compile error.
//!
//! ## Different: No support for custom Drop implementation
//!
//! pin-project supports this by [`#[pinned_drop]`][pinned-drop].
//!
//! ## Different: No support for custom Unpin implementation
//!
//! pin-project supports this by [`UnsafeUnpin`][unsafe-unpin] and [`!Unpin`][not-unpin].
//!
//! ## Different: No support for tuple structs and tuple variants
//!
//! pin-project supports this.
//!
//! [not-unpin]: https://docs.rs/pin-project/1/pin_project/attr.pin_project.html#unpin
//! [pin-project]: https://github.com/taiki-e/pin-project
//! [pinned-drop]: https://docs.rs/pin-project/1/pin_project/attr.pin_project.html#pinned_drop
//! [unsafe-unpin]: https://docs.rs/pin-project/1/pin_project/attr.pin_project.html#unsafeunpin

#![no_std]
#![doc(test(
    no_crate_inject,
    attr(
        deny(warnings, rust_2018_idioms, single_use_lifetimes),
        allow(dead_code, unused_variables)
    )
))]
#![warn(future_incompatible, rust_2018_idioms, single_use_lifetimes, unreachable_pub)]
#![warn(clippy::all, clippy::default_trait_access)]

/// A macro that creates a projection type covering all the fields of struct.
///
/// This macro creates a projection type according to the following rules:
///
/// * For the field that uses `#[pin]` attribute, makes the pinned reference to the field.
/// * For the other fields, makes the unpinned reference to the field.
///
/// And the following methods are implemented on the original type:
///
/// ```rust
/// # use std::pin::Pin;
/// # type Projection<'a> = &'a ();
/// # type ProjectionRef<'a> = &'a ();
/// # trait Dox {
/// fn project(self: Pin<&mut Self>) -> Projection<'_>;
/// fn project_ref(self: Pin<&Self>) -> ProjectionRef<'_>;
/// # }
/// ```
///
/// By passing an attribute with the same name as the method to the macro,
/// you can name the projection type returned from the method. This allows you
/// to use pattern matching on the projected types.
///
/// ```rust
/// # use pin_project_lite::pin_project;
/// # use std::pin::Pin;
/// pin_project! {
///     #[project = EnumProj]
///     enum Enum<T> {
///         Variant { #[pin] field: T },
///     }
/// }
///
/// impl<T> Enum<T> {
///     fn method(self: Pin<&mut Self>) {
///         let this: EnumProj<'_, T> = self.project();
///         match this {
///             EnumProj::Variant { field } => {
///                 let _: Pin<&mut T> = field;
///             }
///         }
///     }
/// }
/// ```
///
/// By passing the `#[project_replace = MyProjReplace]` attribute you may create an additional
/// method which allows the contents of `Pin<&mut Self>` to be replaced while simultaneously moving
/// out all unpinned fields in `Self`.
///
/// ```rust
/// # use std::pin::Pin;
/// # type MyProjReplace = ();
/// # trait Dox {
/// fn project_replace(self: Pin<&mut Self>, replacement: Self) -> MyProjReplace;
/// # }
/// ```
///
/// Also, note that the projection types returned by `project` and `project_ref` have
/// an additional lifetime at the beginning of generics.
///
/// ```text
/// let this: EnumProj<'_, T> = self.project();
///                    ^^
/// ```
///
/// The visibility of the projected types and projection methods is based on the
/// original type. However, if the visibility of the original type is `pub`, the
/// visibility of the projected types and the projection methods is downgraded
/// to `pub(crate)`.
///
/// # Safety
///
/// `pin_project!` macro guarantees safety in much the same way as [pin-project] crate.
/// Both are completely safe unless you write other unsafe code.
///
/// See [pin-project] crate for more details.
///
/// # Examples
///
/// ```rust
/// use std::pin::Pin;
///
/// use pin_project_lite::pin_project;
///
/// pin_project! {
///     struct Struct<T, U> {
///         #[pin]
///         pinned: T,
///         unpinned: U,
///     }
/// }
///
/// impl<T, U> Struct<T, U> {
///     fn method(self: Pin<&mut Self>) {
///         let this = self.project();
///         let _: Pin<&mut T> = this.pinned; // Pinned reference to the field
///         let _: &mut U = this.unpinned; // Normal reference to the field
///     }
/// }
/// ```
///
/// To use `pin_project!` on enums, you need to name the projection type
/// returned from the method.
///
/// ```rust
/// use std::pin::Pin;
///
/// use pin_project_lite::pin_project;
///
/// pin_project! {
///     #[project = EnumProj]
///     enum Enum<T> {
///         Struct {
///             #[pin]
///             field: T,
///         },
///         Unit,
///     }
/// }
///
/// impl<T> Enum<T> {
///     fn method(self: Pin<&mut Self>) {
///         match self.project() {
///             EnumProj::Struct { field } => {
///                 let _: Pin<&mut T> = field;
///             }
///             EnumProj::Unit => {}
///         }
///     }
/// }
/// ```
///
/// If you want to call the `project()` method multiple times or later use the
/// original [`Pin`] type, it needs to use [`.as_mut()`][`Pin::as_mut`] to avoid
/// consuming the [`Pin`].
///
/// ```rust
/// use std::pin::Pin;
///
/// use pin_project_lite::pin_project;
///
/// pin_project! {
///     struct Struct<T> {
///         #[pin]
///         field: T,
///     }
/// }
///
/// impl<T> Struct<T> {
///     fn call_project_twice(mut self: Pin<&mut Self>) {
///         // `project` consumes `self`, so reborrow the `Pin<&mut Self>` via `as_mut`.
///         self.as_mut().project();
///         self.as_mut().project();
///     }
/// }
/// ```
///
/// # `!Unpin`
///
/// If you want to ensure that [`Unpin`] is not implemented, use `#[pin]`
/// attribute for a [`PhantomPinned`] field.
///
/// ```rust
/// use std::marker::PhantomPinned;
///
/// use pin_project_lite::pin_project;
///
/// pin_project! {
///     struct Struct<T> {
///         field: T,
///         #[pin] // <------ This `#[pin]` is required to make `Struct` to `!Unpin`.
///         _pin: PhantomPinned,
///     }
/// }
/// ```
///
/// Note that using [`PhantomPinned`] without `#[pin]` attribute has no effect.
///
/// [`PhantomPinned`]: core::marker::PhantomPinned
/// [`Pin::as_mut`]: core::pin::Pin::as_mut
/// [`Pin`]: core::pin::Pin
/// [pin-project]: https://github.com/taiki-e/pin-project
#[macro_export]
macro_rules! pin_project {
    ($($tt:tt)*) => {
        $crate::__pin_project_internal! {
            [][][][]
            $($tt)*
        }
    };
}

// limitations:
// * no support for tuple structs and tuple variant (wontfix).
// * no support for multiple trait/lifetime bounds.
// * no support for `Self` in where clauses. (wontfix)
// * no support for overlapping lifetime names. (wontfix)
// * no interoperability with other field attributes.
// * no useful error messages. (wontfix)
// etc...

// Not public API.
#[doc(hidden)]
#[macro_export]
macro_rules! __pin_project_internal {
    // =============================================================================================
    // struct:main
    (@struct=>internal;
        [$($proj_mut_ident:ident)?]
        [$($proj_ref_ident:ident)?]
        [$($proj_replace_ident:ident)?]
        [$proj_vis:vis]
        [$(#[$attrs:meta])* $vis:vis struct $ident:ident]
        [$($def_generics:tt)*]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)*)?]
        {
            $(
                $(#[$pin:ident])?
                $field_vis:vis $field:ident: $field_ty:ty
            ),+
        }
    ) => {
        $(#[$attrs])*
        $vis struct $ident $($def_generics)*
        $(where
            $($where_clause)*)?
        {
            $(
                $field_vis $field: $field_ty
            ),+
        }

        $crate::__pin_project_internal! { @struct=>make_proj_ty=>named;
            [$proj_vis]
            [$($proj_mut_ident)?]
            [make_proj_field_mut]
            [$ident]
            [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
            {
                $(
                    $(#[$pin])?
                    $field_vis $field: $field_ty
                ),+
            }
        }
        $crate::__pin_project_internal! { @struct=>make_proj_ty=>named;
            [$proj_vis]
            [$($proj_ref_ident)?]
            [make_proj_field_ref]
            [$ident]
            [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
            {
                $(
                    $(#[$pin])?
                    $field_vis $field: $field_ty
                ),+
            }
        }
        $crate::__pin_project_internal! { @struct=>make_proj_replace_ty=>named;
            [$proj_vis]
            [$($proj_replace_ident)?]
            [make_proj_field_replace]
            [$ident]
            [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
            {
                $(
                    $(#[$pin])?
                    $field_vis $field: $field_ty
                ),+
            }
        }

        #[allow(explicit_outlives_requirements)] // https://github.com/rust-lang/rust/issues/60993
        #[allow(single_use_lifetimes)] // https://github.com/rust-lang/rust/issues/55058
        // This lint warns of `clippy::*` generated by external macros.
        // We allow this lint for compatibility with older compilers.
        #[allow(clippy::unknown_clippy_lints)]
        #[allow(clippy::redundant_pub_crate)] // This lint warns `pub(crate)` field in private struct.
        #[allow(clippy::used_underscore_binding)]
        const _: () = {
            $crate::__pin_project_internal! { @struct=>make_proj_ty=>unnamed;
                [$proj_vis]
                [$($proj_mut_ident)?][Projection]
                [make_proj_field_mut]
                [$ident]
                [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
                {
                    $(
                        $(#[$pin])?
                        $field_vis $field: $field_ty
                    ),+
                }
            }
            $crate::__pin_project_internal! { @struct=>make_proj_ty=>unnamed;
                [$proj_vis]
                [$($proj_ref_ident)?][ProjectionRef]
                [make_proj_field_ref]
                [$ident]
                [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
                {
                    $(
                        $(#[$pin])?
                        $field_vis $field: $field_ty
                    ),+
                }
            }
            $crate::__pin_project_internal! { @struct=>make_proj_replace_ty=>unnamed;
                [$proj_vis]
                [$($proj_replace_ident)?][ProjectionReplace]
                [make_proj_field_replace]
                [$ident]
                [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
                {
                    $(
                        $(#[$pin])?
                        $field_vis $field: $field_ty
                    ),+
                }
            }

            impl <$($impl_generics)*> $ident <$($ty_generics)*>
            $(where
                $($where_clause)*)?
            {
                $crate::__pin_project_internal! { @struct=>make_proj_method;
                    [$proj_vis]
                    [$($proj_mut_ident)?][Projection]
                    [project get_unchecked_mut mut]
                    [$($ty_generics)*]
                    {
                        $(
                            $(#[$pin])?
                            $field_vis $field
                        ),+
                    }
                }
                $crate::__pin_project_internal! { @struct=>make_proj_method;
                    [$proj_vis]
                    [$($proj_ref_ident)?][ProjectionRef]
                    [project_ref get_ref]
                    [$($ty_generics)*]
                    {
                        $(
                            $(#[$pin])?
                            $field_vis $field
                        ),+
                    }
                }
                $crate::__pin_project_internal! { @struct=>make_proj_replace_method;
                    [$proj_vis]
                    [$($proj_replace_ident)?][ProjectionReplace]
                    [$($ty_generics)*]
                    {
                        $(
                            $(#[$pin])?
                            $field_vis $field
                        ),+
                    }
                }
            }

            $crate::__pin_project_internal! { @make_unpin_impl;
                [$vis $ident]
                [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
                $(
                    $field: $crate::__pin_project_internal!(@make_unpin_bound;
                        $(#[$pin])? $field_ty
                    )
                ),+
            }

            $crate::__pin_project_internal! { @make_drop_impl;
                [$ident]
                [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
            }

            // Ensure that it's impossible to use pin projections on a #[repr(packed)] struct.
            //
            // Taking a reference to a packed field is UB, and applying
            // `#[forbid(unaligned_references)]` makes sure that doing this is a hard error.
            //
            // If the struct ends up having #[repr(packed)] applied somehow,
            // this will generate an (unfriendly) error message. Under all reasonable
            // circumstances, we'll detect the #[repr(packed)] attribute, and generate
            // a much nicer error above.
            //
            // See https://github.com/taiki-e/pin-project/pull/34 for more details.
            //
            // Note:
            // - Lint-based tricks aren't perfect, but they're much better than nothing:
            //   https://github.com/taiki-e/pin-project-lite/issues/26
            //
            // - Enable both unaligned_references and safe_packed_borrows lints
            //   because unaligned_references lint does not exist in older compilers:
            //   https://github.com/taiki-e/pin-project-lite/pull/55
            //   https://github.com/rust-lang/rust/pull/82525
            #[forbid(unaligned_references, safe_packed_borrows)]
            fn __assert_not_repr_packed <$($impl_generics)*> (this: &$ident <$($ty_generics)*>)
            $(where
                $($where_clause)*)?
            {
                $(
                    let _ = &this.$field;
                )+
            }
        };
    };
    // =============================================================================================
    // enum:main
    (@enum=>internal;
        [$($proj_mut_ident:ident)?]
        [$($proj_ref_ident:ident)?]
        [$($proj_replace_ident:ident)?]
        [$proj_vis:vis]
        [$(#[$attrs:meta])* $vis:vis enum $ident:ident]
        [$($def_generics:tt)*]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)*)?]
        {
            $(
                $(#[$variant_attrs:meta])*
                $variant:ident $({
                    $(
                        $(#[$pin:ident])?
                        $field:ident: $field_ty:ty
                    ),+
                })?
            ),+
        }
    ) => {
        $(#[$attrs])*
        $vis enum $ident $($def_generics)*
        $(where
            $($where_clause)*)?
        {
            $(
                $(#[$variant_attrs])*
                $variant $({
                    $(
                        $field: $field_ty
                    ),+
                })?
            ),+
        }

        $crate::__pin_project_internal! { @enum=>make_proj_ty;
            [$proj_vis]
            [$($proj_mut_ident)?]
            [make_proj_field_mut]
            [$ident]
            [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
            {
                $(
                    $variant $({
                        $(
                            $(#[$pin])?
                            $field: $field_ty
                        ),+
                    })?
                ),+
            }
        }
        $crate::__pin_project_internal! { @enum=>make_proj_ty;
            [$proj_vis]
            [$($proj_ref_ident)?]
            [make_proj_field_ref]
            [$ident]
            [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
            {
                $(
                    $variant $({
                        $(
                            $(#[$pin])?
                            $field: $field_ty
                        ),+
                    })?
                ),+
            }
        }
        $crate::__pin_project_internal! { @enum=>make_proj_replace_ty;
            [$proj_vis]
            [$($proj_replace_ident)?]
            [make_proj_field_replace]
            [$ident]
            [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
            {
                $(
                    $variant $({
                        $(
                            $(#[$pin])?
                            $field: $field_ty
                        ),+
                    })?
                ),+
            }
        }

        #[allow(single_use_lifetimes)] // https://github.com/rust-lang/rust/issues/55058
        // This lint warns of `clippy::*` generated by external macros.
        // We allow this lint for compatibility with older compilers.
        #[allow(clippy::unknown_clippy_lints)]
        #[allow(clippy::used_underscore_binding)]
        const _: () = {
            impl <$($impl_generics)*> $ident <$($ty_generics)*>
            $(where
                $($where_clause)*)?
            {
                $crate::__pin_project_internal! { @enum=>make_proj_method;
                    [$proj_vis]
                    [$($proj_mut_ident)?]
                    [project get_unchecked_mut mut]
                    [$($ty_generics)*]
                    {
                        $(
                            $variant $({
                                $(
                                    $(#[$pin])?
                                    $field
                                ),+
                            })?
                        ),+
                    }
                }
                $crate::__pin_project_internal! { @enum=>make_proj_method;
                    [$proj_vis]
                    [$($proj_ref_ident)?]
                    [project_ref get_ref]
                    [$($ty_generics)*]
                    {
                        $(
                            $variant $({
                                $(
                                    $(#[$pin])?
                                    $field
                                ),+
                            })?
                        ),+
                    }
                }
                $crate::__pin_project_internal! { @enum=>make_proj_replace_method;
                    [$proj_vis]
                    [$($proj_replace_ident)?]
                    [$($ty_generics)*]
                    {
                        $(
                            $variant $({
                                $(
                                    $(#[$pin])?
                                    $field
                                ),+
                            })?
                        ),+
                    }
                }
            }

            $crate::__pin_project_internal! { @make_unpin_impl;
                [$vis $ident]
                [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
                $(
                    $variant: ($(
                        $(
                            $crate::__pin_project_internal!(@make_unpin_bound;
                                $(#[$pin])? $field_ty
                            )
                        ),+
                    )?)
                ),+
            }

            $crate::__pin_project_internal! { @make_drop_impl;
                [$ident]
                [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
            }

            // We don't need to check for '#[repr(packed)]',
            // since it does not apply to enums.
        };
    };

    // =============================================================================================
    // struct:make_proj_ty
    (@struct=>make_proj_ty=>unnamed;
        [$proj_vis:vis]
        [$_proj_ty_ident:ident][$proj_ty_ident:ident]
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        $($field:tt)*
    ) => {};
    (@struct=>make_proj_ty=>unnamed;
        [$proj_vis:vis]
        [][$proj_ty_ident:ident]
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        $($field:tt)*
    ) => {
        $crate::__pin_project_internal! { @struct=>make_proj_ty=>named;
            [$proj_vis]
            [$proj_ty_ident]
            [$make_proj_field]
            [$ident]
            [$($impl_generics)*] [$($ty_generics)*] [$(where $($where_clause)*)?]
            $($field)*
        }
    };
    (@struct=>make_proj_ty=>named;
        [$proj_vis:vis]
        [$proj_ty_ident:ident]
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        {
            $(
                $(#[$pin:ident])?
                $field_vis:vis $field:ident: $field_ty:ty
            ),+
        }
    ) => {
        #[allow(dead_code)] // This lint warns unused fields/variants.
        #[allow(single_use_lifetimes)] // https://github.com/rust-lang/rust/issues/55058
        // This lint warns of `clippy::*` generated by external macros.
        // We allow this lint for compatibility with older compilers.
        #[allow(clippy::unknown_clippy_lints)]
        #[allow(clippy::mut_mut)] // This lint warns `&mut &mut <ty>`. (only needed for project)
        #[allow(clippy::redundant_pub_crate)] // This lint warns `pub(crate)` field in private struct.
        #[allow(clippy::ref_option_ref)] // This lint warns `&Option<&<ty>>`. (only needed for project_ref)
        #[allow(clippy::type_repetition_in_bounds)] // https://github.com/rust-lang/rust-clippy/issues/4326
        $proj_vis struct $proj_ty_ident <'__pin, $($impl_generics)*>
        where
            $ident <$($ty_generics)*>: '__pin
            $(, $($where_clause)*)?
        {
            $(
                $field_vis $field: $crate::__pin_project_internal!(@$make_proj_field;
                    $(#[$pin])? $field_ty
                )
            ),+
        }
    };
    (@struct=>make_proj_ty=>named;
        [$proj_vis:vis]
        []
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        $($field:tt)*
    ) => {};

    (@struct=>make_proj_replace_ty=>unnamed;
        [$proj_vis:vis]
        [$_proj_ty_ident:ident][$proj_ty_ident:ident]
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        $($field:tt)*
    ) => {};
    (@struct=>make_proj_replace_ty=>unnamed;
        [$proj_vis:vis]
        [][$proj_ty_ident:ident]
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        $($field:tt)*
    ) => {
    };
    (@struct=>make_proj_replace_ty=>named;
        [$proj_vis:vis]
        [$proj_ty_ident:ident]
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        {
            $(
                $(#[$pin:ident])?
                $field_vis:vis $field:ident: $field_ty:ty
            ),+
        }
    ) => {
        #[allow(dead_code)] // This lint warns unused fields/variants.
        #[allow(single_use_lifetimes)] // https://github.com/rust-lang/rust/issues/55058
        #[allow(clippy::mut_mut)] // This lint warns `&mut &mut <ty>`. (only needed for project)
        #[allow(clippy::redundant_pub_crate)] // This lint warns `pub(crate)` field in private struct.
        #[allow(clippy::type_repetition_in_bounds)] // https://github.com/rust-lang/rust-clippy/issues/4326
        $proj_vis struct $proj_ty_ident <$($impl_generics)*>
        where
            $($($where_clause)*)?
        {
            $(
                $field_vis $field: $crate::__pin_project_internal!(@$make_proj_field;
                    $(#[$pin])? $field_ty
                )
            ),+
        }
    };
    (@struct=>make_proj_replace_ty=>named;
        [$proj_vis:vis]
        []
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        $($field:tt)*
    ) => {};
    // =============================================================================================
    // enum:make_proj_ty
    (@enum=>make_proj_ty;
        [$proj_vis:vis]
        [$proj_ty_ident:ident]
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        {
            $(
                $variant:ident $({
                    $(
                        $(#[$pin:ident])?
                        $field:ident: $field_ty:ty
                    ),+
                })?
            ),+
        }
    ) => {
        #[allow(dead_code)] // This lint warns unused fields/variants.
        #[allow(single_use_lifetimes)] // https://github.com/rust-lang/rust/issues/55058
        // This lint warns of `clippy::*` generated by external macros.
        // We allow this lint for compatibility with older compilers.
        #[allow(clippy::unknown_clippy_lints)]
        #[allow(clippy::mut_mut)] // This lint warns `&mut &mut <ty>`. (only needed for project)
        #[allow(clippy::redundant_pub_crate)] // This lint warns `pub(crate)` field in private struct.
        #[allow(clippy::ref_option_ref)] // This lint warns `&Option<&<ty>>`. (only needed for project_ref)
        #[allow(clippy::type_repetition_in_bounds)] // https://github.com/rust-lang/rust-clippy/issues/4326
        $proj_vis enum $proj_ty_ident <'__pin, $($impl_generics)*>
        where
            $ident <$($ty_generics)*>: '__pin
            $(, $($where_clause)*)?
        {
            $(
                $variant $({
                    $(
                        $field: $crate::__pin_project_internal!(@$make_proj_field;
                            $(#[$pin])? $field_ty
                        )
                    ),+
                })?
            ),+
        }
    };
    (@enum=>make_proj_ty;
        [$proj_vis:vis]
        []
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        $($variant:tt)*
    ) => {};

    (@enum=>make_proj_replace_ty;
        [$proj_vis:vis]
        [$proj_ty_ident:ident]
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        {
            $(
                $variant:ident $({
                    $(
                        $(#[$pin:ident])?
                        $field:ident: $field_ty:ty
                    ),+
                })?
            ),+
        }
    ) => {
        #[allow(dead_code)] // This lint warns unused fields/variants.
        #[allow(single_use_lifetimes)] // https://github.com/rust-lang/rust/issues/55058
        #[allow(clippy::mut_mut)] // This lint warns `&mut &mut <ty>`. (only needed for project)
        #[allow(clippy::redundant_pub_crate)] // This lint warns `pub(crate)` field in private struct.
        #[allow(clippy::type_repetition_in_bounds)] // https://github.com/rust-lang/rust-clippy/issues/4326
        $proj_vis enum $proj_ty_ident <$($impl_generics)*>
        where
            $($($where_clause)*)?
        {
            $(
                $variant $({
                    $(
                        $field: $crate::__pin_project_internal!(@$make_proj_field;
                            $(#[$pin])? $field_ty
                        )
                    ),+
                })?
            ),+
        }
    };
    (@enum=>make_proj_replace_ty;
        [$proj_vis:vis]
        []
        [$make_proj_field:ident]
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        $($variant:tt)*
    ) => {};

    // =============================================================================================
    (@make_proj_replace_block;
        [$($proj_path: tt)+]
        {
            $(
                $(#[$pin:ident])?
                $field_vis:vis $field:ident
            ),+
        }
    ) => {
        let result = $($proj_path)* {
            $(
                $field: $crate::__pin_project_internal!(@make_replace_field_proj;
                    $(#[$pin])? $field
                )
            ),+
        };

        {
            ( $(
                $crate::__pin_project_internal!(@make_unsafe_drop_in_place_guard;
                    $(#[$pin])? $field
                ),
            )* );
        }

        result
    };
    (@make_proj_replace_block;
        [$($proj_path: tt)+]
    ) => {
        $($proj_path)*
    };

    // =============================================================================================
    // struct:make_proj_method
    (@struct=>make_proj_method;
        [$proj_vis:vis]
        [$proj_ty_ident:ident][$_proj_ty_ident:ident]
        [$method_ident:ident $get_method:ident $($mut:ident)?]
        [$($ty_generics:tt)*]
        {
            $(
                $(#[$pin:ident])?
                $field_vis:vis $field:ident
            ),+
        }
    ) => {
        $proj_vis fn $method_ident<'__pin>(
            self: $crate::__private::Pin<&'__pin $($mut)? Self>,
        ) -> $proj_ty_ident <'__pin, $($ty_generics)*> {
            unsafe {
                let Self { $($field),* } = self.$get_method();
                $proj_ty_ident {
                    $(
                        $field: $crate::__pin_project_internal!(@make_unsafe_field_proj;
                            $(#[$pin])? $field
                        )
                    ),+
                }
            }
        }
    };
    (@struct=>make_proj_method;
        [$proj_vis:vis]
        [][$proj_ty_ident:ident]
        [$method_ident:ident $get_method:ident $($mut:ident)?]
        [$($ty_generics:tt)*]
        $($variant:tt)*
    ) => {
        $crate::__pin_project_internal! { @struct=>make_proj_method;
            [$proj_vis]
            [$proj_ty_ident][$proj_ty_ident]
            [$method_ident $get_method $($mut)?]
            [$($ty_generics)*]
            $($variant)*
        }
    };

    (@struct=>make_proj_replace_method;
        [$proj_vis:vis]
        [$proj_ty_ident:ident][$_proj_ty_ident:ident]
        [$($ty_generics:tt)*]
        {
            $(
                $(#[$pin:ident])?
                $field_vis:vis $field:ident
            ),+
        }
    ) => {
        $proj_vis fn project_replace(
            self: $crate::__private::Pin<&mut Self>,
            replacement: Self,
        ) -> $proj_ty_ident <$($ty_generics)*> {
            unsafe {
                let __self_ptr: *mut Self = self.get_unchecked_mut();

                // Destructors will run in reverse order, so next create a guard to overwrite
                // `self` with the replacement value without calling destructors.
                let __guard = $crate::__private::UnsafeOverwriteGuard {
                    target: __self_ptr,
                    value: $crate::__private::ManuallyDrop::new(replacement),
                };

                let Self { $($field),* } = &mut *__self_ptr;

                $crate::__pin_project_internal!{@make_proj_replace_block;
                    [$proj_ty_ident]
                    {
                        $(
                            $(#[$pin])?
                            $field
                        ),+
                    }
                }
            }
        }
    };
    (@struct=>make_proj_replace_method;
        [$proj_vis:vis]
        [][$proj_ty_ident:ident]
        [$($ty_generics:tt)*]
        $($variant:tt)*
    ) => {
    };

    // =============================================================================================
    // enum:make_proj_method
    (@enum=>make_proj_method;
        [$proj_vis:vis]
        [$proj_ty_ident:ident]
        [$method_ident:ident $get_method:ident $($mut:ident)?]
        [$($ty_generics:tt)*]
        {
            $(
                $variant:ident $({
                    $(
                        $(#[$pin:ident])?
                        $field:ident
                    ),+
                })?
            ),+
        }
    ) => {
        $proj_vis fn $method_ident<'__pin>(
            self: $crate::__private::Pin<&'__pin $($mut)? Self>,
        ) -> $proj_ty_ident <'__pin, $($ty_generics)*> {
            unsafe {
                match self.$get_method() {
                    $(
                        Self::$variant $({
                            $($field),+
                        })? => {
                            $proj_ty_ident::$variant $({
                                $(
                                    $field: $crate::__pin_project_internal!(
                                        @make_unsafe_field_proj;
                                        $(#[$pin])? $field
                                    )
                                ),+
                            })?
                        }
                    ),+
                }
            }
        }
    };
    (@enum=>make_proj_method;
        [$proj_vis:vis]
        []
        [$method_ident:ident $get_method:ident $($mut:ident)?]
        [$($ty_generics:tt)*]
        $($variant:tt)*
    ) => {};

    (@enum=>make_proj_replace_method;
        [$proj_vis:vis]
        [$proj_ty_ident:ident]
        [$($ty_generics:tt)*]
        {
            $(
                $variant:ident $({
                    $(
                        $(#[$pin:ident])?
                        $field:ident
                    ),+
                })?
            ),+
        }
    ) => {
        $proj_vis fn project_replace(
            self: $crate::__private::Pin<&mut Self>,
            replacement: Self,
        ) -> $proj_ty_ident <$($ty_generics)*> {
            unsafe {
                let __self_ptr: *mut Self = self.get_unchecked_mut();

                // Destructors will run in reverse order, so next create a guard to overwrite
                // `self` with the replacement value without calling destructors.
                let __guard = $crate::__private::UnsafeOverwriteGuard {
                    target: __self_ptr,
                    value: $crate::__private::ManuallyDrop::new(replacement),
                };

                match &mut *__self_ptr {
                    $(
                        Self::$variant $({
                            $($field),+
                        })? => {
                            $crate::__pin_project_internal!{@make_proj_replace_block;
                                [$proj_ty_ident :: $variant]
                                $({
                                    $(
                                        $(#[$pin])?
                                        $field
                                    ),+
                                })?
                            }
                        }
                    ),+
                }
            }
        }
    };
    (@enum=>make_proj_replace_method;
        [$proj_vis:vis]
        []
        [$($ty_generics:tt)*]
        $($variant:tt)*
    ) => {};

    // =============================================================================================
    // make_unpin_impl
    (@make_unpin_impl;
        [$vis:vis $ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
        $($field:tt)*
    ) => {
        // Automatically create the appropriate conditional `Unpin` implementation.
        //
        // Basically this is equivalent to the following code:
        // ```rust
        // impl<T, U> Unpin for Struct<T, U> where T: Unpin {}
        // ```
        //
        // However, if struct is public and there is a private type field,
        // this would cause an E0446 (private type in public interface).
        //
        // When RFC 2145 is implemented (rust-lang/rust#48054),
        // this will become a lint, rather then a hard error.
        //
        // As a workaround for this, we generate a new struct, containing all of the pinned
        // fields from our #[pin_project] type. This struct is delcared within
        // a function, which makes it impossible to be named by user code.
        // This guarnatees that it will use the default auto-trait impl for Unpin -
        // that is, it will implement Unpin iff all of its fields implement Unpin.
        // This type can be safely declared as 'public', satisfiying the privacy
        // checker without actually allowing user code to access it.
        //
        // This allows users to apply the #[pin_project] attribute to types
        // regardless of the privacy of the types of their fields.
        //
        // See also https://github.com/taiki-e/pin-project/pull/53.
        #[allow(non_snake_case)]
        $vis struct __Origin <'__pin, $($impl_generics)*>
        $(where
            $($where_clause)*)?
        {
            __dummy_lifetime: $crate::__private::PhantomData<&'__pin ()>,
            $($field)*
        }
        impl <'__pin, $($impl_generics)*> $crate::__private::Unpin for $ident <$($ty_generics)*>
        where
            __Origin <'__pin, $($ty_generics)*>: $crate::__private::Unpin
            $(, $($where_clause)*)?
        {
        }
    };

    // =============================================================================================
    // make_drop_impl
    (@make_drop_impl;
        [$ident:ident]
        [$($impl_generics:tt)*] [$($ty_generics:tt)*] [$(where $($where_clause:tt)* )?]
    ) => {
        // Ensure that struct does not implement `Drop`.
        //
        // There are two possible cases:
        // 1. The user type does not implement Drop. In this case,
        // the first blanked impl will not apply to it. This code
        // will compile, as there is only one impl of MustNotImplDrop for the user type
        // 2. The user type does impl Drop. This will make the blanket impl applicable,
        // which will then comflict with the explicit MustNotImplDrop impl below.
        // This will result in a compilation error, which is exactly what we want.
        trait MustNotImplDrop {}
        #[allow(clippy::drop_bounds, drop_bounds)]
        impl<T: $crate::__private::Drop> MustNotImplDrop for T {}
        impl <$($impl_generics)*> MustNotImplDrop for $ident <$($ty_generics)*>
        $(where
            $($where_clause)*)?
        {
        }
    };

    // =============================================================================================
    // make_unpin_bound
    (@make_unpin_bound;
        #[pin]
        $field_ty:ty
    ) => {
        $field_ty
    };
    (@make_unpin_bound;
        $field_ty:ty
    ) => {
        $crate::__private::AlwaysUnpin<$field_ty>
    };

    // =============================================================================================
    // make_unsafe_field_proj
    (@make_unsafe_field_proj;
        #[pin]
        $field:ident
    ) => {
        $crate::__private::Pin::new_unchecked($field)
    };
    (@make_unsafe_field_proj;
        $field:ident
    ) => {
        $field
    };

    // =============================================================================================
    // make_replace_field_proj
    (@make_replace_field_proj;
        #[pin]
        $field:ident
    ) => {
        $crate::__private::PhantomData
    };
    (@make_replace_field_proj;
        $field:ident
    ) => {
        $crate::__private::ptr::read($field)
    };


    // =============================================================================================
    // make_unsafe_drop_in_place_guard
    (@make_unsafe_drop_in_place_guard;
        #[pin]
        $field:ident
    ) => {
        $crate::__private::UnsafeDropInPlaceGuard($field)
    };
    (@make_unsafe_drop_in_place_guard;
        $field:ident
    ) => {
        ()
    };

    // =============================================================================================
    // make_proj_field
    (@make_proj_field_mut;
        #[pin]
        $field_ty:ty
    ) => {
        $crate::__private::Pin<&'__pin mut ($field_ty)>
    };
    (@make_proj_field_mut;
        $field_ty:ty
    ) => {
        &'__pin mut ($field_ty)
    };
    (@make_proj_field_ref;
        #[pin]
        $field_ty:ty
    ) => {
        $crate::__private::Pin<&'__pin ($field_ty)>
    };
    (@make_proj_field_ref;
        $field_ty:ty
    ) => {
        &'__pin ($field_ty)
    };

    (@make_proj_field_replace;
        #[pin]
        $field_ty:ty
    ) => {
        $crate::__private::PhantomData<$field_ty>
    };
    (@make_proj_field_replace;
        $field_ty:ty
    ) => {
        $field_ty
    };

    // =============================================================================================
    // Parses input and determines visibility

    (
        []
        [$($proj_ref_ident:ident)?]
        [$($proj_replace_ident:ident)?]
        [$($attrs:tt)*]

        #[project = $proj_mut_ident:ident]
        $($tt:tt)*
    ) => {
        $crate::__pin_project_internal! {
            [$proj_mut_ident]
            [$($proj_ref_ident)?]
            [$($proj_replace_ident)?]
            [$($attrs)*]
            $($tt)*
        }
    };

    {
        [$($proj_mut_ident:ident)?]
        []
        [$($proj_replace_ident:ident)?]
        [$($attrs:tt)*]

        #[project_ref = $proj_ref_ident:ident]
        $($tt:tt)*
    } => {
        $crate::__pin_project_internal! {
            [$($proj_mut_ident)?]
            [$proj_ref_ident]
            [$($proj_replace_ident)?]
            [$($attrs)*]
            $($tt)*
        }
    };

    {
        [$($proj_mut_ident:ident)?]
        [$($proj_ref_ident:ident)?]
        []
        [$($attrs:tt)*]

        #[project_replace = $proj_replace_ident:ident]
        $($tt:tt)*
    } => {
        $crate::__pin_project_internal! {
            [$($proj_mut_ident)?]
            [$($proj_ref_ident)?]
            [$proj_replace_ident]
            [$($attrs)*]
            $($tt)*
        }
    };

    {
        [$($proj_mut_ident:ident)?]
        [$($proj_ref_ident:ident)?]
        [$($proj_replace_ident:ident)?]
        [$($attrs:tt)*]

        #[$($attr:tt)*]
        $($tt:tt)*
    } => {
        $crate::__pin_project_internal! {
            [$($proj_mut_ident)?]
            [$($proj_ref_ident)?]
            [$($proj_replace_ident)?]
            [$($attrs)* #[$($attr)*]]
            $($tt)*
        }
    };

    // struct
    (
        [$($proj_mut_ident:ident)?]
        [$($proj_ref_ident:ident)?]
        [$($proj_replace_ident:ident)?]
        [$($attrs:tt)*]

        pub struct $ident:ident $(<
            $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ),* $(,)?
            $( $generics:ident
                $(: $generics_bound:path)?
                $(: ?$generics_unsized_bound:path)?
                $(: $generics_lifetime_bound:lifetime)?
                $(= $generics_default:ty)?
            ),* $(,)?
        >)?
        $(where
            $( $where_clause_ty:ty
                $(: $where_clause_bound:path)?
                $(: ?$where_clause_unsized_bound:path)?
                $(: $where_clause_lifetime_bound:lifetime)?
            ),* $(,)?
        )?
        {
            $(
                $(#[$pin:ident])?
                $field_vis:vis $field:ident: $field_ty:ty
            ),+ $(,)?
        }
    ) => {
        $crate::__pin_project_internal! { @struct=>internal;
            [$($proj_mut_ident)?]
            [$($proj_ref_ident)?]
            [$($proj_replace_ident)?]
            [pub(crate)]
            [$($attrs)* pub struct $ident]
            [$(<
                $( $lifetime $(: $lifetime_bound)? ,)*
                $( $generics
                    $(: $generics_bound)?
                    $(: ?$generics_unsized_bound)?
                    $(: $generics_lifetime_bound)?
                    $(= $generics_default)?
                ),*
            >)?]
            [$(
                $( $lifetime $(: $lifetime_bound)? ,)*
                $( $generics
                    $(: $generics_bound)?
                    $(: ?$generics_unsized_bound)?
                    $(: $generics_lifetime_bound)?
                ),*
            )?]
            [$( $( $lifetime ,)* $( $generics ),* )?]
            [$(where $( $where_clause_ty
                $(: $where_clause_bound)?
                $(: ?$where_clause_unsized_bound)?
                $(: $where_clause_lifetime_bound)?
            ),* )?]
            {
                $(
                    $(#[$pin])?
                    $field_vis $field: $field_ty
                ),+
            }
        }
    };
    (
        [$($proj_mut_ident:ident)?]
        [$($proj_ref_ident:ident)?]
        [$($proj_replace_ident:ident)?]
        [$($attrs:tt)*]

        $vis:vis struct $ident:ident $(<
            $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ),* $(,)?
            $( $generics:ident
                $(: $generics_bound:path)?
                $(: ?$generics_unsized_bound:path)?
                $(: $generics_lifetime_bound:lifetime)?
                $(= $generics_default:ty)?
            ),* $(,)?
        >)?
        $(where
            $( $where_clause_ty:ty
                $(: $where_clause_bound:path)?
                $(: ?$where_clause_unsized_bound:path)?
                $(: $where_clause_lifetime_bound:lifetime)?
            ),* $(,)?
        )?
        {
            $(
                $(#[$pin:ident])?
                $field_vis:vis $field:ident: $field_ty:ty
            ),+ $(,)?
        }
    ) => {
        $crate::__pin_project_internal! { @struct=>internal;
            [$($proj_mut_ident)?]
            [$($proj_ref_ident)?]
            [$($proj_replace_ident)?]
            [$vis]
            [$($attrs)* $vis struct $ident]
            [$(<
                $( $lifetime $(: $lifetime_bound)? ,)*
                $( $generics
                    $(: $generics_bound)?
                    $(: ?$generics_unsized_bound)?
                    $(: $generics_lifetime_bound)?
                    $(= $generics_default)?
                ),*
            >)?]
            [$(
                $( $lifetime $(: $lifetime_bound)? ,)*
                $( $generics
                    $(: $generics_bound)?
                    $(: ?$generics_unsized_bound)?
                    $(: $generics_lifetime_bound)?
                ),*
            )?]
            [$( $( $lifetime ,)* $( $generics ),* )?]
            [$(where $( $where_clause_ty
                $(: $where_clause_bound)?
                $(: ?$where_clause_unsized_bound)?
                $(: $where_clause_lifetime_bound)?
            ),* )?]
            {
                $(
                    $(#[$pin])?
                    $field_vis $field: $field_ty
                ),+
            }
        }
    };
    // enum
    (
        [$($proj_mut_ident:ident)?]
        [$($proj_ref_ident:ident)?]
        [$($proj_replace_ident:ident)?]
        [$($attrs:tt)*]

        pub enum $ident:ident $(<
            $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ),* $(,)?
            $( $generics:ident
                $(: $generics_bound:path)?
                $(: ?$generics_unsized_bound:path)?
                $(: $generics_lifetime_bound:lifetime)?
                $(= $generics_default:ty)?
            ),* $(,)?
        >)?
        $(where
            $( $where_clause_ty:ty
                $(: $where_clause_bound:path)?
                $(: ?$where_clause_unsized_bound:path)?
                $(: $where_clause_lifetime_bound:lifetime)?
            ),* $(,)?
        )?
        {
            $(
                $(#[$variant_attrs:meta])*
                $variant:ident $({
                    $(
                        $(#[$pin:ident])?
                        $field:ident: $field_ty:ty
                    ),+ $(,)?
                })?
            ),+ $(,)?
        }
    ) => {
        $crate::__pin_project_internal! { @enum=>internal;
            [$($proj_mut_ident)?]
            [$($proj_ref_ident)?]
            [$($proj_replace_ident)?]
            [pub(crate)]
            [$($attrs)* pub enum $ident]
            [$(<
                $( $lifetime $(: $lifetime_bound)? ,)*
                $( $generics
                    $(: $generics_bound)?
                    $(: ?$generics_unsized_bound)?
                    $(: $generics_lifetime_bound)?
                    $(= $generics_default)?
                ),*
            >)?]
            [$(
                $( $lifetime $(: $lifetime_bound)? ,)*
                $( $generics
                    $(: $generics_bound)?
                    $(: ?$generics_unsized_bound)?
                    $(: $generics_lifetime_bound)?
                ),*
            )?]
            [$( $( $lifetime ,)* $( $generics ),* )?]
            [$(where $( $where_clause_ty
                $(: $where_clause_bound)?
                $(: ?$where_clause_unsized_bound)?
                $(: $where_clause_lifetime_bound)?
            ),* )?]
            {
                $(
                    $(#[$variant_attrs])*
                    $variant $({
                        $(
                            $(#[$pin])?
                            $field: $field_ty
                        ),+
                    })?
                ),+
            }
        }
    };
    (
        [$($proj_mut_ident:ident)?]
        [$($proj_ref_ident:ident)?]
        [$($proj_replace_ident:ident)?]
        [$($attrs:tt)*]

        $vis:vis enum $ident:ident $(<
            $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ),* $(,)?
            $( $generics:ident
                $(: $generics_bound:path)?
                $(: ?$generics_unsized_bound:path)?
                $(: $generics_lifetime_bound:lifetime)?
                $(= $generics_default:ty)?
            ),* $(,)?
        >)?
        $(where
            $( $where_clause_ty:ty
                $(: $where_clause_bound:path)?
                $(: ?$where_clause_unsized_bound:path)?
                $(: $where_clause_lifetime_bound:lifetime)?
            ),* $(,)?
        )?
        {
            $(
                $(#[$variant_attrs:meta])*
                $variant:ident $({
                    $(
                        $(#[$pin:ident])?
                        $field:ident: $field_ty:ty
                    ),+ $(,)?
                })?
            ),+ $(,)?
        }
    ) => {
        $crate::__pin_project_internal! { @enum=>internal;
            [$($proj_mut_ident)?]
            [$($proj_ref_ident)?]
            [$($proj_replace_ident)?]
            [$vis]
            [$($attrs)* $vis enum $ident]
            [$(<
                $( $lifetime $(: $lifetime_bound)? ,)*
                $( $generics
                    $(: $generics_bound)?
                    $(: ?$generics_unsized_bound)?
                    $(: $generics_lifetime_bound)?
                    $(= $generics_default)?
                ),*
            >)?]
            [$(
                $( $lifetime $(: $lifetime_bound)? ,)*
                $( $generics
                    $(: $generics_bound)?
                    $(: ?$generics_unsized_bound)?
                    $(: $generics_lifetime_bound)?
                ),*
            )?]
            [$( $( $lifetime ,)* $( $generics ),* )?]
            [$(where $( $where_clause_ty
                $(: $where_clause_bound)?
                $(: ?$where_clause_unsized_bound)?
                $(: $where_clause_lifetime_bound)?
            ),* )?]
            {
                $(
                    $(#[$variant_attrs])*
                    $variant $({
                        $(
                            $(#[$pin])?
                            $field: $field_ty
                        ),+
                    })?
                ),+
            }
        }
    };
}

// Not public API.
#[doc(hidden)]
pub mod __private {
    #[doc(hidden)]
    pub use core::{
        marker::{PhantomData, Unpin},
        mem::ManuallyDrop,
        ops::Drop,
        pin::Pin,
        ptr,
    };

    // This is an internal helper struct used by `pin_project!`.
    #[doc(hidden)]
    pub struct AlwaysUnpin<T: ?Sized>(PhantomData<T>);

    impl<T: ?Sized> Unpin for AlwaysUnpin<T> {}

    // This is an internal helper used to ensure a value is dropped.
    #[doc(hidden)]
    pub struct UnsafeDropInPlaceGuard<T: ?Sized>(pub *mut T);

    impl<T: ?Sized> Drop for UnsafeDropInPlaceGuard<T> {
        fn drop(&mut self) {
            unsafe {
                ptr::drop_in_place(self.0);
            }
        }
    }

    // This is an internal helper used to ensure a value is overwritten without
    // its destructor being called.
    #[doc(hidden)]
    pub struct UnsafeOverwriteGuard<T> {
        pub value: ManuallyDrop<T>,
        pub target: *mut T,
    }

    impl<T> Drop for UnsafeOverwriteGuard<T> {
        fn drop(&mut self) {
            unsafe {
                ptr::write(self.target, ptr::read(&*self.value));
            }
        }
    }
}
