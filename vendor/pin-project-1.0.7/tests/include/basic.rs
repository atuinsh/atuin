include!("basic-safe-part.rs");

unsafe impl<T: ::pin_project::__private::Unpin, U: ::pin_project::__private::Unpin>
    ::pin_project::UnsafeUnpin for UnsafeUnpinStruct<T, U>
{
}
unsafe impl<T: ::pin_project::__private::Unpin, U: ::pin_project::__private::Unpin>
    ::pin_project::UnsafeUnpin for UnsafeUnpinTupleStruct<T, U>
{
}
unsafe impl<T: ::pin_project::__private::Unpin, U: ::pin_project::__private::Unpin>
    ::pin_project::UnsafeUnpin for UnsafeUnpinEnum<T, U>
{
}
