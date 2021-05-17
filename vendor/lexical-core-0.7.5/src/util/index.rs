//! Macro to facilitate indexing for unchecked variants.

/// Macro to index without bounds checking.
#[allow(unused_macros)]
macro_rules! unchecked_index {
    // Get
    ($container:ident[$index:expr]) => (
        * unsafe { $container.get_unchecked($index) }
    );

    // Get
    ($obj:ident$(.$subobj:ident)*[$index:expr]) => (
        * unsafe { $obj$(.$subobj)*.get_unchecked($index) }
    );
}

/// Macro to mutably index without bounds checking.
#[allow(unused_macros)]
macro_rules! unchecked_index_mut {
    // Get
    ($container:ident[$index:expr]) => {
        * unsafe { $container.get_unchecked_mut($index) }
    };

    // Set
    ($container:ident[$index:expr] = $rhs:expr) => (
        unsafe { *$container.get_unchecked_mut($index) = $rhs }
    );
}

/// Macro to index without bounds checking.
#[cfg(feature = "unchecked_index")]
macro_rules! index {
    // Get
    ($container:ident[$index:expr]) => (
        * unsafe { $container.get_unchecked($index) }
    );

    // Get
    ($obj:ident$(.$subobj:ident)*[$index:expr]) => (
        * unsafe { $obj$(.$subobj)*.get_unchecked($index) }
    );
}

/// Macro to mutably index without bounds checking.
#[cfg(feature = "unchecked_index")]
macro_rules! index_mut {
    // Get
    ($container:ident[$index:expr]) => (
        * unsafe { $container.get_unchecked_mut($index) }
    );

    // Set
    ($container:ident[$index:expr] = $rhs:expr) => (
        unsafe { *$container.get_unchecked_mut($index) = $rhs }
    );
}

/// Macro to index with bounds checking.
#[cfg(not(feature = "unchecked_index"))]
macro_rules! index {
    // Get
    ($container:ident[$index:expr]) => (
        $container[$index]
    );

    // Get
    ($obj:ident$(.$subobj:ident)*[$index:expr]) => (
        $obj$(.$subobj)*[$index]
    );
}

/// Macro to mutably index with bounds checking.
#[cfg(not(feature = "unchecked_index"))]
macro_rules! index_mut {
    // Get
    ($container:ident[$index:expr]) => (
        $container[$index]
    );

    // Set
    ($container:ident[$index:expr] = $rhs:expr) => (
        $container[$index] = $rhs
    );
}
