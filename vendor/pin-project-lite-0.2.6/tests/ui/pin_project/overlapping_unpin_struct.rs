use pin_project_lite::pin_project;
use std::marker::PhantomPinned;

pin_project! {
    struct Foo<T> {
        #[pin]
        inner: T,
    }
}

struct __Origin {}

impl Unpin for __Origin {}

fn is_unpin<T: Unpin>() {}

fn main() {
    is_unpin::<Foo<PhantomPinned>>(); //~ ERROR E0277
}
