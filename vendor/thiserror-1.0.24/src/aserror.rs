use std::error::Error;

pub trait AsDynError {
    fn as_dyn_error(&self) -> &(dyn Error + 'static);
}

impl<T: Error + 'static> AsDynError for T {
    #[inline]
    fn as_dyn_error(&self) -> &(dyn Error + 'static) {
        self
    }
}

impl AsDynError for dyn Error + 'static {
    #[inline]
    fn as_dyn_error(&self) -> &(dyn Error + 'static) {
        self
    }
}

impl AsDynError for dyn Error + Send + 'static {
    #[inline]
    fn as_dyn_error(&self) -> &(dyn Error + 'static) {
        self
    }
}

impl AsDynError for dyn Error + Send + Sync + 'static {
    #[inline]
    fn as_dyn_error(&self) -> &(dyn Error + 'static) {
        self
    }
}
