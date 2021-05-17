use pin_project_lite::pin_project;

pin_project! { //~ ERROR E0496
    pub struct Foo<'__pin, T> { //~ ERROR E0263
        #[pin]
        field: &'__pin mut T,
    }
}

fn main() {}
